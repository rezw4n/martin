//! Inspecting source GeoTIFFs with `gdalinfo -json`.

use std::path::Path;

use serde_json::Value;

use crate::error::{TilerError, TilerResult};
use crate::gdal::GdalEnv;
use crate::model::{BBox, EARTH_CIRCUMFERENCE, RasterInfo};

/// Inspect a batch of rasters, returning one [`RasterInfo`] per input.
pub async fn inspect_many(gdal: &GdalEnv, paths: &[std::path::PathBuf]) -> TilerResult<Vec<RasterInfo>> {
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        out.push(inspect_one(gdal, p).await?);
    }
    Ok(out)
}

/// Inspect a single raster.
pub async fn inspect_one(gdal: &GdalEnv, path: &Path) -> TilerResult<RasterInfo> {
    if !path.exists() {
        return Err(TilerError::InputMissing(path.to_path_buf()));
    }
    let tool = gdal.tool("gdalinfo");
    let args = vec![
        "-json".to_string(),
        "-nomd".to_string(),
        path.to_string_lossy().into_owned(),
    ];
    let stdout = gdal.run_capture(&tool, &args).await?;
    let json: Value = serde_json::from_str(&stdout)?;
    parse_gdalinfo(path, &json)
}

/// Pull a `RasterInfo` out of parsed `gdalinfo -json` output.
fn parse_gdalinfo(path: &Path, json: &Value) -> TilerResult<RasterInfo> {
    let bad = |reason: &str| TilerError::InspectParse {
        path: path.to_path_buf(),
        reason: reason.to_string(),
    };

    let size = json.get("size").and_then(|v| v.as_array());
    let (width, height) = match size {
        Some(a) if a.len() == 2 => (
            a[0].as_u64().unwrap_or(0) as u32,
            a[1].as_u64().unwrap_or(0) as u32,
        ),
        _ => return Err(bad("missing `size`")),
    };

    let driver = json
        .get("driverShortName")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();

    let bands = json
        .get("bands")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let band_count = bands.len() as u32;
    let has_alpha = bands.iter().any(|b| {
        b.get("colorInterpretation")
            .and_then(Value::as_str)
            .is_some_and(|c| c.eq_ignore_ascii_case("Alpha"))
    });
    let has_overviews = bands.iter().any(|b| {
        b.get("overviews")
            .and_then(Value::as_array)
            .is_some_and(|o| !o.is_empty())
    });
    // Striped TIFFs have a block as wide as the whole image; tiled ones use sub-width blocks.
    let block_x = bands
        .first()
        .and_then(|b| b.get("block"))
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_u64)
        .unwrap_or(u64::from(width)) as u32;
    let is_tiled = block_x < width;

    let geo_transform: Vec<f64> = json
        .get("geoTransform")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_default();
    let pixel_size = if geo_transform.len() == 6 {
        [geo_transform[1], geo_transform[5]]
    } else {
        [0.0, 0.0]
    };

    let wkt = json
        .get("coordinateSystem")
        .and_then(|c| c.get("wkt"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let epsg = parse_epsg(wkt);
    let crs_name = parse_crs_name(wkt);
    let is_geographic = !wkt.contains("PROJCRS") && !wkt.contains("PROJCS") && !wkt.is_empty();

    // Native bounds from the corner coordinates.
    let bounds_native = corner_bbox(json).ok_or_else(|| bad("missing `cornerCoordinates`"))?;

    // WGS84 footprint from `wgs84Extent` (GeoJSON), falling back to native if already geographic.
    let bounds_wgs84 = wgs84_bbox(json).unwrap_or(bounds_native);

    let center_lat = (bounds_wgs84.min_y + bounds_wgs84.max_y) / 2.0;
    let resolution_m = ground_resolution_m(pixel_size[0].abs(), is_geographic, center_lat);
    let native_zoom = resolution_m.and_then(web_mercator_zoom);

    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);

    let mut notes = Vec::new();
    if !is_tiled {
        notes.push(
            "Striped GeoTIFF (not internally tiled). Fine for generation; Martin's on-the-fly COG \
             reader would reject it."
                .to_string(),
        );
    }
    if !has_overviews {
        notes.push("No internal overviews/pyramids; lower zooms are built during tiling.".to_string());
    }
    if epsg.is_none() {
        notes.push("Could not determine an EPSG code; reprojection relies on the embedded WKT.".to_string());
    }

    Ok(RasterInfo {
        path: path.to_path_buf(),
        file_name: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        driver,
        width,
        height,
        band_count,
        has_alpha,
        crs_name,
        epsg,
        pixel_size,
        bounds_native,
        bounds_wgs84,
        native_zoom,
        resolution_m,
        file_size,
        is_tiled,
        has_overviews,
        notes,
    })
}

