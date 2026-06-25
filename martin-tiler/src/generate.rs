//! The generation pipeline: multiple GeoTIFFs -> single sparse tile map (per grid).
//!
//! Pipeline per run:
//! 1. inspect inputs (bounds, native zoom)
//! 2. `gdalbuildvrt -addalpha` -> mosaic with a coverage mask (empty areas become transparent)
//! 3. per grid: `gdalwarp` -> reproject (alpha preserved)
//! 4. per grid: `gdal2tiles --xyz` -> sparse z/x/y pyramid (transparent tiles are skipped)
//! 5. per grid: `mbtiles::pack` -> sparse MBTiles + metadata
//!
//! The transparency-driven skipping in step 4 is what satisfies the mandatory requirement:
//! empty areas produce no tile images, only the (implicit) index of tiles that exist.

use std::path::{Path, PathBuf};
use std::time::Instant;

use mbtiles::{Mbtiles, PackCompression, TileScheme, pack};

use crate::error::{TilerError, TilerResult};
use crate::gdal::{GdalEnv, parse_progress_percent};
use crate::inspect::{inspect_many, inspect_one};
use crate::model::{
    BBox, CogOutput, GenerateOptions, GenerateReport, GridOutput, GridParams, ProgressEvent,
    RasterInfo, Resampling, TileFormat, TileGrid, ZoomTally,
};

