//! `martin-tiler` — command-line front-end for the tile-map generation engine.
//!
//! ```text
//! martin-tiler inspect  data/*.tif
//! martin-tiler generate --name my_map --output ./out data/*.tif
//! martin-tiler validate ./out/my_map-webmercator.mbtiles
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use martin_tiler::{
    GdalEnv, GenerateOptions, Resampling, TileFormat, TileGrid, generate, inspect_many, validate,
};

#[derive(Parser)]
#[command(name = "martin-tiler", version, about = "Generate a single sparse z/x/y tile map from multiple GeoTIFFs")]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Emit machine-readable JSON instead of human-friendly text.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Inspect one or more GeoTIFFs (CRS, footprint, resolution, native zoom).
    Inspect {
        /// Input GeoTIFF files.
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
    },
    /// Generate a sparse tile map (MBTiles) from multiple GeoTIFFs.
    Generate {
        /// Input GeoTIFF files (mosaicked together).
        #[arg(required = true)]
        inputs: Vec<PathBuf>,
        /// Base name of the tile map.
        #[arg(long, short)]
        name: String,
        /// Output directory for the .mbtiles file(s).
        #[arg(long, short, default_value = ".")]
        output: PathBuf,
        /// Output grids (repeatable): web-mercator, geodetic.
        #[arg(long = "grid", value_enum, default_values_t = [GridArg::WebMercator])]
        grids: Vec<GridArg>,
        /// Additional custom projected output grids by EPSG code (repeatable), e.g. `--epsg 9680`.
        /// Tiles are cut in that projection's own space and require an OpenLayers-style custom
        /// tile grid to display (not Web Mercator).
        #[arg(long = "epsg")]
        epsg: Vec<u32>,
        /// Minimum zoom (default: auto).
        #[arg(long)]
        min_zoom: Option<u8>,
        /// Maximum zoom (default: native resolution).
        #[arg(long)]
        max_zoom: Option<u8>,
        /// Tile image format.
        #[arg(long, value_enum, default_value_t = FormatArg::Png)]
        format: FormatArg,
        /// Resampling kernel.
        #[arg(long, value_enum, default_value_t = ResamplingArg::Bilinear)]
        resampling: ResamplingArg,
        /// Parallel tiling processes (default: number of CPUs).
        #[arg(long)]
        processes: Option<usize>,
        /// Also produce a single Cloud-Optimized GeoTIFF (served directly by Martin at z/x/y).
        #[arg(long)]
        cog: bool,
        /// Keep intermediate VRT/XYZ files.
        #[arg(long)]
        keep_intermediate: bool,
    },
    /// Validate a generated MBTiles tile map.
    Validate {
        /// Path to the .mbtiles file.
        mbtiles: PathBuf,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum GridArg {
    WebMercator,
    Geodetic,
}
impl From<GridArg> for TileGrid {
    fn from(g: GridArg) -> Self {
        match g {
            GridArg::WebMercator => Self::WebMercator,
            GridArg::Geodetic => Self::Geodetic,
        }
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum FormatArg {
    Png,
    Webp,
}
impl From<FormatArg> for TileFormat {
    fn from(f: FormatArg) -> Self {
        match f {
            FormatArg::Png => Self::Png,
            FormatArg::Webp => Self::Webp,
        }
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ResamplingArg {
    Near,
    Bilinear,
    Cubic,
    Average,
    Lanczos,
}
impl From<ResamplingArg> for Resampling {
    fn from(r: ResamplingArg) -> Self {
        match r {
            ResamplingArg::Near => Self::Near,
            ResamplingArg::Bilinear => Self::Bilinear,
            ResamplingArg::Cubic => Self::Cubic,
            ResamplingArg::Average => Self::Average,
            ResamplingArg::Lanczos => Self::Lanczos,
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber_init();
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn tracing_subscriber_init() {
    // Best-effort; ignore if a global subscriber is already installed.
    let _ = tracing::subscriber::set_global_default(tracing::subscriber::NoSubscriber::default());
}

async fn run(cli: Cli) -> martin_tiler::TilerResult<()> {
    let gdal = GdalEnv::discover()?;

    match cli.command {
        Command::Inspect { inputs } => {
            let infos = inspect_many(&gdal, &inputs).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&infos)?);
            } else {
                for i in &infos {
                    println!("• {}", i.file_name);
                    println!("    driver     : {}", i.driver);
                    println!("    size       : {} x {} px, {} band(s){}", i.width, i.height, i.band_count, if i.has_alpha { " +alpha" } else { "" });
                    println!(
                        "    CRS        : {}{}",
                        i.crs_name.clone().unwrap_or_else(|| "?".into()),
                        i.epsg.map(|e| format!(" (EPSG:{e})")).unwrap_or_default()
                    );
                    println!("    resolution : {} m/px (native ~z{})", i.resolution_m.map(|r| format!("{r:.3}")).unwrap_or_else(|| "?".into()), i.native_zoom.map(|z| z.to_string()).unwrap_or_else(|| "?".into()));
                    println!(
                        "    wgs84 bbox : {:.5},{:.5} .. {:.5},{:.5}",
                        i.bounds_wgs84.min_x, i.bounds_wgs84.min_y, i.bounds_wgs84.max_x, i.bounds_wgs84.max_y
                    );
                    for n in &i.notes {
                        println!("    note       : {n}");
                    }
                }
            }
        }
        Command::Generate {
            inputs,
            name,
            output,
            grids,
            epsg,
            min_zoom,
            max_zoom,
            format,
            resampling,
            processes,
            cog,
            keep_intermediate,
        } => {
            let opts = GenerateOptions {
                inputs,
                output_dir: output,
                name,
                grids: grids
                    .into_iter()
                    .map(TileGrid::from)
                    .chain(epsg.into_iter().map(TileGrid::Custom))
                    .collect(),
                min_zoom,
                max_zoom,
                format: format.into(),
                resampling: resampling.into(),
                processes,
                cog,
                keep_intermediate,
            };
            let json = cli.json;
            let report = generate(&gdal, &opts, |ev| {
                if json {
                    if let Ok(s) = serde_json::to_string(&ev) {
                        println!("{s}");
                    }
                } else if let martin_tiler::ProgressEvent::Log { message } = &ev {
                    println!("{message}");
                } else if let martin_tiler::ProgressEvent::Stage { stage, index, total, grid } = &ev {
                    let g = grid.map(|g| format!(" [{}]", g.slug())).unwrap_or_default();
                    println!("──▶ stage {index}/{total}: {stage}{g}");
                }
            })
            .await?;

            if !json {
                println!("\n✔ Generated '{}' in {:.1}s", report.name, report.duration_secs);
                for o in &report.outputs {
                    println!(
                        "  {} → {}\n     z{}–z{}, {} tiles, {} empty skipped ({:.0}% sparse), {:.1} MB",
                        o.grid.label(),
                        o.mbtiles_path.display(),
                        o.min_zoom,
                        o.max_zoom,
                        o.tiles_total,
                        o.empty_skipped,
                        o.sparsity() * 100.0,
                        o.file_size as f64 / 1_048_576.0,
                    );
                }
                if let Some(cog) = &report.cog_output {
                    println!(
                        "  COG (single file) → {}\n     {:.1} MB, served directly at /{}/{{z}}/{{x}}/{{y}}",
                        cog.cog_path.display(),
                        cog.file_size as f64 / 1_048_576.0,
                        cog.source_id,
                    );
                }
            }
        }
        Command::Validate { mbtiles } => {
            let report = validate(&mbtiles).await?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Validation of {}", report.mbtiles_path.display());
                for c in &report.checks {
                    let mark = match c.status {
                        martin_tiler::CheckStatus::Pass => "✔",
                        martin_tiler::CheckStatus::Warn => "⚠",
                        martin_tiler::CheckStatus::Fail => "✘",
                    };
                    println!("  {mark} {:<20} {}", c.name, c.detail);
                }
                println!("\nResult: {}", if report.ok { "PASS" } else { "FAIL" });
            }
            if !report.ok {
                return Err(martin_tiler::TilerError::Other("validation failed".into()));
            }
        }
    }
    Ok(())
}
