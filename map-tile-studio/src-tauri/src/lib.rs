//! Map Tile Studio — Tauri backend.
//!
//! Wraps the `martin-tiler` generation engine as Tauri commands and serves
//! generated tiles to the in-app MapLibre preview via the `mbtile://` custom
//! protocol (no HTTP server, fully offline).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use martin_core::tiles::cog::CogSource;
use martin_core::tiles::Source;
use martin_core::CacheZoomRange;
use martin_tile_utils::TileCoord;
use martin_tiler::{
    generate as engine_generate, inspect_many, validate as engine_validate, GdalEnv,
    GenerateOptions, GenerateReport, ProgressEvent, RasterInfo, ValidationReport,
};
use percent_encoding::percent_decode_str;
use serde::Serialize;
use tauri::Emitter;

/// Shared application state: the discovered GDAL environment + the default
/// output directory + the local tile-server base URL.
struct AppState {
    gdal: Mutex<Result<GdalEnv, String>>,
    output_dir: PathBuf,
    tile_base: String,
}

/// Base URL of the local XYZ tile server, e.g. `http://127.0.0.1:7765`.
/// Tiles are served at `{base}/{source}/{z}/{x}/{y}`.
#[tauri::command]
fn tile_base(state: tauri::State<'_, AppState>) -> String {
    state.tile_base.clone()
}

#[derive(Serialize)]
struct GdalStatus {
    available: bool,
    error: Option<String>,
    bin: Option<String>,
    output_dir: String,
}

#[tauri::command]
fn gdal_status(state: tauri::State<'_, AppState>) -> GdalStatus {
    let guard = state.gdal.lock().expect("gdal state");
    match &*guard {
        Ok(env) => GdalStatus {
            available: true,
            error: None,
            bin: Some(env.bin_dir.display().to_string()),
            output_dir: state.output_dir.display().to_string(),
        },
        Err(e) => GdalStatus {
            available: false,
            error: Some(e.clone()),
            bin: None,
            output_dir: state.output_dir.display().to_string(),
        },
    }
}

/// Clone the discovered GdalEnv out of state (so we can use it across awaits).
fn gdal_clone(state: &tauri::State<'_, AppState>) -> Result<GdalEnv, String> {
    state.gdal.lock().expect("gdal state").clone()
}