/// Run a full generation. `on_progress` receives streamed [`ProgressEvent`]s.
pub async fn generate(
    gdal: &GdalEnv,
    opts: &GenerateOptions,
    mut on_progress: impl FnMut(ProgressEvent),
) -> TilerResult<GenerateReport> {
    let started = Instant::now();

    if opts.inputs.is_empty() {
        return Err(TilerError::NoInputs);
    }
    for p in &opts.inputs {
        if !p.exists() {
            return Err(TilerError::InputMissing(p.clone()));
        }
    }

    // Total stages: 1 (mosaic) + 3 per grid (reproject, tile, pack) + 1 for an optional COG.
    let total_stages = 1 + (opts.grids.len() as u32) * 3 + u32::from(opts.cog);
    let mut stage_index = 0u32;
    let stage = |on_progress: &mut dyn FnMut(ProgressEvent),
                     idx: &mut u32,
                     name: &str,
                     grid: Option<TileGrid>| {
        *idx += 1;
        on_progress(ProgressEvent::Stage {
            stage: name.to_string(),
            index: *idx,
            total: total_stages,
            grid,
        });
    };

    let log = |on_progress: &mut dyn FnMut(ProgressEvent), msg: String| {
        on_progress(ProgressEvent::Log { message: msg });
    };

    // ---- inspect ------------------------------------------------------------
    let infos = inspect_many(gdal, &opts.inputs).await?;
    let combined_wgs84 = union_bounds_wgs84(&infos);
    let native_zoom = infos.iter().filter_map(|i| i.native_zoom).max();

    // ---- work dir -----------------------------------------------------------
    std::fs::create_dir_all(&opts.output_dir)?;
    let work_dir = opts.output_dir.join(format!(".tiler-work-{}", sanitize(&opts.name)));
    let _ = std::fs::remove_dir_all(&work_dir);
    std::fs::create_dir_all(&work_dir)?;

    // ---- stage 1: mosaic VRT with coverage mask -----------------------------
    stage(&mut on_progress, &mut stage_index, "mosaic", None);
    let mosaic_vrt = work_dir.join("mosaic.vrt");
    build_mosaic(gdal, &opts.inputs, &mosaic_vrt, &mut on_progress).await?;
    log(
        &mut on_progress,
        format!("Mosaicked {} input raster(s) with an alpha coverage mask", opts.inputs.len()),
    );

    // ---- per grid -----------------------------------------------------------
    let mut outputs = Vec::new();
    for grid in &opts.grids {
        let grid = *grid;

        // stage: reproject (warp the mosaic into the grid's CRS)
        stage(&mut on_progress, &mut stage_index, "reproject", Some(grid));
        let warped = work_dir.join(format!("warp_{}.vrt", grid.slug()));
        reproject(gdal, &mosaic_vrt, &warped, grid, opts, &mut on_progress).await?;

        // For a custom/raster-profile grid the pyramid is cut in the warped image's own
        // coordinate space, so zoom levels and grid params come from the warped raster.
        let warped_info = if grid.is_raster_profile() {
            Some(inspect_one(gdal, &warped).await?)
        } else {
            None
        };

        // Resolve zoom range. For a raster-profile (custom CRS) grid the pyramid is cut in
        // image space, so zoom is image-derived and the web-mercator-style user range does
        // NOT apply (passing z18 there would massively over-zoom). For web-mercator/geodetic
        // grids, honor the user's range (or auto from native resolution).
        let (max_zoom, min_zoom) = if let Some(info) = &warped_info {
            let max = image_native_zoom(info.width, info.height).min(24);
            (max, max.saturating_sub(8))
        } else {
            let max = opts.max_zoom.or(native_zoom).unwrap_or(18).min(24);
            (max, opts.min_zoom.unwrap_or_else(|| max.saturating_sub(8)))
        };
        if min_zoom > max_zoom {
            return Err(TilerError::BadZoomRange { min: min_zoom, max: max_zoom });
        }

        // For a custom projected grid, compute the tile-grid parameters a client (OpenLayers)
        // needs to display the tiles: origin, per-zoom resolutions, native bounds and proj4.
        let grid_params = if let Some(info) = &warped_info {
            let native_res = info.pixel_size[0].abs();
            let resolutions: Vec<f64> = (0..=max_zoom)
                .map(|z| native_res * 2f64.powi(i32::from(max_zoom - z)))
                .collect();
            Some(GridParams {
                epsg: grid.epsg(),
                proj4: proj4_for_epsg(gdal, grid.epsg()).await,
                tile_origin: [info.bounds_native.min_x, info.bounds_native.max_y],
                resolutions,
                tile_size: 256,
                bounds_crs: [
                    info.bounds_native.min_x,
                    info.bounds_native.min_y,
                    info.bounds_native.max_x,
                    info.bounds_native.max_y,
                ],
            })
        } else {
            None
        };

        // stage: tile
        stage(&mut on_progress, &mut stage_index, "tile", Some(grid));
        let tiles_dir = work_dir.join(format!("tiles_{}", grid.slug()));
        run_gdal2tiles(gdal, &warped, &tiles_dir, grid, min_zoom, max_zoom, opts, &mut on_progress)
            .await?;

        // count what actually got written (proves sparsity)
        let (per_zoom, dense_total) = count_tiles(&tiles_dir, opts.format.ext());
        let tiles_total: u64 = per_zoom.iter().map(|z| z.tiles).sum();
        log(
            &mut on_progress,
            format!(
                "[{}] {tiles_total} tiles written, {} empty tiles skipped ({:.0}% of the bounding box was empty)",
                grid.slug(),
                dense_total.saturating_sub(tiles_total),
                if dense_total > 0 {
                    100.0 * (dense_total.saturating_sub(tiles_total) as f64) / dense_total as f64
                } else {
                    0.0
                }
            ),
        );

        // stage: pack -> MBTiles
        stage(&mut on_progress, &mut stage_index, "pack", Some(grid));
        let source_id = format!("{}-{}", sanitize(&opts.name), grid.slug());
        let mbtiles_path = opts.output_dir.join(format!("{source_id}.mbtiles"));
        let _ = std::fs::remove_file(&mbtiles_path);
        pack(&tiles_dir, &mbtiles_path, TileScheme::Xyz, PackCompression::None).await?;
        write_metadata(
            &mbtiles_path,
            opts,
            grid,
            min_zoom,
            max_zoom,
            combined_wgs84,
            grid_params.as_ref(),
        )
        .await?;

        let file_size = std::fs::metadata(&mbtiles_path).map(|m| m.len()).unwrap_or(0);
        outputs.push(GridOutput {
            grid,
            mbtiles_path,
            source_id,
            min_zoom,
            max_zoom,
            bounds_wgs84: combined_wgs84,
            per_zoom,
            tiles_total,
            dense_total,
            empty_skipped: dense_total.saturating_sub(tiles_total),
            file_size,
            grid_params,
        });
    }

    // ---- optional single COG ------------------------------------------------
    let cog_output = if opts.cog {
        stage(&mut on_progress, &mut stage_index, "cog", None);
        let source_id = format!("{}-cog", sanitize(&opts.name));
        let cog_path = opts.output_dir.join(format!("{source_id}.tif"));
        let _ = std::fs::remove_file(&cog_path);
        build_cog(gdal, &mosaic_vrt, &cog_path, opts, &mut on_progress).await?;
        let info = inspect_one(gdal, &cog_path).await.ok();
        let bounds = info.as_ref().map_or(combined_wgs84, |i| i.bounds_wgs84);
        let file_size = std::fs::metadata(&cog_path).map(|m| m.len()).unwrap_or(0);
        let format = match opts.format {
            TileFormat::Webp => "webp",
            TileFormat::Png => "png",
        }
        .to_string();
        log(
            &mut on_progress,
            format!(
                "[cog] {} ({:.1} MB) — served directly at /{source_id}/{{z}}/{{x}}/{{y}}",
                cog_path.display(),
                file_size as f64 / 1_048_576.0
            ),
        );
        Some(CogOutput {
            cog_path,
            source_id,
            bounds_wgs84: bounds,
            max_zoom: info.as_ref().and_then(|i| i.native_zoom),
            format,
            file_size,
        })
    } else {
        None
    };

    if !opts.keep_intermediate {
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    let report = GenerateReport {
        name: opts.name.clone(),
        inputs: opts.inputs.clone(),
        outputs,
        cog_output,
        duration_secs: started.elapsed().as_secs_f64(),
    };
    on_progress(ProgressEvent::Done { report: report.clone() });
    Ok(report)
}

/// `gdalbuildvrt -addalpha` — combine inputs and add a transparency mask over empty areas.
async fn build_mosaic(
    gdal: &GdalEnv,
    inputs: &[PathBuf],
    out_vrt: &Path,
    on_progress: &mut impl FnMut(ProgressEvent),
) -> TilerResult<()> {
    let mut args = vec![
        "-overwrite".to_string(),
        "-addalpha".to_string(),
        out_vrt.to_string_lossy().into_owned(),
    ];
    for i in inputs {
        args.push(i.to_string_lossy().into_owned());
    }
    let tool = gdal.tool("gdalbuildvrt");
    gdal.run_streaming(&tool, &args, |line| emit_line(on_progress, "mosaic", line)).await
}

/// `gdalwarp` reproject into the grid's CRS (alpha preserved from the mosaic).
async fn reproject(
    gdal: &GdalEnv,
    mosaic_vrt: &Path,
    out_vrt: &Path,
    grid: TileGrid,
    opts: &GenerateOptions,
    on_progress: &mut impl FnMut(ProgressEvent),
) -> TilerResult<()> {
    let args = vec![
        "-overwrite".to_string(),
        "-of".to_string(),
        "VRT".to_string(),
        "-t_srs".to_string(),
        format!("EPSG:{}", grid.epsg()),
        "-r".to_string(),
        opts.resampling.gdal().to_string(),
        "-multi".to_string(),
        "-wo".to_string(),
        "NUM_THREADS=ALL_CPUS".to_string(),
        mosaic_vrt.to_string_lossy().into_owned(),
        out_vrt.to_string_lossy().into_owned(),
    ];
    let tool = gdal.tool("gdalwarp");
    gdal.run_streaming(&tool, &args, |line| emit_line(on_progress, "reproject", line)).await
}

/// `gdal2tiles --xyz` — cut the sparse pyramid; transparent tiles are not written.
async fn run_gdal2tiles(
    gdal: &GdalEnv,
    warped_vrt: &Path,
    tiles_dir: &Path,
    grid: TileGrid,
    min_zoom: u8,
    max_zoom: u8,
    opts: &GenerateOptions,
    on_progress: &mut impl FnMut(ProgressEvent),
) -> TilerResult<()> {
    let processes = opts
        .processes
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, std::num::NonZeroUsize::get));
    let args = vec![
        "--xyz".to_string(),
        "--profile".to_string(),
        grid.profile().to_string(),
        "-z".to_string(),
        format!("{min_zoom}-{max_zoom}"),
        "-r".to_string(),
        opts.resampling.gdal().to_string(),
        "--tiledriver".to_string(),
        opts.format.tiledriver().to_string(),
        "--processes".to_string(),
        processes.to_string(),
        "-w".to_string(),
        "none".to_string(),
        // EPSG:4326/geodetic otherwise emits `.kml` SuperOverlay files next to the tiles,
        // which `mbtiles pack` rejects (non-tile numeric-stemmed files). Suppress them.
        "--no-kml".to_string(),
        warped_vrt.to_string_lossy().into_owned(),
        tiles_dir.to_string_lossy().into_owned(),
    ];
    // Prefer `python -m osgeo_utils.gdal2tiles` (relocation-safe in a portable bundle).
    let (program, mut full_args) = gdal.gdal2tiles_invocation();
    full_args.extend(args);
    gdal.run_streaming(&program, &full_args, |line| {
        emit_line(on_progress, "tile", line);
    })
    .await
}

