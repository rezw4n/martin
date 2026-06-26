//! The live PostGIS registry: owns the connection pools, the discovered sources,
//! and a background thread that periodically re-discovers tables. Shared (`Arc`)
//! across the tile server's worker threads, which call [`PgRegistry::get_tile`].
//!
//! All async work (connecting, discovery, per-tile MVT queries) runs on one
//! dedicated multi-threaded Tokio runtime; worker threads `block_on` it.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use martin_core::tiles::postgres::PostgresPool;
use tokio::runtime::Runtime;

use super::config::PgConfig;
use super::discover::{discover_tables, query_mvt};
use super::embed::PgEmbed;

/// World bounds in WGS84, used when a table's extent can't be determined.
const WORLD: [f64; 4] = [-180.0, -85.051_129, 180.0, 85.051_129];
const DEFAULT_MINZOOM: u8 = 0;
const DEFAULT_MAXZOOM: u8 = 18;

/// A servable PostGIS vector source (exactly one table).
#[derive(Clone, Debug)]
pub struct PgSource {
    /// URL-safe slug used as `{source}` in the tile path.
    pub id: String,
    pub conn_id: String,
    pub conn_label: String,
    pub schema: String,
    pub table: String,
    pub geom_type: String,
    pub srid: i32,
    /// `(name, pg_type)` of every emitted feature property.
    pub fields: Vec<(String, String)>,
    pub minzoom: u8,
    pub maxzoom: u8,
    pub bounds: [f64; 4],
    /// Cached `ST_AsMVT` query (`$1=z,$2=x,$3=y`).
    pub sql: String,
}

/// Per-connection health, surfaced to the catalog UI.
#[derive(Clone, Debug)]
pub struct ConnState {
    pub id: String,
    pub label: String,
    pub bundled: bool,
    pub ok: bool,
    pub message: String,
    pub table_count: usize,
}

/// The registry. Build with [`PgRegistry::new`]; share the returned `Arc`.
pub struct PgRegistry {
    rt: Runtime,
    config_path: PathBuf,
    /// Maps folder served alongside PostGIS — used to avoid id clashes with files.
    maps_dir: PathBuf,
    embed: Option<PgEmbed>,
    /// `conn_id -> (connection string used, pool)`. Keyed by the effective
    /// connection string so a credential change rebuilds the pool automatically.
    pools: RwLock<HashMap<String, (String, Arc<PostgresPool>)>>,
    sources: RwLock<HashMap<String, Arc<PgSource>>>,
    states: RwLock<Vec<ConnState>>,
    refresh_seconds: RwLock<u64>,
}

/// File-source stems (`.mbtiles` / `.tif` / `.tiff`) in the maps dir; PostGIS
/// source ids are kept distinct from these so neither shadows the other.
fn file_source_stems(maps_dir: &Path) -> HashSet<String> {
    let mut out = HashSet::new();
    if let Ok(rd) = std::fs::read_dir(maps_dir) {
        for e in rd.flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
            if matches!(ext.as_deref(), Some("mbtiles" | "tif" | "tiff")) {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    out.insert(slugify(stem));
                }
            }
        }
    }
    out
}

fn slugify(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        "layer".to_string()
    } else {
        out
    }
}

fn unique_slug(name: &str, used: &mut HashSet<String>) -> String {
    let base = slugify(name);
    if used.insert(base.clone()) {
        return base;
    }
    let mut i = 2;
    loop {
        let cand = format!("{base}_{i}");
        if used.insert(cand.clone()) {
            return cand;
        }
        i += 1;
    }
}

impl PgRegistry {
    /// Create the registry and start its background refresh thread. Returns
    /// immediately; the first discovery happens on the spawned thread.
    #[must_use]
    pub fn new(config_path: PathBuf, maps_dir: PathBuf, embed: Option<PgEmbed>) -> Arc<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("mts-pg")
            .build()
            .expect("pg runtime");
        let reg = Arc::new(Self {
            rt,
            config_path,
            maps_dir,
            embed,
            pools: RwLock::new(HashMap::new()),
            sources: RwLock::new(HashMap::new()),
            states: RwLock::new(Vec::new()),
            refresh_seconds: RwLock::new(30),
        });

        let weak = Arc::downgrade(&reg);
        std::thread::Builder::new()
            .name("mts-pg-refresh".into())
            .spawn(move || loop {
                let Some(r) = weak.upgrade() else { break };
                r.refresh();
                let secs = {
                    let s = *r.refresh_seconds.read().unwrap();
                    if s == 0 {
                        3600
                    } else {
                        s
                    }
                };
                drop(r);
                std::thread::sleep(Duration::from_secs(secs));
            })
            .expect("spawn pg refresh");

