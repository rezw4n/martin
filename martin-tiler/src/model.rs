//! Public data model shared between the engine, the CLI binary, and the HTTP API.
//!
//! Every type here is `serde`-serialisable so it can cross the HTTP boundary to the
//! Tile Map Studio frontend unchanged.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Circumference of the Earth at the equator in meters (Web Mercator / EPSG:3857).
pub const EARTH_CIRCUMFERENCE: f64 = 40_075_016.685_578_49;

/// Derive `schemars::JsonSchema` only when the feature is on, so the core engine
/// stays dependency-light while the server can still expose typed schemas.
macro_rules! model {
    ($(#[$m:meta])* $vis:vis enum $name:ident { $($body:tt)* }) => {
        // Base derives must come first so the `serde`/`default` helper attributes the
        // caller adds in `$(#[$m])*` are introduced before they are used.
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "unstable-schemas", derive(schemars::JsonSchema))]
        $(#[$m])*
        $vis enum $name { $($body)* }
    };
    ($(#[$m:meta])* $vis:vis struct $name:ident { $($body:tt)* }) => {
        #[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
        #[cfg_attr(feature = "unstable-schemas", derive(schemars::JsonSchema))]
        $(#[$m])*
        $vis struct $name { $($body)* }
    };
}

model! {
    /// Output tiling grid (the coordinate reference system the pyramid is cut in).
    ///
    /// The client requested "multiple coordinate systems": a map can be generated in
    /// one or more of these grids simultaneously, each producing its own sparse MBTiles.
    #[derive(Copy, Eq, Default)]
    #[serde(rename_all = "kebab-case")]
    pub enum TileGrid {
        /// EPSG:3857 "Web Mercator" — the universal z/x/y web-map grid (MapLibre, Leaflet, Google, OSM).
        #[default]
        WebMercator,
        /// EPSG:4326 geographic "geodetic" grid (WGS84 lon/lat), 2 tiles wide at z0.
        Geodetic,
        /// Tile in an arbitrary projected CRS (e.g. EPSG:9680). Cut with `gdal2tiles
        /// --profile=raster` in the warped image's own coordinate space. These tiles are
        /// NOT Web Mercator and need a client that supports a custom tile grid (e.g. OpenLayers).
        Custom(u32),
    }
}

impl TileGrid {
    /// EPSG code of the grid's coordinate reference system.
    #[must_use]
    pub fn epsg(self) -> u32 {
        match self {
            Self::WebMercator => 3857,
            Self::Geodetic => 4326,
            Self::Custom(e) => e,
        }
    }

    /// `gdal2tiles` `--profile` value. Built-in grids use mercator/geodetic; any other
    /// CRS must be cut in raster (image) space after warping to that CRS.
    #[must_use]
    pub fn profile(self) -> &'static str {
        match self {
            Self::WebMercator => "mercator",
            Self::Geodetic => "geodetic",
            Self::Custom(_) => "raster",
        }
    }

    /// True when the pyramid is cut in image space (a projected CRS with no global
    /// georeferenced web grid), so zoom levels are image-derived and the output needs a
    /// custom-tile-grid client.
    #[must_use]
    pub fn is_raster_profile(self) -> bool {
        matches!(self, Self::Custom(_))
    }

    /// Short stable identifier used in file names and source ids.
    #[must_use]
    pub fn slug(self) -> String {
        match self {
            Self::WebMercator => "webmercator".into(),
            Self::Geodetic => "wgs84".into(),
            Self::Custom(e) => format!("epsg{e}"),
        }
    }

    /// Human-friendly label for the UI.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::WebMercator => "Web Mercator (EPSG:3857)".into(),
            Self::Geodetic => "WGS84 geographic (EPSG:4326)".into(),
            Self::Custom(e) => format!("Custom projected grid (EPSG:{e})"),
        }
    }
}

model! {
    /// Image codec used for the generated tiles. Both keep an alpha channel so empty
    /// areas stay transparent (and therefore skippable) — required for the sparse output.
    #[derive(Copy, Eq, Default)]
    #[serde(rename_all = "lowercase")]
    pub enum TileFormat {
        /// Lossless PNG (default). Largest, perfectly preserves imagery.
        #[default]
        Png,
        /// WebP with alpha. ~2-4x smaller than PNG at high visual quality.
        Webp,
    }
}