/// Map a resampling kernel to a GDAL COG `OVERVIEW_RESAMPLING` value.
fn cog_overview_resampling(r: Resampling) -> &'static str {
    match r {
        Resampling::Near => "NEAREST",
        Resampling::Bilinear => "BILINEAR",
        Resampling::Cubic => "CUBIC",
        Resampling::Average => "AVERAGE",
        Resampling::Lanczos => "LANCZOS",
    }
}

/// `gdal_translate -of COG` — merge the mosaic into a single Cloud-Optimized GeoTIFF in
/// EPSG:3857 aligned to the web-mercator tile grid (`TILING_SCHEME=GoogleMapsCompatible`),
/// with overviews and sparse blocks. Martin's COG source serves it directly at z/x/y, and
/// `SPARSE_OK=TRUE` keeps empty areas tile-free (HTTP 204, no blank images).
async fn build_cog(
    gdal: &GdalEnv,
    mosaic_vrt: &Path,
    out_cog: &Path,
    opts: &GenerateOptions,
    on_progress: &mut impl FnMut(ProgressEvent),
) -> TilerResult<()> {
    // COG can't store PNG internally; use lossless DEFLATE for "png", lossy WEBP for "webp".
    let compress = match opts.format {
        TileFormat::Webp => "WEBP",
        TileFormat::Png => "DEFLATE",
    };
    let mut args = vec![
        "-of".to_string(),
        "COG".to_string(),
        "-co".to_string(),
        "TILING_SCHEME=GoogleMapsCompatible".to_string(),
        "-co".to_string(),
        format!("COMPRESS={compress}"),
        "-co".to_string(),
        "SPARSE_OK=TRUE".to_string(),
        "-co".to_string(),
        format!("OVERVIEW_RESAMPLING={}", cog_overview_resampling(opts.resampling)),
    ];
    if compress == "WEBP" {
        args.push("-co".to_string());
        args.push("QUALITY=85".to_string());
    }
    args.push(mosaic_vrt.to_string_lossy().into_owned());
    args.push(out_cog.to_string_lossy().into_owned());
    let tool = gdal.tool("gdal_translate");
    gdal.run_streaming(&tool, &args, |line| emit_line(on_progress, "cog", line)).await
}

