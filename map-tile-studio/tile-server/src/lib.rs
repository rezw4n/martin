//! Headless XYZ tile server shared by the Studio app (in-app preview) and the
//! `tile-serviced` background service.
//!
//! Serves standard XYZ raster tiles from a maps directory, fully offline:
//!   GET /{source}/{z}/{x}/{y}
//! `{source}.mbtiles` is read directly (XYZ→TMS y-flip); `{source}.tif` / `.tiff`
//! is read as a Cloud-Optimized GeoTIFF via `martin-core`. Empty/sparse/missing
//! tiles return `204` so MapLibre/Leaflet treat them as blank.
//!
//! Requests are handled concurrently by a pool of worker threads, each sharing the
//! same `tiny_http::Server` and keeping its own COG cache + runtime.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod service;

use martin_core::tiles::cog::CogSource;
use martin_core::tiles::Source;
use martin_core::CacheZoomRange;
use martin_tile_utils::TileCoord;
use percent_encoding::percent_decode_str;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

/// Read a single tile from an MBTiles file (XYZ in → MBTiles stores TMS).
pub fn read_mbtiles_tile(src: &str, z: u32, x: u32, y: u32) -> Option<Vec<u8>> {
    use rusqlite::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(
        src,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
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

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("static header is valid")
}

/// Parse + sanitize `/{source}/{z}/{x}/{y}[.ext]`, confined to a single in-folder
/// source name (no path traversal).
fn parse_tile_path(url: &str) -> Option<(String, u32, u32, u32)> {
    let path = url.split('?').next().unwrap_or("");
    let segs: Vec<&str> =
        path.trim_start_matches('/').split('/').filter(|s| !s.is_empty()).collect();
    let [source, z, x, y] = segs.as_slice() else {
        return None;
    };
    let y = y.split('.').next().unwrap_or(y);
    let source = percent_decode_str(source).decode_utf8_lossy().into_owned();
    if source.is_empty() || source.contains(['/', '\\', ':']) || source.contains("..") {
        return None;
    }
    Some((source, z.parse().ok()?, x.parse().ok()?, y.parse().ok()?))
}

/// Resolve a request URL to tile bytes from the MBTiles or COG backing `{source}`.
fn resolve_tile(
    url: &str,
    maps_dir: &Path,
    rt: &tokio::runtime::Runtime,
    cogs: &mut HashMap<PathBuf, CogSource>,
) -> Option<Vec<u8>> {
    let (source, z, x, y) = parse_tile_path(url)?;

    let mbtiles = maps_dir.join(format!("{source}.mbtiles"));
    if mbtiles.is_file() {
        return read_mbtiles_tile(&mbtiles.to_string_lossy(), z, x, y).filter(|b| !b.is_empty());
    }

    for ext in ["tif", "tiff"] {
        let cog_path = maps_dir.join(format!("{source}.{ext}"));
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

/* ── catalog endpoints (landing page + TileJSON) ────────────────────────── */

struct SourceMeta {
    minzoom: u8,
    maxzoom: u8,
    bounds: [f64; 4],
}

/// List servable source names (file stems) in the maps folder, sorted.
fn list_sources(maps_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(maps_dir) {
        for entry in rd.flatten() {
            let p = entry.path();
            match p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
                Some("mbtiles" | "tif" | "tiff") => {
                    if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                        out.push(stem.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn read_mbtiles_meta(path: &Path) -> Option<SourceMeta> {
    use rusqlite::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let get = |k: &str| {
        conn.query_row("SELECT value FROM metadata WHERE name=?1", [k], |r| r.get::<_, String>(0))
            .ok()
    };
    let bounds = get("bounds").and_then(|b| {
        let p: Vec<f64> = b.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        <[f64; 4]>::try_from(p).ok()
    });
    Some(SourceMeta {
        minzoom: get("minzoom").and_then(|s| s.parse().ok()).unwrap_or(0),
        maxzoom: get("maxzoom").and_then(|s| s.parse().ok()).unwrap_or(22),
        bounds: bounds.unwrap_or([-180.0, -85.051_13, 180.0, 85.051_13]),
    })
}

fn read_cog_meta(path: &Path) -> Option<SourceMeta> {
    let src = CogSource::new("s".to_string(), path.to_path_buf(), CacheZoomRange::default()).ok()?;
    let tj = src.get_tilejson();
    let b = tj.bounds.clone()?;
    Some(SourceMeta {
        minzoom: tj.minzoom.unwrap_or(0),
        maxzoom: tj.maxzoom.unwrap_or(22),
        bounds: [b.left, b.bottom, b.right, b.top],
    })
}

fn source_meta(maps_dir: &Path, source: &str) -> Option<SourceMeta> {
    let mbtiles = maps_dir.join(format!("{source}.mbtiles"));
    if mbtiles.is_file() {
        return read_mbtiles_meta(&mbtiles);
    }
    for ext in ["tif", "tiff"] {
        let p = maps_dir.join(format!("{source}.{ext}"));
        if p.is_file() {
            return read_cog_meta(&p);
        }
    }
    None
}

/// TileJSON for a source (so QGIS / clients can add it by one URL).
fn tilejson(maps_dir: &Path, source: &str, base: &str) -> Option<String> {
    let m = source_meta(maps_dir, source)?;
    Some(format!(
        r#"{{"tilejson":"3.0.0","name":"{source}","scheme":"xyz","tiles":["{base}/{source}/{{z}}/{{x}}/{{y}}"],"minzoom":{},"maxzoom":{},"bounds":[{},{},{},{}]}}"#,
        m.minzoom, m.maxzoom, m.bounds[0], m.bounds[1], m.bounds[2], m.bounds[3]
    ))
}

fn landing_html(maps_dir: &Path, base: &str) -> String {
    let sources = list_sources(maps_dir);
    let rows = sources
        .iter()
        .map(|s| {
            format!(
                "<li><b>{s}</b><br><code>{base}/{s}/{{z}}/{{x}}/{{y}}</code> &middot; <a href=\"/{s}.json\">TileJSON</a></li>"
            )
        })
        .collect::<Vec<_>>()
        .join("");
    let empty = if sources.is_empty() { "<p class=muted>No maps yet — generate one in Map Tile Studio.</p>" } else { "" };
    format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">\
         <title>Map Tile Studio — Tile Service</title>\
         <style>body{{font:14px/1.5 system-ui,Segoe UI,sans-serif;margin:40px auto;max-width:760px;color:#161b22;padding:0 16px}}\
         h1{{font-size:19px}}code{{background:#f1f3f5;padding:2px 6px;border-radius:5px;font-size:12px}}\
         ul{{padding:0}}li{{list-style:none;margin:14px 0;border:1px solid #edeef1;border-radius:10px;padding:12px 14px}}\
         a{{color:#2463eb}}.muted{{color:#667085}}.brand{{color:#2463eb;font-weight:600}}</style>\
         <h1>Map Tile Studio <span class=muted>· tile service</span></h1>\
         <p class=muted>{n} map(s). XYZ tiles at <code>{base}/&lbrace;source&rbrace;/&lbrace;z&rbrace;/&lbrace;x&rbrace;/&lbrace;y&rbrace;</code></p>\
         {empty}<ul>{rows}</ul>\
         <p class=muted>Served by <span class=brand>AiGeoLAB</span> · ai-geolab.org</p>",
        n = sources.len()
    )
}

fn host_of(req: &Request) -> String {
    req.headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("host"))
        .map(|h| h.value.as_str().to_string())
        .unwrap_or_else(|| "localhost".to_string())
}

fn respond_str(req: Request, status: u16, content_type: &str, body: String) {
    let resp = Response::from_string(body)
        .with_status_code(StatusCode(status))
        .with_header(header("Content-Type", content_type))
        .with_header(header("Access-Control-Allow-Origin", "*"));
    let _ = req.respond(resp);
}

/// Handle one request: catalog endpoints, then a tile or 204.
fn handle(
    req: Request,
    maps_dir: &Path,
    rt: &tokio::runtime::Runtime,
    cogs: &mut HashMap<PathBuf, CogSource>,
) {
    let cors = || header("Access-Control-Allow-Origin", "*");
    let pna = || header("Access-Control-Allow-Private-Network", "true");

    if req.method() == &Method::Options {
        let resp = Response::from_data(Vec::new())
            .with_status_code(StatusCode(204))
            .with_header(cors())
            .with_header(pna())
            .with_header(header("Access-Control-Allow-Methods", "GET, OPTIONS"))
            .with_header(header("Access-Control-Allow-Headers", "*"));
        let _ = req.respond(resp);
        return;
    }

    let url = req.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();

    // Catalog endpoints (only meaningful for GET).
    if path == "/health" {
        return respond_str(req, 200, "text/plain; charset=utf-8", "ok".to_string());
    }
    if path == "/" || path == "/index.html" {
        let base = format!("http://{}", host_of(&req));
        return respond_str(req, 200, "text/html; charset=utf-8", landing_html(maps_dir, &base));
    }
    if let Some(source) = path.trim_start_matches('/').strip_suffix(".json") {
        if !source.is_empty() && !source.contains(['/', '\\', ':']) && !source.contains("..") {
            let base = format!("http://{}", host_of(&req));
            if let Some(tj) = tilejson(maps_dir, source, &base) {
                return respond_str(req, 200, "application/json", tj);
            }
        }
    }

    let resp = match resolve_tile(&url, maps_dir, rt, cogs) {
        Some(bytes) => {
            let ct = content_type_of(&bytes);
            Response::from_data(bytes)
                .with_header(header("Content-Type", ct))
                .with_header(header("Cache-Control", "no-cache"))
                .with_header(cors())
                .with_header(pna())
        }
        None => Response::from_data(Vec::new())
            .with_status_code(StatusCode(204))
            .with_header(cors())
            .with_header(pna()),
    };
    let _ = req.respond(resp);
}

/// One worker thread: its own COG cache + current-thread runtime (the loop is
/// sequential per thread; concurrency comes from running several of these).
fn worker(server: &Server, maps_dir: &Path) {
    // A bare current-thread runtime is enough: `CogSource::get_tile` does blocking
    // file I/O inside its async body and never awaits a tokio driver.
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("tile-server runtime");
    let mut cogs: HashMap<PathBuf, CogSource> = HashMap::new();
    while let Ok(req) = server.recv() {
        handle(req, maps_dir, &rt, &mut cogs);
    }
}

fn io_err(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
}

/// Number of worker threads to serve with (bounded so a small box isn't swamped).
fn worker_count() -> usize {
    num_cpus::get().clamp(2, 16)
}

/// Bind `addr` and serve forever with a pool of worker threads. Used by the
/// `tile-serviced` background service (e.g. `0.0.0.0:7765`).
pub fn serve_blocking(maps_dir: PathBuf, addr: SocketAddr) -> std::io::Result<()> {
    let server = Arc::new(Server::http(addr).map_err(io_err)?);
    let mut handles = Vec::new();
    for _ in 0..worker_count() {
        let server = Arc::clone(&server);
        let maps = maps_dir.clone();
        handles.push(std::thread::spawn(move || worker(&server, &maps)));
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

/// Spawn a background **loopback** server (for the app's own in-app preview) on
/// the first free port at or after `preferred`, falling back to an ephemeral
/// port. Returns the bound port; worker threads run detached.
pub fn serve_background(maps_dir: PathBuf, preferred: u16) -> std::io::Result<u16> {
    let mut bound = None;
    for p in preferred..preferred.saturating_add(30) {
        if let Ok(s) = Server::http(("127.0.0.1", p)) {
            bound = Some((s, p));
            break;
        }
    }
    let (server, port) = match bound {
        Some(b) => b,
        None => {
            let s = Server::http(("127.0.0.1", 0)).map_err(io_err)?;
            let p = s.server_addr().to_ip().map_or(0, |a| a.port());
            (s, p)
        }
    };
    let server = Arc::new(server);
    for _ in 0..worker_count() {
        let server = Arc::clone(&server);
        let maps = maps_dir.clone();
        std::thread::spawn(move || worker(&server, &maps));
    }
    Ok(port)
}
