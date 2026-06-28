//! Tauri commands for the PostGIS data-source feature: connection CRUD (persisted
//! to `connections.json`), connection testing, importing shapefiles/GeoJSON via the
//! bundled `ogr2ogr` (reprojected to EPSG:4326), and dropping imported tables.
//!
//! Serving + discovery live in `mts-tile-server`; these commands drive that
//! registry and the on-disk config. Heavy/blocking work is moved off the async
//! runtime with `spawn_blocking` (the registry's own runtime uses `block_on`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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

/* ── source CRS resolution (so reprojection lands data correctly) ─────────── */

/// EPSG Gulshan 303 -> WGS 84 geocentric translation (the standard Bangladesh
/// Everest datum shift). Without it, Everest-based local grids land ~300 m off.
const BD_TOWGS84: &str = "+towgs84=283.729,735.942,261.143";

/// Read the source CRS of `target` (a dataset or a `.prj`) as a PROJ.4 string,
/// using the bundled GDAL (`gdalsrsinfo`). Returns `None` when there is no CRS.
fn proj4_of(srsinfo: &Path, target: &Path, env: &BTreeMap<String, String>) -> Option<String> {
    let mut cmd = std::process::Command::new(srsinfo);
    cmd.arg("-o").arg("proj4").arg(target);
    for (k, v) in env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .find(|l| l.contains("+proj="))
        .map(str::to_string)
}

/// True if a PROJ.4 string uses an Everest ellipsoid — by `+ellps=evrst*` OR by
/// the numeric semi-major axes GDAL emits when it can't name the ellipsoid
/// (`+a=6377276.345` etc., the various Everest 1830 definitions).
fn is_everest(proj4_lc: &str) -> bool {
    proj4_lc.contains("evrst")
        || ["+a=6377276", "+a=6377298", "+a=6377299", "+a=6377301", "+a=6377304"]
            .iter()
            .any(|a| proj4_lc.contains(a))
}

/// True if the PROJ.4 already carries a *real* datum transformation: a named
/// `+datum=`, or a `+towgs84=` with at least one non-zero component (GDAL can
/// emit an identity `+towgs84=0,0,0,0,0,0,0`, which is NOT a transformation).
fn has_real_datum_shift(proj4_lc: &str) -> bool {
    if proj4_lc.contains("+datum=") {
        return true;
    }
    proj4_lc
        .split("+towgs84=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .is_some_and(|nums| {
            nums.split(',').any(|n| n.trim().parse::<f64>().is_ok_and(|v| v != 0.0))
        })
}

/// Central meridian (`+lon_0`) of a PROJ.4 string, if any.
fn lon_0_of(proj4: &str) -> Option<f64> {
    proj4
        .split_whitespace()
        .find_map(|t| t.strip_prefix("+lon_0=").and_then(|v| v.parse::<f64>().ok()))
}

/// Inject the Bangladesh datum shift into a PROJ.4 string when it uses an Everest
/// ellipsoid, carries no real datum transformation, and is centred over Bangladesh
/// (so Indian/Nepali Everest grids aren't mis-shifted). Returns the string
/// unchanged when no fix is needed.
fn with_bd_datum_shift(proj4: &str) -> String {
    let lc = proj4.to_lowercase();
    // No `+lon_0` (e.g. a geographic CRS) → assume Bangladesh (fail-open).
    let over_bangladesh = lon_0_of(proj4).is_none_or(|l| (86.0..=94.0).contains(&l));
    if is_everest(&lc) && !has_real_datum_shift(&lc) && over_bangladesh {
        // Drop any identity +towgs84 first, then append the real shift.
        let cleaned: Vec<&str> = proj4
            .split_whitespace()
            .filter(|t| !t.to_lowercase().starts_with("+towgs84="))
            .collect();
        format!("{} {BD_TOWGS84}", cleaned.join(" "))
    } else {
        proj4.to_string()
    }
}

/// Outcome of resolving an import's source CRS.
enum SrsResolution {
    /// The file has a usable CRS already; let `ogr2ogr` read it (no `-s_srs`).
    UseFileCrs,
    /// Use this PROJ.4 as `-s_srs` (Everest fix, or a borrowed sibling CRS).
    Override(String),
    /// The file has no CRS and no sibling `.prj` to infer one from.
    NoCrs,
    /// The file has no CRS and the folder mixes projections (can't infer one).
    Ambiguous,
}

