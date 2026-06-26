//! Map Tile Studio — Tauri backend.
//!
//! Wraps the `martin-tiler` generation engine as Tauri commands and serves
//! generated tiles to the in-app MapLibre preview via the `mbtile://` custom
//! protocol (no HTTP server, fully offline).

use std::path::PathBuf;
use std::sync::Mutex;

use martin_tiler::{
    generate as engine_generate, inspect_many, validate as engine_validate, GdalEnv,
    GenerateOptions, GenerateReport, ProgressEvent, RasterInfo, ValidationReport,
};
use percent_encoding::percent_decode_str;
use serde::Serialize;
use tauri::Emitter;

/// Shared application state: the discovered GDAL environment + the default
/// output directory.
struct AppState {
    gdal: Mutex<Result<GdalEnv, String>>,
    output_dir: PathBuf,
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

pub fn run() {
    // Default output directory: <app local data>/maps, created on first run.
    let output_dir = dirs_local_app_data()
        .unwrap_or_else(|| std::env::temp_dir())
        .join("MapTileStudio")
        .join("maps");
    let _ = std::fs::create_dir_all(&output_dir);

    let gdal = GdalEnv::discover().map_err(|e| e.to_string());

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState {
            gdal: Mutex::new(gdal),
            output_dir,
        })
        // mbtile://{z}/{x}/{y}?src=<urlencoded path>  →  tile bytes (or 204)
        .register_uri_scheme_protocol("mbtile", |_ctx, request| {
            use tauri::http::Response;
            let empty = || {
                Response::builder()
                    .status(204)
                    .header("Access-Control-Allow-Origin", "*")
                    .body(Vec::<u8>::new())
                    .unwrap()
            };

            let uri = request.uri();
            let parts: Vec<&str> = uri.path().trim_start_matches('/').split('/').collect();
            let src = uri
                .query()
                .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("src=")))
                .map(|v| percent_decode_str(v).decode_utf8_lossy().into_owned());

            let (Some(src), [z, x, y]) = (src, parts.as_slice()) else {
                return empty();
            };
            let (Ok(z), Ok(x), Ok(y)) = (z.parse(), x.parse(), y.parse::<u32>()) else {
                return empty();
            };

            match read_mbtiles_tile(&src, z, x, y) {
                Some(bytes) if !bytes.is_empty() => {
                    let ct = content_type_of(&bytes);
                    Response::builder()
                        .status(200)
                        .header("Content-Type", ct)
                        .header("Access-Control-Allow-Origin", "*")
                        .header("Cache-Control", "no-cache")
                        .body(bytes)
                        .unwrap()
                }
                _ => empty(),
            }
        })
        .invoke_handler(tauri::generate_handler![
            gdal_status,
            inspect_paths,
            validate_mbtiles,
            generate,
            cpu_count,
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
