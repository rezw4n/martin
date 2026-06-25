//! # martin-tiler
//!
//! GDAL-backed tile-map generation engine for [Martin](https://martin.maplibre.org/).
//!
//! Turns **multiple GeoTIFF images (in any coordinate system) into a single,
//! integrated, _sparse_ tile map** — empty areas produce no tile images, only the
//! tiles that actually contain imagery are written. The output is a standard
//! `z/x/y` MBTiles archive that Martin serves directly.
//!
//! The engine orchestrates the proven GDAL command-line tools (`gdalbuildvrt`,
//! `gdalwarp`, `gdal2tiles`) for the heavy raster work, then packs the resulting
//! sparse pyramid with [`mbtiles::pack`]. The mandatory "no blank tiles" behaviour
//! comes from adding an alpha coverage mask during mosaicking (`gdalbuildvrt
//! -addalpha`): empty areas become transparent and `gdal2tiles` skips fully
//! transparent tiles.
//!
//! ## Pipeline
//! ```text
//! GeoTIFFs ─▶ gdalbuildvrt -addalpha ─▶ gdalwarp (reproject) ─▶ gdal2tiles --xyz ─▶ mbtiles::pack
//!            (mosaic + coverage mask)   (per output grid)        (sparse pyramid)    (sparse MBTiles)
//! ```

mod error;
mod gdal;
mod generate;
mod inspect;
mod model;
mod validate;

pub use error::{TilerError, TilerResult};
pub use gdal::GdalEnv;
pub use generate::generate;
pub use inspect::{inspect_many, inspect_one};
pub use model::{
    BBox, CheckStatus, EARTH_CIRCUMFERENCE, GenerateOptions, GenerateReport, GridOutput,
    ProgressEvent, RasterInfo, Resampling, TileFormat, TileGrid, ValidationCheck,
    ValidationReport, ZoomTally,
};
pub use validate::validate;