/// Last `ID["EPSG",N]` / `AUTHORITY["EPSG","N"]` in a WKT string is the CRS code.
fn parse_epsg(wkt: &str) -> Option<u32> {
    let re = regex::Regex::new(r#"(?:ID|AUTHORITY)\s*\[\s*"EPSG"\s*,\s*"?(\d+)"?\s*\]"#).ok()?;
    re.captures_iter(wkt)
        .filter_map(|c| c.get(1))
        .filter_map(|m| m.as_str().parse::<u32>().ok())
        .last()
}

/// First quoted name after a `*CRS[`/`*CS[` keyword.
fn parse_crs_name(wkt: &str) -> Option<String> {
    let re = regex::Regex::new(r#"(?:PROJCRS|GEOGCRS|PROJCS|GEOGCS)\s*\[\s*"([^"]+)""#).ok()?;
    re.captures(wkt)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Bounding box from `cornerCoordinates` (native CRS).
fn corner_bbox(json: &Value) -> Option<BBox> {
    let cc = json.get("cornerCoordinates")?;
    let pt = |k: &str| -> Option<(f64, f64)> {
        let a = cc.get(k)?.as_array()?;
        Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
    };
    let corners = ["upperLeft", "lowerLeft", "upperRight", "lowerRight"];
    let mut bbox: Option<BBox> = None;
    for c in corners {
        if let Some((x, y)) = pt(c) {
            let b = BBox::new(x, y, x, y);
            bbox = Some(bbox.map_or(b, |acc| acc.union(b)));
        }
    }
    bbox
}

/// Bounding box from the GeoJSON `wgs84Extent` polygon.
fn wgs84_bbox(json: &Value) -> Option<BBox> {
    let coords = json.get("wgs84Extent")?.get("coordinates")?.as_array()?;
    let mut bbox: Option<BBox> = None;
    // Walk the (possibly nested) coordinate arrays collecting [lon, lat] pairs.
    fn walk(v: &Value, bbox: &mut Option<BBox>) {
        if let Some(arr) = v.as_array() {
            if arr.len() == 2 && arr[0].is_number() && arr[1].is_number() {
                let lon = arr[0].as_f64().unwrap_or_default();
                let lat = arr[1].as_f64().unwrap_or_default();
                let b = BBox::new(lon, lat, lon, lat);
                *bbox = Some(bbox.map_or(b, |acc| acc.union(b)));
            } else {
                for item in arr {
                    walk(item, bbox);
                }
            }
        }
    }
    walk(&Value::Array(coords.clone()), &mut bbox);
    bbox
}

/// Convert a pixel size into approximate ground meters/pixel.
fn ground_resolution_m(pixel_x: f64, is_geographic: bool, center_lat: f64) -> Option<f64> {
    if pixel_x <= 0.0 {
        return None;
    }
    if is_geographic {
        // pixel size is in degrees of longitude
        Some(pixel_x * 111_320.0 * center_lat.to_radians().cos().abs().max(0.01))
    } else {
        Some(pixel_x)
    }
}

/// Nearest Web Mercator zoom level (256px tiles) for a given ground resolution.
fn web_mercator_zoom(resolution_m: f64) -> Option<u8> {
    if resolution_m <= 0.0 {
        return None;
    }
    // resolution at zoom z = EARTH_CIRCUMFERENCE / (256 * 2^z)
    let z = (EARTH_CIRCUMFERENCE / (256.0 * resolution_m)).log2().round();
    if z.is_finite() && (0.0..=24.0).contains(&z) {
        Some(z as u8)
    } else if z > 24.0 {
        Some(24)
    } else {
        Some(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_epsg_from_wkt() {
        let wkt = r#"PROJCRS["WGS 84 / TM 90 NE",BASEGEOGCRS["WGS 84",ID["EPSG",4326]],ID["EPSG",9680]]"#;
        // The outermost (last) authority code is the CRS itself, not the base datum.
        assert_eq!(parse_epsg(wkt), Some(9680));
    }

    #[test]
    fn parses_crs_name() {
        let wkt = r#"PROJCRS["WGS 84 / TM 90 NE",BASEGEOGCRS["WGS 84"]]"#;
        assert_eq!(parse_crs_name(wkt).as_deref(), Some("WGS 84 / TM 90 NE"));
    }

    #[test]
    fn native_zoom_for_10cm_imagery_is_z21() {
        // 0.1 m/px high-resolution aerial imagery sits around zoom 21.
        assert_eq!(web_mercator_zoom(0.1), Some(21));
        // ~156 km/px is the whole world in one z0 tile.
        assert_eq!(web_mercator_zoom(EARTH_CIRCUMFERENCE / 256.0), Some(0));
    }

    #[test]
    fn geographic_resolution_converts_degrees_to_meters() {
        // ~1 degree at the equator is ~111 km.
        let r = ground_resolution_m(1.0, true, 0.0).unwrap();
        assert!((r - 111_320.0).abs() < 1.0);
    }
}