/// Decide how to source the CRS for `input`:
/// 1. If the file has a CRS, only override it when an Everest datum shift is needed.
/// 2. If it has **no** CRS (a `.shp` with no `.prj`), borrow a sibling `.prj` from
///    the same folder — but only when every sibling agrees on one CRS.
fn resolve_source_srs(srsinfo: &Path, input: &Path, env: &BTreeMap<String, String>) -> SrsResolution {
    if let Some(p4) = proj4_of(srsinfo, input, env) {
        let fixed = with_bd_datum_shift(&p4);
        return if fixed == p4 { SrsResolution::UseFileCrs } else { SrsResolution::Override(fixed) };
    }
    // No CRS on the input — gather the DISTINCT CRSs of sibling .prj files.
    let Some(dir) = input.parent() else { return SrsResolution::NoCrs };
    let Ok(entries) = std::fs::read_dir(dir) else { return SrsResolution::NoCrs };
    let mut distinct: Vec<String> = Vec::new();
    for path in entries.flatten().map(|e| e.path()) {
        let is_prj = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("prj"));
        if is_prj {
            if let Some(p4) = proj4_of(srsinfo, &path, env) {
                if !distinct.contains(&p4) {
                    distinct.push(p4);
                }
            }
        }
    }
    match distinct.as_slice() {
        [] => SrsResolution::NoCrs,
        [one] => SrsResolution::Override(with_bd_datum_shift(one)),
        _ => SrsResolution::Ambiguous,
    }
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

    // Locate ogr2ogr + gdalsrsinfo + the GDAL environment (for reprojection data).
    let (ogr, srsinfo, gdal_env) = {
        let guard = state.gdal.lock().map_err(|_| "GDAL state poisoned")?;
        let g = guard.as_ref().map_err(|e| format!("GDAL unavailable: {e}"))?;
        (g.tool("ogr2ogr"), g.tool("gdalsrsinfo"), g.env.clone())
    };

    // Resolve the source CRS: an explicit override wins; otherwise read it from the
    // file (or a sibling .prj in the same folder) and inject the Bangladesh datum
    // shift for Everest grids that ship without one — so the data lands in the
    // right place regardless of the (often custom / header-less) projection.
    let effective_s_srs: Option<String> =
        if let Some(s) = params.src_srs.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(s.to_string())
        } else {
            let (si, input, env) = (srsinfo.clone(), PathBuf::from(&params.path), gdal_env.clone());
            let res = tokio::task::spawn_blocking(move || resolve_source_srs(&si, &input, &env))
                .await
                .map_err(|e| e.to_string())?;
            match res {
                SrsResolution::UseFileCrs => None,
                SrsResolution::Override(s) => Some(s),
                SrsResolution::NoCrs => {
                    return Err("This file has no projection (.prj) and none could be inferred \
                                from the folder. Set a Source CRS (e.g. EPSG:3857) in the import \
                                dialog and try again."
                        .to_string());
                }
                SrsResolution::Ambiguous => {
                    return Err("This shapefile has no .prj, and the folder mixes shapefiles in \
                                different projections — so the CRS can't be inferred. Set a Source \
                                CRS explicitly in the import dialog."
                        .to_string());
                }
            }
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
    if let Some(s) = &effective_s_srs {
        args.push("-s_srs".into());
        args.push(s.clone());
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

#[cfg(test)]
mod tests {
    use super::{has_real_datum_shift, is_everest, lon_0_of, with_bd_datum_shift, BD_TOWGS84};

    #[test]
    fn everest_detected_by_name_and_axis() {
        assert!(is_everest("+proj=tmerc +ellps=evrst30 +units=m"));
        // GDAL's numeric form for the various Everest 1830 definitions.
        assert!(is_everest("+proj=tmerc +a=6377276.345 +rf=300.802 +units=m"));
        assert!(is_everest("+proj=cass +a=6377299.36 +rf=300.8017 +units=m"));
        assert!(is_everest("+proj=tmerc +a=6377301.243 +rf=300.8017255"));
        assert!(!is_everest("+proj=longlat +datum=wgs84"));
        assert!(!is_everest("+proj=utm +zone=46 +a=6378137 +rf=298.257223563"));
    }

    #[test]
    fn real_vs_identity_datum_shift() {
        assert!(has_real_datum_shift("+proj=longlat +datum=wgs84 +no_defs"));
        assert!(has_real_datum_shift("+ellps=evrst30 +towgs84=283.729,735.942,261.143"));
        // An identity towgs84 is NOT a real transformation.
        assert!(!has_real_datum_shift("+ellps=evrst30 +towgs84=0,0,0,0,0,0,0"));
        assert!(!has_real_datum_shift("+ellps=evrst30 +towgs84=0,0,0"));
        assert!(!has_real_datum_shift("+proj=tmerc +ellps=evrst30 +units=m"));
    }

    #[test]
    fn lon0_parsing() {
        assert_eq!(lon_0_of("+proj=tmerc +lon_0=90.5 +ellps=evrst30"), Some(90.5));
        assert_eq!(lon_0_of("+proj=longlat +ellps=evrst30"), None);
    }

    #[test]
    fn injects_for_bangladesh_everest() {
        // Canonical Gulshan 303 (named ellipsoid).
        let canonical = "+proj=tmerc +lat_0=24.5 +lon_0=90.5 +k=1 +x_0=100000 +y_0=200000 +ellps=evrst30 +units=m +no_defs";
        let out = with_bd_datum_shift(canonical);
        assert!(out.contains(BD_TOWGS84), "named ellipsoid must get the shift: {out}");

        // Variant flattening → numeric axis, still over Bangladesh.
        let numeric = "+proj=tmerc +lat_0=24.5 +lon_0=90.5 +a=6377276.345 +rf=300.802 +units=m +no_defs";
        assert!(with_bd_datum_shift(numeric).contains(BD_TOWGS84));

        // Identity towgs84 is stripped and the real shift applied (no duplicate).
        let identity = "+proj=tmerc +lon_0=90 +ellps=evrst30 +towgs84=0,0,0,0,0,0,0 +no_defs";
        let fixed = with_bd_datum_shift(identity);
        assert!(fixed.contains(BD_TOWGS84));
        assert!(!fixed.contains("towgs84=0,0,0"), "identity shift must be removed: {fixed}");
    }

    #[test]
    fn leaves_correct_or_non_bangladesh_crs_untouched() {
        // Already has the real shift → unchanged.
        let ok = "+proj=tmerc +lon_0=90.5 +ellps=evrst30 +towgs84=283.729,735.942,261.143 +no_defs";
        assert_eq!(with_bd_datum_shift(ok), ok);
        // Proper WGS84 → unchanged.
        let wgs = "+proj=longlat +datum=WGS84 +no_defs";
        assert_eq!(with_bd_datum_shift(wgs), wgs);
        // Everest but centred over India (lon_0=80) → not Bangladesh → unchanged.
        let india = "+proj=tmerc +lat_0=0 +lon_0=80 +ellps=evrst30 +units=m +no_defs";
        assert_eq!(with_bd_datum_shift(india), india);
    }
}
