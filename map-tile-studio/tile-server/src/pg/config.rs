//! Connection registry model — persisted as `connections.json` next to the maps
//! folder. Credentials live here (plain JSON, by design) so nothing is hardcoded:
//! users can add/edit external PostGIS servers and change the bundled password.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Loopback port the app's bundled PostgreSQL listens on.
pub const BUNDLED_PORT: u16 = 5433;
/// Default superuser for the bundled cluster (documented in the README).
pub const BUNDLED_USER: &str = "postgres";
/// Default database created inside the bundled cluster.
pub const BUNDLED_DB: &str = "gis";
/// Default password for the bundled cluster (loopback-only; documented + editable).
pub const BUNDLED_PASSWORD: &str = "mapstudio";
/// Stable id of the bundled connection.
pub const BUNDLED_ID: &str = "bundled";

fn default_true() -> bool {
    true
}
fn default_sslmode() -> String {
    "prefer".to_string()
}
fn default_refresh() -> u64 {
    30
}

/// A single PostgreSQL/PostGIS connection the app discovers + serves from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PgConnection {
    /// Stable, URL-safe id (`bundled`, or a user-chosen slug).
    pub id: String,
    /// Human label shown in the catalog.
    pub label: String,
    pub host: String,
    pub port: u16,
    pub dbname: String,
    pub user: String,
    #[serde(default)]
    pub password: String,
    /// libpq sslmode (`disable` | `prefer` | `require` | …).
    #[serde(default = "default_sslmode")]
    pub sslmode: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// True only for the app's managed, embedded cluster.
    #[serde(default)]
    pub bundled: bool,
}

impl PgConnection {
    /// The built-in embedded PostGIS connection.
    #[must_use]
    pub fn bundled_default() -> Self {
        Self {
            id: BUNDLED_ID.to_string(),
            label: "Bundled PostGIS".to_string(),
            host: "127.0.0.1".to_string(),
            port: BUNDLED_PORT,
            dbname: BUNDLED_DB.to_string(),
            user: BUNDLED_USER.to_string(),
            password: BUNDLED_PASSWORD.to_string(),
            sslmode: "disable".to_string(),
            enabled: true,
            bundled: true,
        }
    }

    /// libpq-style keyword/value connection string (consumed by `tokio-postgres`).
    #[must_use]
    pub fn conn_string(&self) -> String {
        // Values come from the catalog UI; wrap each in single quotes and escape
        // embedded quotes/backslashes so spaces or symbols in a password are safe.
        fn q(v: &str) -> String {
            format!("'{}'", v.replace('\\', "\\\\").replace('\'', "\\'"))
        }
        format!(
            "host={} port={} dbname={} user={} password={} sslmode={}",
            q(&self.host),
            self.port,
            q(&self.dbname),
            q(&self.user),
            q(&self.password),
            q(&self.sslmode),
        )
    }

    /// Connection string against the cluster's default `postgres` db (for bootstrap).
    #[must_use]
    pub fn maintenance_conn_string(&self) -> String {
        let mut c = self.clone();
        c.dbname = "postgres".to_string();
        c.conn_string()
    }
}

/// Whole registry: a refresh cadence plus the list of connections.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PgConfig {
    /// How often (seconds) to re-discover tables. 0 disables auto-refresh.
    #[serde(default = "default_refresh")]
    pub refresh_seconds: u64,
    #[serde(default)]
    pub connections: Vec<PgConnection>,
}

impl Default for PgConfig {
    fn default() -> Self {
        Self {
            refresh_seconds: default_refresh(),
            connections: vec![PgConnection::bundled_default()],
        }
    }
}

impl PgConfig {
    /// Load from disk, falling back to a default (bundled-only) config.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<Self>(&s).ok())
            .unwrap_or_default()
            .with_bundled()
    }

    /// Persist to disk (pretty JSON), creating the parent directory.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        std::fs::write(path, json)
    }

    /// Guarantee a bundled connection is present (so the embedded DB is always served).
    #[must_use]
    pub fn with_bundled(mut self) -> Self {
        if !self.connections.iter().any(|c| c.bundled) {
            self.connections.insert(0, PgConnection::bundled_default());
        }
        self
    }

    /// The bundled connection, if any.
    #[must_use]
    pub fn bundled(&self) -> Option<&PgConnection> {
        self.connections.iter().find(|c| c.bundled)
    }
}

/// Standard location of `connections.json` given the maps directory.
#[must_use]
pub fn config_path_for(maps_dir: &Path) -> PathBuf {
    maps_dir
        .parent()
        .unwrap_or(maps_dir)
        .join("connections.json")
}
