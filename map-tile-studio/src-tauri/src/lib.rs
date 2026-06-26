//! Map Tile Studio — Tauri backend.
//!
//! Wraps the `martin-tiler` generation engine as Tauri commands and runs an
//! in-process XYZ tile server (`mts-tile-server`) for the in-app MapLibre
//! preview. The same server is shipped standalone as `tile-serviced`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use martin_core::tiles::cog::CogSource;
use martin_core::tiles::Source;
use martin_core::CacheZoomRange;
use martin_tiler::{
    generate as engine_generate, inspect_many, validate as engine_validate, GdalEnv,
    GenerateOptions, GenerateReport, ProgressEvent, RasterInfo, ValidationReport,
};
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
    /// Number of source GeoTIFFs stitched into this map (MBTiles only).
    sources: Option<u32>,
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
        sources: None,
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
        e.sources = get("mts_sources").and_then(|s| s.parse().ok());
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
        sources: None,
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

/* ── Background tile service (publish over LAN) ─────────────────────────────
 * The desktop app serves a loopback preview; for production the same maps are
 * served by the standalone `tile-serviced` binary installed as an OS service.
 * Status is queried in-process; install/uninstall/start/stop need admin so they
 * shell out to `tile-serviced` with UAC elevation.
 */

#[derive(Serialize)]
struct ServiceInfo {
    /// "running" | "stopped" | "not installed" | …
    status: String,
    port: u16,
    /// `http://<lan-ip>:<port>` — paste-able base for `/{source}/{z}/{x}/{y}`.
    lan_url: String,
    maps_dir: String,
}

/// Best-effort primary LAN IPv4 (no packets sent — just resolves the route).
fn lan_ip() -> String {
    std::net::UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}

/// Path to the `tile-serviced` binary (shipped next to the app exe; same dir in dev).
fn tile_serviced_path() -> PathBuf {
    let name = if cfg!(windows) { "tile-serviced.exe" } else { "tile-serviced" };
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|d| d.join(name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Persisted service port (so the UI shows the right URL across restarts).
fn service_port_file(state: &AppState) -> PathBuf {
    state.output_dir.join("..").join("service-port")
}
fn read_service_port(state: &AppState) -> u16 {
    std::fs::read_to_string(service_port_file(state))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(7765)
}

#[tauri::command]
fn service_status(state: tauri::State<'_, AppState>) -> ServiceInfo {
    let port = read_service_port(&state);
    ServiceInfo {
        status: mts_tile_server::service::status(),
        port,
        lan_url: format!("http://{}:{port}", lan_ip()),
        maps_dir: state.output_dir.display().to_string(),
    }
}

/// Run `tile-serviced <args…>` elevated (UAC on Windows / pkexec on Linux), waiting
/// for it to finish. Returns an error if elevation was declined or the call failed.
fn run_serviced_elevated(args: &[String]) -> Result<(), String> {
    let exe = tile_serviced_path();
    if !exe.exists() {
        return Err(format!("tile-serviced not found at {}", exe.display()));
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let esc = |s: &str| s.replace('\'', "''");
        let arglist = args
            .iter()
            .map(|a| format!("'{}'", esc(a)))
            .collect::<Vec<_>>()
            .join(",");
        let ps = format!(
            "$ErrorActionPreference='Stop'; $p = Start-Process -FilePath '{}' -ArgumentList {} -Verb RunAs -Wait -PassThru; exit $p.ExitCode",
            esc(&exe.display().to_string()),
            arglist
        );
        let status = std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .status()
            .map_err(|e| e.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err("the elevated command failed (UAC declined, or the service operation errored)".into())
        }
    }
    #[cfg(not(windows))]
    {
        let status = std::process::Command::new("pkexec")
            .arg(&exe)
            .args(args)
            .status()
            .map_err(|e| format!("pkexec not available: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("the elevated command failed".into())
        }
    }
}

#[tauri::command]
async fn service_install(state: tauri::State<'_, AppState>, port: u16) -> Result<(), String> {
    let maps = state.output_dir.display().to_string();
    let _ = std::fs::write(service_port_file(&state), port.to_string());
    run_serviced_elevated(&[
        "install".to_string(),
        "--maps".to_string(),
        maps,
        "--bind".to_string(),
        format!("0.0.0.0:{port}"),
    ])
}

#[tauri::command]
async fn service_uninstall() -> Result<(), String> {
    run_serviced_elevated(&["uninstall".to_string()])
}

#[tauri::command]
async fn service_set_running(start: bool) -> Result<(), String> {
    run_serviced_elevated(&[if start { "start" } else { "stop" }.to_string()])
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

    // Loopback XYZ tile server for offline previews + copyable tile URLs.
    // (The standalone `tile-serviced` binary serves the same maps for production.)
    let tile_base_url = mts_tile_server::serve_background(output_dir.clone(), 7765)
        .map(|port| format!("http://127.0.0.1:{port}"))
        .unwrap_or_default();

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
            service_status,
            service_install,
            service_uninstall,
            service_set_running,
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
