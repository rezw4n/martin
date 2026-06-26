//! Tauri commands for the PostGIS data-source feature: connection CRUD (persisted
//! to `connections.json`), connection testing, importing shapefiles/GeoJSON via the
//! bundled `ogr2ogr` (reprojected to EPSG:4326), and dropping imported tables.
//!
//! Serving + discovery live in `mts-tile-server`; these commands drive that
//! registry and the on-disk config. Heavy/blocking work is moved off the async
//! runtime with `spawn_blocking` (the registry's own runtime uses `block_on`).

use std::path::Path;
use std::process::Stdio;

use martin_core::tiles::postgres::PostgresPool;
use mts_tile_server::pg::discover::discover_tables;
use mts_tile_server::pg::{config_path_for, PgConfig, PgConnection, PgEmbed};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::AppState;

/* ── DTOs ────────────────────────────────────────────────────────────────── */

#[derive(Serialize)]
pub struct PgConnDto {
    id: String,
    label: String,
    host: String,
    port: u16,
    dbname: String,
    user: String,
    sslmode: String,
    enabled: bool,
    bundled: bool,
    ok: bool,
    message: String,
    table_count: usize,
}

#[derive(Serialize)]
pub struct PgSourceDto {
    id: String,
    table: String,
    conn_id: String,
    conn_label: String,
    geom_type: String,
    srid: i32,
    fields: Vec<(String, String)>,
    minzoom: u8,
    maxzoom: u8,
    bounds: [f64; 4],
    tile_url: String,
    tilejson_url: String,
}

#[derive(Serialize)]
pub struct PgOverview {
    /// Whether PostGIS support is active at all (bundled cluster or a config file).
    available: bool,
    bundled_running: bool,
    connections: Vec<PgConnDto>,
    sources: Vec<PgSourceDto>,
}

#[derive(Serialize)]
pub struct PgTestResult {
    ok: bool,
    message: String,
    table_count: usize,
}

#[derive(Deserialize)]
pub struct ImportParams {
    /// Source file (.shp, .geojson, .json, .gpkg, …).
    path: String,
    /// Target connection id (defaults to the bundled cluster).
    conn_id: Option<String>,
    /// Target table name (defaults to a slug of the file name).
    table: Option<String>,
    /// Override the source CRS when the file lacks projection info (e.g. `EPSG:3857`).
    src_srs: Option<String>,
}

#[derive(Serialize)]
pub struct ImportReport {
    table: String,
    source_id: Option<String>,
    message: String,
}

/* ── helpers ─────────────────────────────────────────────────────────────── */

fn slugify(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "layer".to_string()
    } else {
        trimmed
    }
}

