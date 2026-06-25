//! Error type for the tiler engine.

use std::path::PathBuf;

/// Convenience result alias.
pub type TilerResult<T> = Result<T, TilerError>;

/// Anything that can go wrong while inspecting, generating or validating a tile map.
#[derive(Debug, thiserror::Error)]
pub enum TilerError {
    /// The GDAL toolchain could not be located on this machine.
    #[error(
        "GDAL was not found. Install GDAL (e.g. OSGeo4W on Windows) or set MARTIN_GDAL_PREFIX/MARTIN_GDAL_BIN. Tried: {0}"
    )]
    GdalNotFound(String),

    /// A required GDAL tool (e.g. `gdal2tiles`) was missing from an otherwise valid install.
    #[error("required GDAL tool `{0}` was not found in the GDAL installation at {1}")]
    GdalToolMissing(String, PathBuf),

    /// An external command exited with a non-zero status.
    #[error("command `{cmd}` failed with status {status}:\n{stderr}")]
    CommandFailed {
        cmd: String,
        status: String,
        stderr: String,
    },

    /// No input files were supplied.
    #[error("no input rasters were supplied")]
    NoInputs,

    /// A supplied input path does not exist.
    #[error("input raster does not exist: {0}")]
    InputMissing(PathBuf),

    /// `gdalinfo` produced output we could not understand.
    #[error("could not parse gdalinfo output for {path}: {reason}")]
    InspectParse { path: PathBuf, reason: String },

    /// A computed zoom range was empty or invalid.
    #[error("invalid zoom range: min_zoom ({min}) must be <= max_zoom ({max})")]
    BadZoomRange { min: u8, max: u8 },

    /// Wrapper around an I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Wrapper around a JSON (de)serialisation failure.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// Wrapper around an error from the `mbtiles` crate (packing / metadata / validation).
    #[error("mbtiles error: {0}")]
    Mbtiles(#[from] mbtiles::MbtError),

    /// A catch-all for engine-level problems with a descriptive message.
    #[error("{0}")]
    Other(String),
}