impl TileFormat {
    /// File extension (no dot).
    #[must_use]
    pub fn ext(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Webp => "webp",
        }
    }

    /// `gdal2tiles` `--tiledriver` value.
    #[must_use]
    pub fn tiledriver(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Webp => "WEBP",
        }
    }
}

model! {
    /// Resampling kernel used when reprojecting and building lower-zoom overviews.
    #[derive(Copy, Eq, Default)]
    #[serde(rename_all = "lowercase")]
    pub enum Resampling {
        /// Nearest neighbour — fastest, best for categorical/label rasters.
        Near,
        /// Bilinear — good default for continuous imagery.
        #[default]
        Bilinear,
        /// Cubic convolution — smoother, slower.
        Cubic,
        /// Average — good for downsampling photography.
        Average,
        /// Lanczos — sharpest downsampling, slowest.
        Lanczos,
    }
}

impl Resampling {
    /// GDAL `-r` value (shared by `gdalwarp` and `gdal2tiles`).
    #[must_use]
    pub fn gdal(self) -> &'static str {
        match self {
            Self::Near => "near",
            Self::Bilinear => "bilinear",
            Self::Cubic => "cubic",
            Self::Average => "average",
            Self::Lanczos => "lanczos",
        }
    }
}

model! {
    /// An axis-aligned bounding box `[min_x, min_y, max_x, max_y]`.
    #[derive(Copy)]
    pub struct BBox {
        pub min_x: f64,
        pub min_y: f64,
        pub max_x: f64,
        pub max_y: f64,
    }
}

impl BBox {
    /// Build a bbox, normalising swapped corners.
    #[must_use]
    pub fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Self {
        Self {
            min_x: min_x.min(max_x),
            min_y: min_y.min(max_y),
            max_x: min_x.max(max_x),
            max_y: min_y.max(max_y),
        }
    }

    /// Union of two boxes (covering both).
    #[must_use]
    pub fn union(self, other: Self) -> Self {
        Self {
            min_x: self.min_x.min(other.min_x),
            min_y: self.min_y.min(other.min_y),
            max_x: self.max_x.max(other.max_x),
            max_y: self.max_y.max(other.max_y),
        }
    }

    /// `[min_x, min_y, max_x, max_y]` as a plain array (TileJSON / MBTiles `bounds` order).
    #[must_use]
    pub fn to_array(self) -> [f64; 4] {
        [self.min_x, self.min_y, self.max_x, self.max_y]
    }
}

model! {
    /// Everything we learned by inspecting a single source GeoTIFF.
    pub struct RasterInfo {
        /// Absolute path of the source file.
        pub path: PathBuf,
        /// Just the file name, for display.
        pub file_name: String,
        /// GDAL driver short name (e.g. `GTiff`).
        pub driver: String,
        /// Pixel width.
        pub width: u32,
        /// Pixel height.
        pub height: u32,
        /// Number of raster bands.
        pub band_count: u32,
        /// True if one of the bands is an alpha channel.
        pub has_alpha: bool,
        /// Human-readable CRS name (e.g. `WGS 84 / TM 90 NE`).
        pub crs_name: Option<String>,
        /// EPSG code of the source CRS, if it could be determined.
        pub epsg: Option<u32>,
        /// Pixel size in the source CRS units `(x, y)` (y is usually negative).
        pub pixel_size: [f64; 2],
        /// Footprint in the source CRS.
        pub bounds_native: BBox,
        /// Footprint in WGS84 lon/lat (for drawing on a web map).
        pub bounds_wgs84: BBox,
        /// Approximate Web Mercator zoom level that matches the native resolution.
        pub native_zoom: Option<u8>,
        /// Approximate ground resolution in meters/pixel.
        pub resolution_m: Option<f64>,
        /// Size of the file on disk in bytes.
        pub file_size: u64,
        /// True if the TIFF is internally tiled (vs striped).
        pub is_tiled: bool,
        /// True if the TIFF carries internal overviews/pyramids.
        pub has_overviews: bool,
        /// Non-fatal notes/warnings about this raster.
        pub notes: Vec<String>,
    }
}