/// Quote a SQL identifier (double-quote, double embedded quotes).
fn sql_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// Quote a SQL string literal (single-quote, double embedded quotes).
fn sql_lit(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// Apply a new password to a role on the cluster `old` points at, authenticating
/// with the OLD credentials. Used so a bundled-password change actually takes.
async fn alter_role_password(old: &PgConnection, new_password: &str) -> Result<(), String> {
    let pool = PostgresPool::new(&old.conn_string(), None, None, None, 1)
        .await
        .map_err(|e| format!("Could not connect to change the password: {e}"))?;
    let client = pool.get().await.map_err(|e| format!("{e}"))?;
    let sql = format!("ALTER ROLE {} WITH PASSWORD {}", sql_ident(&old.user), sql_lit(new_password));
    client
        .batch_execute(&sql)
        .await
        .map_err(|e| format!("Changing the password failed: {e}"))?;
    Ok(())
}

/// Load the on-disk config (always with a bundled entry present).
fn load_cfg(state: &AppState) -> PgConfig {
    PgConfig::load(&config_path_for(&state.output_dir))
}

/* ── commands ────────────────────────────────────────────────────────────── */

/// Everything the catalog needs to render the PostGIS section in one call.
#[tauri::command]
pub fn pg_overview(state: State<AppState>) -> PgOverview {
    let Some(reg) = state.pg.clone() else {
        return PgOverview {
            available: false,
            bundled_running: false,
            connections: Vec::new(),
            sources: Vec::new(),
        };
    };
    let cfg = load_cfg(&state);
    let states = reg.connection_states();

    let connections = cfg
        .connections
        .iter()
        .map(|c| {
            let st = states.iter().find(|s| s.id == c.id);
            PgConnDto {
                id: c.id.clone(),
                label: c.label.clone(),
                host: c.host.clone(),
                port: c.port,
                dbname: c.dbname.clone(),
                user: c.user.clone(),
                sslmode: c.sslmode.clone(),
                enabled: c.enabled,
                bundled: c.bundled,
                ok: st.is_some_and(|s| s.ok),
                message: st.map(|s| s.message.clone()).unwrap_or_default(),
                table_count: st.map_or(0, |s| s.table_count),
            }
        })
        .collect();

    let base = &state.tile_base;
    let sources = reg
        .list_sources()
        .iter()
        .map(|s| PgSourceDto {
            id: s.id.clone(),
            table: s.table.clone(),
            conn_id: s.conn_id.clone(),
            conn_label: s.conn_label.clone(),
            geom_type: s.geom_type.clone(),
            srid: s.srid,
            fields: s.fields.clone(),
            minzoom: s.minzoom,
            maxzoom: s.maxzoom,
            bounds: s.bounds,
            tile_url: format!("{base}/{}/{{z}}/{{x}}/{{y}}", s.id),
            tilejson_url: format!("{base}/{}.json", s.id),
        })
        .collect();

    let bundled_running = states.iter().any(|s| s.bundled && s.ok);
    PgOverview { available: true, bundled_running, connections, sources }
}

/// Add or update an external connection, then reconnect the registry.
#[tauri::command]
pub async fn pg_save_connection(
    state: State<'_, AppState>,
    mut conn: PgConnection,
) -> Result<(), String> {
    let path = config_path_for(&state.output_dir);
    let mut cfg = PgConfig::load(&path);

    if conn.id.trim().is_empty() {
        conn.id = slugify(&conn.label);
    }
    if let Some(existing) = cfg.connections.iter().find(|c| c.id == conn.id).cloned() {
        // A blank password on an existing connection means "keep the current one"
        // (the editor never echoes the stored password back).
        let changing_password = !conn.password.is_empty() && conn.password != existing.password;
        let kept_password =
            if conn.password.is_empty() { existing.password.clone() } else { conn.password.clone() };

        // For the bundled cluster a real password change must also be applied to the
        // running role, or every later connection would fail to authenticate.
        if existing.bundled && changing_password {
            alter_role_password(&existing, &conn.password).await?;
        }

        let slot = cfg
            .connections
            .iter_mut()
            .find(|c| c.id == conn.id)
            .expect("connection still present");
        if existing.bundled {
            // Host/port/dbname/user of the bundled cluster are fixed.
            slot.label = conn.label;
            slot.sslmode = conn.sslmode;
            slot.enabled = conn.enabled;
            slot.password = kept_password;
        } else {
            slot.label = conn.label;
            slot.host = conn.host;
            slot.port = conn.port;
            slot.dbname = conn.dbname;
            slot.user = conn.user;
            slot.sslmode = conn.sslmode;
            slot.enabled = conn.enabled;
            slot.password = kept_password;
        }
    } else {
        conn.bundled = false;
        cfg.connections.push(conn);
    }
    cfg.save(&path).map_err(|e| e.to_string())?;

    if let Some(reg) = state.pg.clone() {
        tokio::task::spawn_blocking(move || reg.reconnect())
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Remove an external connection (the bundled one cannot be removed).
#[tauri::command]
pub async fn pg_delete_connection(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let path = config_path_for(&state.output_dir);
    let mut cfg = PgConfig::load(&path);
    if cfg.connections.iter().any(|c| c.id == id && c.bundled) {
        return Err("The bundled PostGIS connection cannot be removed.".into());
    }
    cfg.connections.retain(|c| c.id != id);
    cfg.save(&path).map_err(|e| e.to_string())?;

    if let Some(reg) = state.pg.clone() {
        tokio::task::spawn_blocking(move || reg.reconnect())
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Try a connection without saving it; report whether it works + table count.
#[tauri::command]
pub async fn pg_test_connection(conn: PgConnection) -> PgTestResult {
    match PostgresPool::new(&conn.conn_string(), None, None, None, 2).await {
        Ok(pool) => match discover_tables(&pool).await {
            Ok(tables) => PgTestResult {
                ok: true,
                message: format!("Connected — {} spatial table(s) found.", tables.len()),
                table_count: tables.len(),
            },
            Err(e) => PgTestResult {
                ok: false,
                message: format!("Connected, but discovery failed: {e}"),
                table_count: 0,
            },
        },
        Err(e) => PgTestResult { ok: false, message: format!("{e}"), table_count: 0 },
    }
}

/// Import a vector file into PostGIS (reprojected to EPSG:4326) via `ogr2ogr`.
#[tauri::command]
pub async fn pg_import(
    state: State<'_, AppState>,
    params: ImportParams,
) -> Result<ImportReport, String> {
    let src = Path::new(&params.path);
    if !src.is_file() {
        return Err(format!("File not found: {}", params.path));
    }
    let table = params
        .table
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(slugify)
        .unwrap_or_else(|| {
            slugify(src.file_stem().and_then(|s| s.to_str()).unwrap_or("layer"))
        });

    // Resolve the target connection.
    let cfg = load_cfg(&state);
    let conn = params
        .conn_id
        .as_deref()
        .and_then(|id| cfg.connections.iter().find(|c| c.id == id))
        .or_else(|| cfg.bundled())
        .cloned()
        .ok_or("No target PostGIS connection configured.")?;

    // Ensure the bundled cluster is up before importing into it.
    if conn.bundled {
        let embed = PgEmbed::new(
            mts_tile_server::pg::default_root(),
            mts_tile_server::pg::default_data_dir(&state.output_dir),
        );
        if !embed.binaries_present() {
            return Err(
                "Bundled PostgreSQL was not found next to the app. Re-extract the bundle so the \
                 pgsql\\ folder sits beside the executable, or add an external PostGIS connection."
                    .to_string(),
            );
        }
        let c = conn.clone();
        tokio::task::spawn_blocking(move || embed.ensure_running(&c))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("Could not start bundled PostgreSQL: {e}"))?;
    }

    // Locate ogr2ogr + the GDAL environment (for reprojection data).
    let (ogr, gdal_env) = {
        let guard = state.gdal.lock().map_err(|_| "GDAL state poisoned")?;
        let g = guard.as_ref().map_err(|e| format!("GDAL unavailable: {e}"))?;
        (g.tool("ogr2ogr"), g.env.clone())
    };

    // Quote each field libpq-style so spaces / symbols in a password (or any value)
    // don't break the DSN or inject extra keywords. Port is numeric.
    let dq = |v: &str| format!("'{}'", v.replace('\\', "\\\\").replace('\'', "\\'"));
    let pg_dsn = format!(
        "PG:host={} port={} dbname={} user={} password={}",
        dq(&conn.host),
        conn.port,
        dq(&conn.dbname),
        dq(&conn.user),
        dq(&conn.password)
    );

    // Build the ogr2ogr invocation.
    let mut args: Vec<String> = vec![
        "-f".into(),
        "PostgreSQL".into(),
        pg_dsn,
        params.path.clone(),
        "-nln".into(),
        table.clone(),
        "-t_srs".into(),
        "EPSG:4326".into(),
        "-nlt".into(),
        "PROMOTE_TO_MULTI".into(),
        "-lco".into(),
        "GEOMETRY_NAME=geom".into(),
        "-lco".into(),
        "FID=id".into(),
        "-lco".into(),
        "SPATIAL_INDEX=GIST".into(),
        "-lco".into(),
        "PRECISION=NO".into(),
        "--config".into(),
        "PG_USE_COPY".into(),
        "YES".into(),
        "-overwrite".into(),
    ];
    if let Some(srs) = params.src_srs.as_deref().filter(|s| !s.trim().is_empty()) {
        args.push("-s_srs".into());
        args.push(srs.to_string());
    }

    // Run ogr2ogr off the async runtime.
    let ogr_path = ogr.clone();
    let run = tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new(&ogr_path);
        cmd.args(&args).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in &gdal_env {
            cmd.env(k, v);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd.output()
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| format!("Could not run ogr2ogr: {e}"))?;

    if !run.status.success() {
        let stderr = String::from_utf8_lossy(&run.stderr);
        let stdout = String::from_utf8_lossy(&run.stdout);
        return Err(format!(
            "Import failed.\n{}\n{}",
            stderr.trim(),
            stdout.trim()
        ));
    }

    // ANALYZE so estimated bounds work immediately, then refresh the registry.
    if let Ok(pool) = PostgresPool::new(&conn.conn_string(), None, None, None, 1).await {
        if let Ok(client) = pool.get().await {
            let _ = client
                .batch_execute(&format!("ANALYZE \"public\".\"{}\"", table.replace('"', "\"\"")))
                .await;
        }
    }

    let source_id = if let Some(reg) = state.pg.clone() {
        let conn_id = conn.id.clone();
        let table_for = table.clone();
        tokio::task::spawn_blocking(move || {
            reg.refresh_now();
            reg.list_sources()
                .iter()
                .find(|s| s.table == table_for && s.conn_id == conn_id)
                .map(|s| s.id.clone())
        })
        .await
        .map_err(|e| e.to_string())?
    } else {
        None
    };

    Ok(ImportReport {
        table: table.clone(),
        source_id,
        message: format!("Imported “{table}” into {}.", conn.label),
    })
}

/// Drop an imported table (delete a PostGIS vector source).
#[tauri::command]
pub async fn pg_drop_source(state: State<'_, AppState>, source_id: String) -> Result<(), String> {
    let reg = state.pg.clone().ok_or("PostGIS is not available.")?;
    let src = reg.get_source(&source_id).ok_or("Unknown source.")?;
    let cfg = load_cfg(&state);
    let conn = cfg
        .connections
        .iter()
        .find(|c| c.id == src.conn_id)
        .cloned()
        .ok_or("Connection not found.")?;

    let schema = src.schema.replace('"', "\"\"");
    let table = src.table.replace('"', "\"\"");
    let pool = PostgresPool::new(&conn.conn_string(), None, None, None, 1)
        .await
        .map_err(|e| format!("{e}"))?;
    pool.get()
        .await
        .map_err(|e| format!("{e}"))?
        .batch_execute(&format!("DROP TABLE IF EXISTS \"{schema}\".\"{table}\""))
        .await
        .map_err(|e| format!("Drop failed: {e}"))?;

    tokio::task::spawn_blocking(move || reg.refresh_now())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}
