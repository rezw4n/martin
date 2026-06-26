//! PostGIS vector-tile support: a managed bundled cluster, a connection registry,
//! table auto-discovery, and on-the-fly `ST_AsMVT` serving (any source SRID is
//! reprojected to Web Mercator at query time, so data always lines up).

pub mod config;
pub mod discover;
pub mod embed;
pub mod registry;

pub use config::{config_path_for, PgConfig, PgConnection};
pub use embed::{default_data_dir, default_root, PgEmbed};
pub use registry::{ConnState, PgRegistry, PgSource};