model! {
    /// Options for a single generation run.
    pub struct GenerateOptions {
        /// Input GeoTIFF files (one or more). They are mosaicked together.
        pub inputs: Vec<PathBuf>,
        /// Directory the output MBTiles (and intermediate work dir) are written into.
        pub output_dir: PathBuf,
        /// Base name of the tile map (used for file names and the served source id).
        pub name: String,
        /// One or more output grids; each produces its own MBTiles.
        #[serde(default = "default_grids")]
        pub grids: Vec<TileGrid>,
        /// Minimum zoom (inclusive). `None` = auto (derived so the whole map fits a few tiles).
        #[serde(default)]
        pub min_zoom: Option<u8>,
        /// Maximum zoom (inclusive). `None` = auto (native resolution of the imagery).
        #[serde(default)]
        pub max_zoom: Option<u8>,
        /// Tile image codec.
        #[serde(default)]
        pub format: TileFormat,
        /// Resampling kernel.
        #[serde(default)]
        pub resampling: Resampling,
        /// Parallel worker processes for tiling. `None` = number of CPUs.
        #[serde(default)]
        pub processes: Option<usize>,
        /// Also produce a single Cloud-Optimized GeoTIFF (EPSG:3857, GoogleMapsCompatible)
        /// that Martin serves directly at z/x/y — no MBTiles needed.
        #[serde(default)]
        pub cog: bool,
        /// Keep the intermediate VRT/XYZ working files for debugging.
        #[serde(default)]
        pub keep_intermediate: bool,
    }
}

fn default_grids() -> Vec<TileGrid> {
    vec![TileGrid::WebMercator]
}

model! {
    /// Per-zoom tile tally for one generated grid.
    pub struct ZoomTally {
        pub zoom: u8,
        pub tiles: u64,
    }
}

model! {
    /// Tile-grid parameters for a custom projected (non-Web-Mercator) output, so a client
    /// such as OpenLayers can be configured to display the tiles correctly.
    pub struct GridParams {
        /// EPSG code of the projection.
        pub epsg: u32,
        /// proj4 definition string (for `proj4.defs`), if available.
        pub proj4: Option<String>,
        /// Tile grid origin (top-left) `[x, y]` in the projection's units.
        pub tile_origin: [f64; 2],
        /// Per-zoom resolutions (units/pixel), indexed z0..=maxzoom.
        pub resolutions: Vec<f64>,
        /// Tile size in pixels.
        pub tile_size: u32,
        /// Data bounds `[min_x, min_y, max_x, max_y]` in the projection's units.
        pub bounds_crs: [f64; 4],
    }
}

model! {
    /// The MBTiles produced for one grid, plus statistics that prove the sparse behaviour.
    pub struct GridOutput {
        pub grid: TileGrid,
        /// Path of the produced `.mbtiles` file.
        pub mbtiles_path: PathBuf,
        /// Source id Martin will serve it under (`{name}-{grid-slug}`).
        pub source_id: String,
        pub min_zoom: u8,
        pub max_zoom: u8,
        /// Footprint in WGS84 lon/lat.
        pub bounds_wgs84: BBox,
        /// Tiles actually stored, per zoom (only non-empty tiles exist).
        pub per_zoom: Vec<ZoomTally>,
        /// Total non-empty tiles stored.
        pub tiles_total: u64,
        /// Tiles a *dense* (non-sparse) pyramid would have needed over the same bbox.
        pub dense_total: u64,
        /// Empty tiles that were skipped (`dense_total - tiles_total`) — the storage saved.
        pub empty_skipped: u64,
        /// Final file size in bytes.
        pub file_size: u64,
        /// Custom-grid parameters (only for a non-Web-Mercator projected grid).
        #[serde(default)]
        pub grid_params: Option<GridParams>,
    }
}

impl GridOutput {
    /// Fraction of the bounding box that was empty and therefore *not* written (0.0–1.0).
    #[must_use]
    pub fn sparsity(&self) -> f64 {
        if self.dense_total == 0 {
            0.0
        } else {
            self.empty_skipped as f64 / self.dense_total as f64
        }
    }
}