/// Emit a log line, and a percent event when one can be parsed.
fn emit_line(on_progress: &mut impl FnMut(ProgressEvent), stage: &str, line: &str) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Some(percent) = parse_progress_percent(trimmed) {
        on_progress(ProgressEvent::Percent { stage: stage.to_string(), percent });
    }
    on_progress(ProgressEvent::Log { message: format!("[{stage}] {trimmed}") });
}

/// Web Mercator-style native zoom for an image cut in raster (image) space:
/// the zoom at which the longest side fills one 256px tile per pixel.
fn image_native_zoom(width: u32, height: u32) -> u8 {
    let max_dim = f64::from(width.max(height).max(1));
    let z = (max_dim / 256.0).log2().ceil();
    if z.is_finite() && z >= 0.0 { (z as u8).min(24) } else { 0 }
}

/// Fetch the proj4 string for an EPSG code via `gdalsrsinfo` (best-effort).
async fn proj4_for_epsg(gdal: &GdalEnv, epsg: u32) -> Option<String> {
    let tool = gdal.tool("gdalsrsinfo");
    let args = vec![
        "-o".to_string(),
        "proj4".to_string(),
        "--single-line".to_string(),
        format!("EPSG:{epsg}"),
    ];
    let out = gdal.run_capture(&tool, &args).await.ok()?;
    let s = out.trim().trim_matches('\'').trim().to_string();
    if s.contains("+proj") { Some(s) } else { None }
}