#[tauri::command]
async fn inspect_paths(
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
) -> Result<Vec<RasterInfo>, String> {
    let gdal = gdal_clone(&state)?;
    let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    inspect_many(&gdal, &paths).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn validate_mbtiles(path: String) -> Result<ValidationReport, String> {
    engine_validate(&PathBuf::from(path)).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn generate(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    opts: GenerateOptions,
) -> Result<GenerateReport, String> {
    let gdal = gdal_clone(&state)?;
    // Make sure the output directory exists.
    let _ = std::fs::create_dir_all(&opts.output_dir);

    let app_for_progress = app.clone();
    let on_progress = move |ev: ProgressEvent| {
        // Stream every engine event straight to the UI. Ignore emit failures.
        let _ = app_for_progress.emit("mts://progress", &ev);
    };

    engine_generate(&gdal, &opts, on_progress)
        .await
        .map_err(|e| e.to_string())
}

/// Number of logical CPUs (used by the UI to default the worker count).
#[tauri::command]
fn cpu_count() -> usize {
    num_cpus::get()
}

/// Read a single tile from an MBTiles file (XYZ scheme; MBTiles stores TMS).
fn read_mbtiles_tile(src: &str, z: u32, x: u32, y: u32) -> Option<Vec<u8>> {
    use rusqlite::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(
        src,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    // XYZ y (top-origin) -> TMS row (bottom-origin)
    let tms_y = (1u32 << z).checked_sub(1)?.checked_sub(y)?;
    conn.query_row(
        "SELECT tile_data FROM tiles WHERE zoom_level=?1 AND tile_column=?2 AND tile_row=?3",
        rusqlite::params![z, x, tms_y],
        |row| row.get::<_, Vec<u8>>(0),
    )
    .ok()
}

fn content_type_of(bytes: &[u8]) -> &'static str {
    if bytes.len() >= 4 && &bytes[0..4] == b"\x89PNG" {
        "image/png"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xD8 {
        "image/jpeg"
    } else {
        "application/octet-stream"
    }
}

/* ── Local XYZ tile server ──────────────────────────────────────────────────
 * A tiny HTTP server so generated maps render as standard XYZ raster tiles in
 * the preview (more robust than a custom URI scheme) and so the tile URL is
 * copyable into other tools (QGIS, Leaflet, …). Serves only from output_dir,
 * fully offline. URL shape: GET /{source}/{z}/{x}/{y}[.ext]
 */

fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes())
        .expect("static header is valid")
}

/// Parse + sanitize `/{source}/{z}/{x}/{y}[.ext]` into its parts, confined to a
/// single in-folder source name (no path traversal).
fn parse_tile_path(url: &str) -> Option<(String, u32, u32, u32)> {
    let path = url.split('?').next().unwrap_or("");
    let segs: Vec<&str> = path.trim_start_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let [source, z, x, y] = segs.as_slice() else {
        return None;
    };
    // strip an optional file extension on the y segment (e.g. `7079.webp`)
    let y = y.split('.').next().unwrap_or(y);
    let source = percent_decode_str(source).decode_utf8_lossy().into_owned();
    if source.is_empty() || source.contains(['/', '\\', ':']) || source.contains("..") {
        return None;
    }
    Some((source, z.parse().ok()?, x.parse().ok()?, y.parse().ok()?))
}

/// Resolve a tile request to image bytes — from an MBTiles (TMS y-flip) or a COG
/// (`martin-core`), whichever file backs `{source}` in `output_dir`.
fn resolve_tile(
    url: &str,
    output_dir: &Path,
    rt: &tokio::runtime::Runtime,
    cogs: &mut std::collections::HashMap<PathBuf, CogSource>,
) -> Option<Vec<u8>> {
    let (source, z, x, y) = parse_tile_path(url)?;

    let mbtiles = output_dir.join(format!("{source}.mbtiles"));
    if mbtiles.is_file() {
        return read_mbtiles_tile(&mbtiles.to_string_lossy(), z, x, y).filter(|b| !b.is_empty());
    }

    for ext in ["tif", "tiff"] {
        let cog_path = output_dir.join(format!("{source}.{ext}"));
        if !cog_path.is_file() {
            continue;
        }
        use std::collections::hash_map::Entry;
        let src = match cogs.entry(cog_path.clone()) {
            Entry::Occupied(e) => e.into_mut(),
            Entry::Vacant(v) => {
                let s = CogSource::new(source.clone(), cog_path.clone(), CacheZoomRange::default())
                    .ok()?;
                v.insert(s)
            }
        };
        let coord = TileCoord { z: u8::try_from(z).ok()?, x, y };
        return rt.block_on(src.get_tile(coord, None)).ok().filter(|b| !b.is_empty());
    }
    None
}

/// Start the tile server on the first free port in a small range; returns the
/// base URL (e.g. `http://127.0.0.1:7765`). Spawns a detached worker thread.
fn start_tile_server(output_dir: PathBuf) -> String {
    let (server, port) = (7765u16..7795)
        .find_map(|p| tiny_http::Server::http(("127.0.0.1", p)).ok().map(|s| (s, p)))
        .or_else(|| {
            // last resort: an ephemeral port
            tiny_http::Server::http(("127.0.0.1", 0)).ok().map(|s| {
                let p = s.server_addr().to_ip().map_or(0, |a| a.port());
                (s, p)
            })
        })
        .expect("could not bind a local tile-server port");

    std::thread::spawn(move || {
        // A small runtime to drive `CogSource::get_tile` (async); the server loop
        // is single-threaded, so one current-thread runtime + COG cache is enough.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tile-server runtime");
        let mut cogs: std::collections::HashMap<PathBuf, CogSource> = std::collections::HashMap::new();

        for req in server.incoming_requests() {
            // CORS + Private-Network-Access headers so the webview (an app origin
            // fetching a loopback address) is allowed to read the tiles.
            let cors = || header("Access-Control-Allow-Origin", "*");
            let pna = || header("Access-Control-Allow-Private-Network", "true");

            // Answer CORS / PNA preflight.
            if req.method() == &tiny_http::Method::Options {
                let resp = tiny_http::Response::from_data(Vec::new())
                    .with_status_code(tiny_http::StatusCode(204))
                    .with_header(cors())
                    .with_header(pna())
                    .with_header(header("Access-Control-Allow-Methods", "GET, OPTIONS"))
                    .with_header(header("Access-Control-Allow-Headers", "*"));
                let _ = req.respond(resp);
                continue;
            }

            let url = req.url().to_string();
            let resp = match resolve_tile(&url, &output_dir, &rt, &mut cogs) {
                Some(bytes) => {
                    let ct = content_type_of(&bytes);
                    tiny_http::Response::from_data(bytes)
                        .with_header(header("Content-Type", ct))
                        .with_header(header("Cache-Control", "no-cache"))
                        .with_header(cors())
                        .with_header(pna())
                }
                // empty/sparse/missing → 204 so MapLibre treats it as a blank tile
                None => tiny_http::Response::from_data(Vec::new())
                    .with_status_code(tiny_http::StatusCode(204))
                    .with_header(cors())
                    .with_header(pna()),
            };
            let _ = req.respond(resp);
        }
    });

    format!("http://127.0.0.1:{port}")
}

/* ── Tiles catalog ──────────────────────────────────────────────────────── */

#[derive(Serialize)]
struct MapEntry {
    source_id: String,
    path: String,
    kind: String, // "mbtiles" | "cog"
    name: String,
    format: String,
    crs: Option<String>,
    min_zoom: Option<u32>,
    max_zoom: Option<u32>,
    tiles_total: Option<u64>,
    bounds: Option<[f64; 4]>,
    size: u64,
    modified: u64,
}

fn file_meta(path: &Path) -> (u64, u64) {
    let m = std::fs::metadata(path).ok();
    let size = m.as_ref().map(std::fs::Metadata::len).unwrap_or(0);
    let modified = m
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    (size, modified)
}

fn read_mbtiles_entry(path: &Path) -> MapEntry {
    use rusqlite::{Connection, OpenFlags};
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let (size, modified) = file_meta(path);
    let mut e = MapEntry {
        source_id: stem.clone(),
        path: path.display().to_string(),
        kind: "mbtiles".into(),
        name: stem,
        format: "png".into(),
        crs: None,
        min_zoom: None,
        max_zoom: None,
        tiles_total: None,
        bounds: None,
        size,
        modified,
    };
    if let Ok(conn) =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
    {
        let get = |k: &str| {
            conn.query_row("SELECT value FROM metadata WHERE name=?1", [k], |r| {
                r.get::<_, String>(0)
            })
            .ok()
        };
        if let Some(n) = get("name").filter(|s| !s.is_empty()) {
            e.name = n;
        }
        if let Some(f) = get("format") {
            e.format = f;
        }
        e.crs = get("crs");
        e.min_zoom = get("minzoom").and_then(|s| s.parse().ok());
        e.max_zoom = get("maxzoom").and_then(|s| s.parse().ok());
        if let Some(b) = get("bounds") {
            let parts: Vec<f64> = b.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if let [w, s, ee, n] = parts.as_slice() {
                e.bounds = Some([*w, *s, *ee, *n]);
            }
        }
        e.tiles_total = conn
            .query_row("SELECT count(*) FROM tiles", [], |r| r.get::<_, i64>(0))
            .ok()
            .map(|n| n as u64);
    }
    e
}

fn format_label(f: martin_tile_utils::Format) -> String {
    use martin_tile_utils::Format;
    match f {
        Format::Png => "png",
        Format::Jpeg => "jpeg",
        Format::Webp => "webp",
        _ => "cog",
    }
    .into()
}

fn read_cog_entry(path: &Path) -> MapEntry {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let (size, modified) = file_meta(path);
    let mut e = MapEntry {
        source_id: stem.clone(),
        path: path.display().to_string(),
        kind: "cog".into(),
        name: stem.clone(),
        format: "cog".into(),
        crs: Some("EPSG:3857".into()),
        min_zoom: None,
        max_zoom: None,
        tiles_total: None,
        bounds: None,
        size,
        modified,
    };
    // A valid (web-mercator) COG: read zoom range + WGS84 bounds + tile format so
    // the catalog can preview it on the map.
    if let Ok(src) = CogSource::new(stem, path.to_path_buf(), CacheZoomRange::default()) {
        let tj = src.get_tilejson();
        e.min_zoom = tj.minzoom.map(u32::from);
        e.max_zoom = tj.maxzoom.map(u32::from);
        if let Some(b) = &tj.bounds {
            e.bounds = Some([b.left, b.bottom, b.right, b.top]);
        }
        e.format = format_label(src.get_tile_info().format);
    }
    e
}

/// Scan the output folder for tile maps. Opens a SQLite connection + counts tiles
/// per MBTiles, so this is run off the IPC thread (see `list_maps`).
fn scan_maps(output_dir: &Path) -> Vec<MapEntry> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(output_dir) else {
        return out;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        match p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
            Some("mbtiles") => out.push(read_mbtiles_entry(&p)),
            Some("tif") | Some("tiff") => out.push(read_cog_entry(&p)),
            _ => {}
        }
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    out
}