model! {
    /// A single Cloud-Optimized GeoTIFF output that Martin serves directly at z/x/y.
    pub struct CogOutput {
        /// Path of the produced `.tif` COG.
        pub cog_path: PathBuf,
        /// Source id Martin serves it under (the file stem).
        pub source_id: String,
        /// Footprint in WGS84 lon/lat.
        pub bounds_wgs84: BBox,
        /// Approximate max (native) Web Mercator zoom.
        pub max_zoom: Option<u8>,
        /// Tile image format Martin will serve (webp / png).
        pub format: String,
        /// Final file size in bytes.
        pub file_size: u64,
    }
}

model! {
    /// Final result of a successful generation run.
    pub struct GenerateReport {
        pub name: String,
        pub inputs: Vec<PathBuf>,
        pub outputs: Vec<GridOutput>,
        /// Single-file COG output, if requested.
        #[serde(default)]
        pub cog_output: Option<CogOutput>,
        pub duration_secs: f64,
    }
}

model! {
    /// Status of a single validation check.
    #[derive(Copy, Eq)]
    #[serde(rename_all = "lowercase")]
    pub enum CheckStatus {
        Pass,
        Warn,
        Fail,
    }
}

model! {
    /// One line item in a validation report.
    pub struct ValidationCheck {
        pub name: String,
        pub status: CheckStatus,
        pub detail: String,
    }
}

model! {
    /// Output of validating a generated MBTiles.
    pub struct ValidationReport {
        pub mbtiles_path: PathBuf,
        /// Overall pass: true only if no check failed.
        pub ok: bool,
        pub checks: Vec<ValidationCheck>,
        pub tiles_total: u64,
        pub min_zoom: Option<u8>,
        pub max_zoom: Option<u8>,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_union_covers_both() {
        let a = BBox::new(0.0, 0.0, 1.0, 1.0);
        let b = BBox::new(2.0, -1.0, 3.0, 0.5);
        let u = a.union(b);
        assert_eq!(u.to_array(), [0.0, -1.0, 3.0, 1.0]);
    }

    #[test]
    fn bbox_normalises_swapped_corners() {
        let b = BBox::new(3.0, 4.0, 1.0, 2.0);
        assert_eq!(b.to_array(), [1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn grid_metadata_is_stable() {
        assert_eq!(TileGrid::WebMercator.epsg(), 3857);
        assert_eq!(TileGrid::WebMercator.profile(), "mercator");
        assert_eq!(TileGrid::Geodetic.epsg(), 4326);
        assert_eq!(TileGrid::Geodetic.profile(), "geodetic");
    }

    #[test]
    fn sparsity_is_fraction_skipped() {
        let o = GridOutput {
            grid: TileGrid::WebMercator,
            mbtiles_path: std::path::PathBuf::new(),
            source_id: "x".into(),
            min_zoom: 0,
            max_zoom: 1,
            bounds_wgs84: BBox::new(0.0, 0.0, 1.0, 1.0),
            per_zoom: vec![],
            tiles_total: 60,
            dense_total: 100,
            empty_skipped: 40,
            file_size: 0,
            grid_params: None,
        };
        assert!((o.sparsity() - 0.4).abs() < 1e-9);
    }
}

/// A progress event streamed during generation, suitable for a live UI / SSE feed.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "unstable-schemas", derive(schemars::JsonSchema))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ProgressEvent {
    /// A new pipeline stage started.
    Stage {
        /// Stage name, e.g. `mosaic`, `reproject`, `tile`, `pack`, `validate`.
        stage: String,
        /// 1-based index of this stage.
        index: u32,
        /// Total number of stages in the run.
        total: u32,
        /// Which grid this stage is working on, if grid-specific.
        grid: Option<TileGrid>,
    },
    /// Percentage progress within the current stage (0–100).
    Percent { stage: String, percent: f64 },
    /// A free-form human-readable log line from the engine or an underlying tool.
    Log { message: String },
    /// The run finished successfully.
    Done { report: GenerateReport },
    /// The run failed.
    Failed { error: String },
}