/// Populate the MBTiles metadata table so Martin can build a correct TileJSON.
/// For a custom projected grid, also persist the tile-grid parameters (origin, resolutions,
/// native bounds, proj4) a non-Web-Mercator client needs.
#[allow(clippy::too_many_arguments)]
async fn write_metadata(
    mbtiles_path: &Path,
    opts: &GenerateOptions,
    grid: TileGrid,
    min_zoom: u8,
    max_zoom: u8,
    bounds: BBox,
    grid_params: Option<&GridParams>,
) -> TilerResult<()> {
    let mbt = Mbtiles::new(mbtiles_path)?;
    let mut conn = mbt.open().await?;
    let center_lon = (bounds.min_x + bounds.max_x) / 2.0;
    let center_lat = (bounds.min_y + bounds.max_y) / 2.0;
    let mut pairs: Vec<(&'static str, String)> = vec![
        ("name", opts.name.clone()),
        ("format", opts.format.ext().to_string()),
        ("type", "overlay".to_string()),
        ("version", "1.0".to_string()),
        ("description", format!("Generated by Martin Tile Map Studio in {}", grid.label())),
        ("minzoom", min_zoom.to_string()),
        ("maxzoom", max_zoom.to_string()),
        ("bounds", format!("{},{},{},{}", bounds.min_x, bounds.min_y, bounds.max_x, bounds.max_y)),
        ("center", format!("{center_lon},{center_lat},{min_zoom}")),
        ("crs", format!("EPSG:{}", grid.epsg())),
    ];

    // For a custom projected grid, persist what a custom-tile-grid client (OpenLayers) needs.
    if let Some(p) = grid_params {
        let resolutions = p.resolutions.iter().map(ToString::to_string).collect::<Vec<_>>().join(",");
        pairs.push(("scheme", "xyz".to_string()));
        pairs.push(("tile_size", p.tile_size.to_string()));
        pairs.push(("tile_origin", format!("{},{}", p.tile_origin[0], p.tile_origin[1])));
        pairs.push(("resolutions", resolutions));
        pairs.push((
            "bounds_crs",
            format!("{},{},{},{}", p.bounds_crs[0], p.bounds_crs[1], p.bounds_crs[2], p.bounds_crs[3]),
        ));
        if let Some(p4) = &p.proj4 {
            pairs.push(("crs_proj4", p4.clone()));
        }
    }

    for (k, v) in pairs {
        mbt.set_metadata_value(&mut conn, k, v).await?;
    }
    Ok(())
}

/// Union of all inputs' WGS84 footprints.
fn union_bounds_wgs84(infos: &[RasterInfo]) -> BBox {
    infos
        .iter()
        .map(|i| i.bounds_wgs84)
        .reduce(BBox::union)
        .unwrap_or(BBox::new(-180.0, -85.0, 180.0, 85.0))
}

/// Count `{z}/{x}/{y}.{ext}` tiles on disk, returning per-zoom tallies and the dense
/// (bounding-box) total the pyramid *would* have had if every tile existed.
fn count_tiles(tiles_dir: &Path, ext: &str) -> (Vec<ZoomTally>, u64) {
    let mut tallies = Vec::new();
    let mut dense_total = 0u64;

    let Ok(zoom_dirs) = std::fs::read_dir(tiles_dir) else {
        return (tallies, 0);
    };
    let mut zoom_levels: Vec<(u8, PathBuf)> = zoom_dirs
        .flatten()
        .filter_map(|e| {
            let z = e.file_name().to_str()?.parse::<u8>().ok()?;
            Some((z, e.path()))
        })
        .collect();
    zoom_levels.sort_by_key(|(z, _)| *z);

    for (z, zdir) in zoom_levels {
        let mut count = 0u64;
        let (mut min_x, mut max_x) = (u32::MAX, 0u32);
        let (mut min_y, mut max_y) = (u32::MAX, 0u32);
        if let Ok(xdirs) = std::fs::read_dir(&zdir) {
            for xentry in xdirs.flatten() {
                let Some(x) = xentry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
                    continue;
                };
                if let Ok(yfiles) = std::fs::read_dir(xentry.path()) {
                    for yentry in yfiles.flatten() {
                        let name = yentry.file_name();
                        let name = name.to_string_lossy();
                        let Some(stem) = name.strip_suffix(&format!(".{ext}")) else {
                            continue;
                        };
                        if stem.parse::<u32>().is_err() {
                            continue;
                        }
                        let y: u32 = stem.parse().unwrap_or(0);
                        count += 1;
                        min_x = min_x.min(x);
                        max_x = max_x.max(x);
                        min_y = min_y.min(y);
                        max_y = max_y.max(y);
                    }
                }
            }
        }
        if count > 0 {
            let dense = u64::from(max_x - min_x + 1) * u64::from(max_y - min_y + 1);
            dense_total += dense;
            tallies.push(ZoomTally { zoom: z, tiles: count });
        }
    }
    (tallies, dense_total)
}

/// Make a string safe to use as a file name / source id.
fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let trimmed = s.trim_matches('_').to_string();
    if trimmed.is_empty() { "tilemap".to_string() } else { trimmed }
}