#[tauri::command]
async fn list_maps(state: tauri::State<'_, AppState>) -> Result<Vec<MapEntry>, String> {
    // The per-file `count(*)` can be slow for big catalogs; do it on a blocking
    // worker so the UI thread never stalls on catalog load/refresh.
    let dir = state.output_dir.clone();
    tokio::task::spawn_blocking(move || scan_maps(&dir))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_maps(state: tauri::State<'_, AppState>, paths: Vec<String>) -> Result<(), String> {
    // Only ever delete files that actually live inside the managed output folder —
    // never trust an arbitrary path handed over IPC.
    let base = state
        .output_dir
        .canonicalize()
        .unwrap_or_else(|_| state.output_dir.clone());
    for p in paths {
        let canon = PathBuf::from(&p).canonicalize().map_err(|e| format!("{p}: {e}"))?;
        if canon.parent() != Some(base.as_path()) {
            return Err(format!("refusing to delete outside the maps folder: {p}"));
        }
        std::fs::remove_file(&canon).map_err(|e| format!("{p}: {e}"))?;
        // also drop a sibling sidecar (e.g. <file>.tif.aux.xml), which is in-folder by construction
        let _ = std::fs::remove_file(PathBuf::from(format!("{}.aux.xml", canon.display())));
    }
    Ok(())
}

#[tauri::command]
fn import_map(state: tauri::State<'_, AppState>, path: String) -> Result<String, String> {
    let src = PathBuf::from(&path);
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !matches!(ext.as_str(), "mbtiles" | "tif" | "tiff") {
        return Err("only .mbtiles / .tif / .tiff can be imported".into());
    }
    let _ = std::fs::create_dir_all(&state.output_dir);
    let file = src.file_name().ok_or("invalid file name")?;
    let mut dest = state.output_dir.join(file);
    // Never overwrite an existing catalog file — keep trying suffixes until free.
    if dest.exists() {
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("import");
        let mut n = 1u32;
        loop {
            let name = if n == 1 {
                format!("{stem}-imported.{ext}")
            } else {
                format!("{stem}-imported-{n}.{ext}")
            };
            let cand = state.output_dir.join(name);
            if !cand.exists() {
                dest = cand;
                break;
            }
            n += 1;
        }
    }
    std::fs::copy(&src, &dest).map_err(|e| e.to_string())?;
    Ok(dest.display().to_string())
}

/// Point the engine at a portable GDAL bundled next to the exe (`./gdal`, `./python`),
/// so the app is self-contained — the client just unzips and runs the exe.
fn setup_bundled_gdal() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    let bin = dir.join("gdal").join("bin");
    let gdalinfo = if cfg!(windows) { "gdalinfo.exe" } else { "gdalinfo" };
    if !bin.join(gdalinfo).exists() {
        return; // no bundled GDAL — fall back to system discovery
    }
    std::env::set_var("MARTIN_GDAL_BIN", &bin);
    std::env::set_var("MARTIN_GDAL_PREFIX", dir.join("gdal"));
    let py = dir.join("python").join(if cfg!(windows) { "python.exe" } else { "python" });
    if py.exists() {
        std::env::set_var("MARTIN_GDAL_PYTHON", &py);
    }
}

pub fn run() {
    setup_bundled_gdal();

    // Default output directory: <app local data>/maps, created on first run.
    let output_dir = dirs_local_app_data()
        .unwrap_or_else(|| std::env::temp_dir())
        .join("MapTileStudio")
        .join("maps");
    let _ = std::fs::create_dir_all(&output_dir);

    let gdal = GdalEnv::discover().map_err(|e| e.to_string());

    // Local XYZ tile server for offline previews + copyable tile URLs.
    let tile_base_url = start_tile_server(output_dir.clone());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            gdal: Mutex::new(gdal),
            output_dir,
            tile_base: tile_base_url,
        })
        .invoke_handler(tauri::generate_handler![
            gdal_status,
            inspect_paths,
            validate_mbtiles,
            generate,
            cpu_count,
            list_maps,
            delete_maps,
            import_map,
            tile_base,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Map Tile Studio");
}

/// Local app-data dir without pulling in the `dirs` crate.
fn dirs_local_app_data() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
    }
}