        reg
    }

    /// Re-read the config, (re)connect pools, and re-discover every table.
    /// Reuses live pools across refreshes so we don't reconnect each cycle.
    pub fn refresh(&self) {
        let cfg = PgConfig::load(&self.config_path);
        *self.refresh_seconds.write().unwrap() = cfg.refresh_seconds;

        // Make sure the bundled cluster is up before we try to connect to it.
        if let (Some(embed), Some(bc)) = (&self.embed, cfg.bundled()) {
            let have = self.pools.read().unwrap().contains_key(&bc.id);
            if !have || !embed.is_running(bc.port) {
                if let Err(e) = embed.ensure_running(bc) {
                    eprintln!("[pg] bundled cluster unavailable: {e}");
                }
            }
        }

        let existing = self.pools.read().unwrap().clone();
        let mut new_pools: HashMap<String, (String, Arc<PostgresPool>)> = HashMap::new();
        let mut new_sources: HashMap<String, Arc<PgSource>> = HashMap::new();
        let mut states: Vec<ConnState> = Vec::new();
        // Seed reserved ids with the file-source stems so a PostGIS table never
        // gets an id that a `.mbtiles`/`.tif` in the maps dir would shadow.
        let mut used_ids: HashSet<String> = file_source_stems(&self.maps_dir);

        self.rt.block_on(async {
            for conn in cfg.connections.iter().filter(|c| c.enabled) {
                let mut st = ConnState {
                    id: conn.id.clone(),
                    label: conn.label.clone(),
                    bundled: conn.bundled,
                    ok: false,
                    message: String::new(),
                    table_count: 0,
                };

                let conn_str = conn.conn_string();
                // Reuse a live pool only if its connection string is unchanged.
                let pool = match existing.get(&conn.id) {
                    Some((s, p)) if *s == conn_str => p.clone(),
                    _ => match PostgresPool::new(&conn_str, None, None, None, 4).await {
                        Ok(p) => Arc::new(p),
                        Err(e) => {
                            st.message = format!("connect failed: {e}");
                            states.push(st);
                            continue;
                        }
                    },
                };

                let tables = match discover_tables(&pool).await {
                    Ok(t) => t,
                    Err(e) => {
                        st.message = e;
                        new_pools.insert(conn.id.clone(), (conn_str, pool));
                        states.push(st);
                        continue;
                    }
                };

                let supports_margin = pool.supports_tile_margin();
                for t in tables {
                    let id = unique_slug(&t.table, &mut used_ids);
                    let bounds = t.compute_bounds(&pool).await.unwrap_or(WORLD);
                    let sql = t.build_mvt_sql(supports_margin);
                    new_sources.insert(
                        id.clone(),
                        Arc::new(PgSource {
                            id,
                            conn_id: conn.id.clone(),
                            conn_label: conn.label.clone(),
                            schema: t.schema,
                            table: t.table,
                            geom_type: t.geom_type,
                            srid: t.srid,
                            fields: t.properties,
                            minzoom: DEFAULT_MINZOOM,
                            maxzoom: DEFAULT_MAXZOOM,
                            bounds,
                            sql,
                        }),
                    );
                    st.table_count += 1;
                }

                st.ok = true;
                new_pools.insert(conn.id.clone(), (conn_str, pool));
                states.push(st);
            }
        });

        *self.pools.write().unwrap() = new_pools;
        *self.sources.write().unwrap() = new_sources;
        *self.states.write().unwrap() = states;
    }

    /// Force an immediate, synchronous refresh (used right after an import).
    pub fn refresh_now(&self) {
        self.refresh();
    }

    /// Drop all pooled connections and refresh from scratch. Use after connection
    /// credentials change so the new settings take effect immediately.
    pub fn reconnect(&self) {
        self.pools.write().unwrap().clear();
        self.refresh();
    }

    /// Serve one MVT tile for `source_id`, or `None` if unknown/empty.
    #[must_use]
    pub fn get_tile(&self, source_id: &str, z: u32, x: u32, y: u32) -> Option<Vec<u8>> {
        let src = self.sources.read().unwrap().get(source_id).cloned()?;
        let pool = self.pools.read().unwrap().get(&src.conn_id).map(|(_, p)| p.clone())?;
        let z = i32::try_from(z).ok()?;
        let x = i32::try_from(x).ok()?;
        let y = i32::try_from(y).ok()?;
        match self.rt.block_on(query_mvt(&pool, &src.sql, z, x, y)) {
            Ok(bytes) => bytes,
            Err(e) => {
                eprintln!("[pg] tile {source_id} {z}/{x}/{y}: {e}");
                None
            }
        }
    }

    /// Whether a given source id is a PostGIS vector source.
    #[must_use]
    pub fn has_source(&self, source_id: &str) -> bool {
        self.sources.read().unwrap().contains_key(source_id)
    }

    /// Snapshot of one source (for TileJSON / catalog metadata).
    #[must_use]
    pub fn get_source(&self, source_id: &str) -> Option<Arc<PgSource>> {
        self.sources.read().unwrap().get(source_id).cloned()
    }

    /// All sources, sorted by id (stable order for the catalog + landing page).
    #[must_use]
    pub fn list_sources(&self) -> Vec<Arc<PgSource>> {
        let mut v: Vec<Arc<PgSource>> =
            self.sources.read().unwrap().values().cloned().collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    /// Per-connection health for the UI.
    #[must_use]
    pub fn connection_states(&self) -> Vec<ConnState> {
        self.states.read().unwrap().clone()
    }
}
