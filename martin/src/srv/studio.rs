//! Tile Map Studio — HTTP API that drives the [`martin_tiler`] generation engine.
//!
//! Endpoints (all under `/studio`):
//! - `GET  /studio/config`        — engine capabilities + configured directories
//! - `GET  /studio/browse`        — list source GeoTIFFs in the data directory
//! - `POST /studio/inspect`       — inspect a set of GeoTIFFs (CRS, footprint, native zoom)
//! - `POST /studio/generate`      — start a generation job, returns a job id
//! - `GET  /studio/jobs`          — list jobs
//! - `GET  /studio/jobs/{id}`     — job status, streamed progress log, and final report
//! - `POST /studio/validate`      — validate a generated MBTiles
//!
//! Generation runs in a background task; the frontend polls `GET /studio/jobs/{id}`
//! for live progress. Output MBTiles are written into the studio output directory,
//! which Martin also serves as MBTiles sources (so a generated map is immediately
//! previewable at `/{source-id}/{z}/{x}/{y}`).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use actix_web::web::{self, Data};
use actix_web::{HttpResponse, Responder, route};
use dashmap::DashMap;
use martin_tiler::{
    GdalEnv, GenerateOptions, ProgressEvent, Resampling, TileFormat, TileGrid, generate,
    inspect_many, validate,
};
use serde::{Deserialize, Serialize};

/// Shared state for the studio: the resolved GDAL toolchain, configured directories,
/// and the in-memory registry of generation jobs.
pub struct StudioState {
    gdal: Option<GdalEnv>,
    gdal_error: Option<String>,
    /// Directory the user picks input GeoTIFFs from.
    data_dir: PathBuf,
    /// Directory generated `.mbtiles` are written to (also served by Martin).
    output_dir: PathBuf,
    jobs: DashMap<String, Arc<std::sync::Mutex<Job>>>,
    counter: AtomicU64,
}

impl StudioState {
    /// Build the studio state from environment configuration, discovering GDAL.
    ///
    /// - `MARTIN_STUDIO_DATA_DIR`   (default `./data`)   — where source GeoTIFFs live
    /// - `MARTIN_STUDIO_OUTPUT_DIR` (default `./studio-maps`) — where tile maps are written
    #[must_use]
    pub fn from_env() -> Self {
        let data_dir = std::env::var_os("MARTIN_STUDIO_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("data"));
        let output_dir = std::env::var_os("MARTIN_STUDIO_OUTPUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("studio-maps"));
        // Ensure both dirs exist so a fresh install has somewhere to receive uploads
        // and write generated maps before the first run.
        let _ = std::fs::create_dir_all(&data_dir);
        let _ = std::fs::create_dir_all(&output_dir);

        let (gdal, gdal_error) = match GdalEnv::discover() {
            Ok(g) => {
                tracing::info!("Tile Map Studio: GDAL found at {}", g.bin_dir.display());
                (Some(g), None)
            }
            Err(e) => {
                tracing::warn!("Tile Map Studio: GDAL not available: {e}");
                (None, Some(e.to_string()))
            }
        };

        Self {
            gdal,
            gdal_error,
            data_dir,
            output_dir,
            jobs: DashMap::new(),
            counter: AtomicU64::new(0),
        }
    }

    fn next_job_id(&self) -> String {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        format!("job-{n:04}")
    }
}

/// Status of a generation job.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum JobStatus {
    Running,
    Done,
    Failed,
}

/// A generation job: its progress log and final result.
#[derive(Clone, Serialize)]
struct Job {
    id: String,
    name: String,
    status: JobStatus,
    /// Human-readable progress log lines (the `[stage] message` feed).
    log: Vec<String>,
    /// Current stage 1-based index and total, for a progress bar.
    stage_index: u32,
    stage_total: u32,
    /// Latest within-stage percentage (0–100), if known.
    percent: Option<f64>,
    /// Final report once the job is `done`.
    report: Option<martin_tiler::GenerateReport>,
    /// Error message if the job `failed`.
    error: Option<String>,
}

// ---- request / response DTOs ----------------------------------------------

#[derive(Deserialize)]
struct InspectRequest {
    inputs: Vec<String>,
}

#[derive(Deserialize)]
struct GenerateRequest {
    inputs: Vec<String>,
    name: String,
    #[serde(default)]
    grids: Vec<TileGrid>,
    /// Additional custom projected output grids by EPSG code (e.g. 9680).
    #[serde(default)]
    custom_epsgs: Vec<u32>,
    /// Also produce a single COG served directly by Martin.
    #[serde(default)]
    cog: bool,
    #[serde(default)]
    min_zoom: Option<u8>,
    #[serde(default)]
    max_zoom: Option<u8>,
    #[serde(default)]
    format: Option<TileFormat>,
    #[serde(default)]
    resampling: Option<Resampling>,
    #[serde(default)]
    processes: Option<usize>,
}

#[derive(Deserialize)]
struct ValidateRequest {
    path: String,
}

#[derive(Serialize)]
struct ConfigResponse {
    gdal_available: bool,
    gdal_error: Option<String>,
    gdal_bin: Option<String>,
    data_dir: String,
    output_dir: String,
    grids: Vec<GridInfo>,
    formats: Vec<&'static str>,
    resamplings: Vec<&'static str>,
}

#[derive(Serialize)]
struct GridInfo {
    id: String,
    epsg: u32,
    label: String,
}

#[derive(Serialize)]
struct BrowseEntry {
    name: String,
    /// Path relative to the data directory (what the client passes back as an input).
    rel_path: String,
    size: u64,
}

#[derive(Serialize)]
struct StartJobResponse {
    job_id: String,
}

// ---- helpers ---------------------------------------------------------------

/// Strip the Windows `\\?\` extended-length / verbatim prefix that
/// [`std::fs::canonicalize`] adds, because the GDAL CLI tools cannot open such paths.
fn strip_verbatim_prefix(p: PathBuf) -> PathBuf {
    if cfg!(windows) {
        if let Some(s) = p.to_str() {
            if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
                return PathBuf::from(format!(r"\\{rest}"));
            }
            if let Some(rest) = s.strip_prefix(r"\\?\") {
                return PathBuf::from(rest);
            }
        }
    }
    p
}

/// Resolve a client-supplied input path to an absolute file inside `data_dir`,
/// rejecting attempts to escape the data directory.
fn resolve_input(data_dir: &Path, rel: &str) -> Result<PathBuf, String> {
    let candidate = data_dir.join(rel);
    let canon = candidate
        .canonicalize()
        .map_err(|e| format!("{rel}: {e}"))?;
    let base = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf());
    if !canon.starts_with(&base) {
        return Err(format!("{rel}: path escapes the data directory"));
    }
    if !canon.is_file() {
        return Err(format!("{rel}: not a file"));
    }
    Ok(strip_verbatim_prefix(canon))
}

/// Recursively collect GeoTIFFs under `dir` (bounded depth), relative to `root`.
fn collect_tiffs(root: &Path, dir: &Path, depth: usize, out: &mut Vec<BrowseEntry>) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip our own working/output noise.
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            collect_tiffs(root, &path, depth + 1, out);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("tif") || e.eq_ignore_ascii_case("tiff"))
        {
            let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            out.push(BrowseEntry {
                name: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                rel_path: rel.to_string_lossy().replace('\\', "/"),
                size: entry.metadata().map(|m| m.len()).unwrap_or(0),
            });
        }
    }
}

fn studio_error(msg: impl Into<String>) -> HttpResponse {
    HttpResponse::ServiceUnavailable().json(serde_json::json!({ "error": msg.into() }))
}

/// Max upload size for a single GeoTIFF (bytes). Default 2 GiB.
/// Override with `MARTIN_STUDIO_MAX_UPLOAD_BYTES`.
fn max_upload_bytes() -> usize {
    std::env::var("MARTIN_STUDIO_MAX_UPLOAD_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2 * 1024 * 1024 * 1024)
}

/// Sanitize a client-supplied upload filename into a safe, flat `.tif`/`.tiff` name.
///
/// Strips directory components (defeats `../`, absolute paths, drive letters),
/// enforces a `.tif`/`.tiff` extension, and whitelists characters to `[A-Za-z0-9._-]`.
fn sanitize_upload_name(raw: &str) -> Result<String, String> {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    if base.is_empty() || base == "." || base == ".." {
        return Err("invalid filename".into());
    }
    let lower = base.to_ascii_lowercase();
    if !(lower.ends_with(".tif") || lower.ends_with(".tiff")) {
        return Err("only .tif/.tiff files are accepted".into());
    }
    let cleaned: String = base
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect();
    if cleaned.trim_matches(['.', '_', '-']).is_empty() {
        return Err("invalid filename".into());
    }
    Ok(cleaned)
}

/// Pick a non-colliding destination filename inside `dir` for `name`.
fn unique_dest(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_string(), format!(".{e}")),
        None => (name.to_string(), String::new()),
    };
    for n in 1u32.. {
        let candidate = dir.join(format!("{stem}-{n}{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(name)
}

#[derive(Deserialize)]
struct UploadQuery {
    name: String,
}

#[derive(Serialize)]
struct UploadResponse {
    name: String,
    rel_path: String,
    size: usize,
}

// ---- handlers --------------------------------------------------------------

#[route("/studio/config", method = "GET")]
pub async fn get_studio_config(state: Data<StudioState>) -> impl Responder {
    let resp = ConfigResponse {
        gdal_available: state.gdal.is_some(),
        gdal_error: state.gdal_error.clone(),
        gdal_bin: state.gdal.as_ref().map(|g| g.bin_dir.display().to_string()),
        data_dir: state.data_dir.display().to_string(),
        output_dir: state.output_dir.display().to_string(),
        grids: vec![
            GridInfo { id: TileGrid::WebMercator.slug(), epsg: TileGrid::WebMercator.epsg(), label: TileGrid::WebMercator.label() },
            GridInfo { id: TileGrid::Geodetic.slug(), epsg: TileGrid::Geodetic.epsg(), label: TileGrid::Geodetic.label() },
        ],
        formats: vec!["png", "webp"],
        resamplings: vec!["near", "bilinear", "cubic", "average", "lanczos"],
    };
    HttpResponse::Ok().json(resp)
}

#[route("/studio/browse", method = "GET")]
pub async fn browse(state: Data<StudioState>) -> impl Responder {
    let mut entries = Vec::new();
    collect_tiffs(&state.data_dir, &state.data_dir, 0, &mut entries);
    entries.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    HttpResponse::Ok().json(entries)
}

/// Accept a single GeoTIFF uploaded as a raw `application/octet-stream` body and
/// write it into the studio data directory.
///
/// `POST /studio/upload?name=<filename.tif>` with the file bytes as the request body.
/// The filename is sanitized to a flat `.tif`/`.tiff` name inside `data_dir`
/// (path-traversal safe); collisions get a numeric suffix.
#[route("/studio/upload", method = "POST")]
pub async fn upload(
    state: Data<StudioState>,
    query: web::Query<UploadQuery>,
    body: web::Bytes,
) -> impl Responder {
    let name = match sanitize_upload_name(&query.name) {
        Ok(n) => n,
        Err(e) => return HttpResponse::BadRequest().json(serde_json::json!({ "error": e })),
    };
    if body.is_empty() {
        return HttpResponse::BadRequest().json(serde_json::json!({ "error": "empty upload" }));
    }
    if body.len() > max_upload_bytes() {
        return HttpResponse::PayloadTooLarge()
            .json(serde_json::json!({ "error": "file exceeds the upload size limit" }));
    }
    if let Err(e) = std::fs::create_dir_all(&state.data_dir) {
        return studio_error(format!("could not create data directory: {e}"));
    }

    let dest = unique_dest(&state.data_dir, &name);
    let dest_name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| name.clone());

    // Defense in depth: confirm the resolved destination stays inside data_dir.
    let base = state.data_dir.canonicalize().unwrap_or_else(|_| state.data_dir.clone());
    if let Some(parent) = dest.parent() {
        let parent_canon = parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf());
        if !parent_canon.starts_with(&base) {
            return HttpResponse::BadRequest()
                .json(serde_json::json!({ "error": "destination escapes the data directory" }));
        }
    }

    let size = body.len();
    let dest_for_task = dest.clone();
    let write_result = web::block(move || std::fs::write(&dest_for_task, &body)).await;
    match write_result {
        Ok(Ok(())) => HttpResponse::Created().json(UploadResponse {
            name: dest_name.clone(),
            rel_path: dest_name,
            size,
        }),
        Ok(Err(e)) => studio_error(format!("write failed: {e}")),
        Err(e) => studio_error(format!("write task failed: {e}")),
    }
}

#[route("/studio/inspect", method = "POST")]
pub async fn inspect(state: Data<StudioState>, body: web::Json<InspectRequest>) -> impl Responder {
    let Some(gdal) = state.gdal.as_ref() else {
        return studio_error(state.gdal_error.clone().unwrap_or_else(|| "GDAL not available".into()));
    };
    let mut paths = Vec::new();
    for rel in &body.inputs {
        match resolve_input(&state.data_dir, rel) {
            Ok(p) => paths.push(p),
            Err(e) => return HttpResponse::BadRequest().json(serde_json::json!({ "error": e })),
        }
    }
    match inspect_many(gdal, &paths).await {
        Ok(infos) => HttpResponse::Ok().json(infos),
        Err(e) => studio_error(e.to_string()),
    }
}

#[route("/studio/validate", method = "POST")]
pub async fn validate_map(_state: Data<StudioState>, body: web::Json<ValidateRequest>) -> impl Responder {
    match validate(Path::new(&body.path)).await {
        Ok(report) => HttpResponse::Ok().json(report),
        Err(e) => studio_error(e.to_string()),
    }
}

#[route("/studio/jobs", method = "GET")]
pub async fn list_jobs(state: Data<StudioState>) -> impl Responder {
    let mut jobs: Vec<Job> = state
        .jobs
        .iter()
        .filter_map(|e| e.value().lock().ok().map(|j| j.clone()))
        .collect();
    jobs.sort_by(|a, b| a.id.cmp(&b.id));
    HttpResponse::Ok().json(jobs)
}

#[route("/studio/jobs/{id}", method = "GET")]
pub async fn get_job(state: Data<StudioState>, id: web::Path<String>) -> impl Responder {
    match state.jobs.get(id.as_str()).and_then(|e| e.value().lock().ok().map(|j| j.clone())) {
        Some(job) => HttpResponse::Ok().json(job),
        None => HttpResponse::NotFound().json(serde_json::json!({ "error": "no such job" })),
    }
}

#[route("/studio/generate", method = "POST")]
pub async fn start_generate(state: Data<StudioState>, body: web::Json<GenerateRequest>) -> impl Responder {
    let Some(gdal) = state.gdal.clone() else {
        return studio_error(state.gdal_error.clone().unwrap_or_else(|| "GDAL not available".into()));
    };
    let req = body.into_inner();
    if req.inputs.is_empty() {
        return HttpResponse::BadRequest().json(serde_json::json!({ "error": "no inputs" }));
    }

    // Resolve + validate every input path inside the data directory.
    let mut inputs = Vec::new();
    for rel in &req.inputs {
        match resolve_input(&state.data_dir, rel) {
            Ok(p) => inputs.push(p),
            Err(e) => return HttpResponse::BadRequest().json(serde_json::json!({ "error": e })),
        }
    }

    // Default to Web Mercator only when nothing else was requested (no grids, no custom
    // EPSGs, and not a COG-only run).
    let mut grids = if req.grids.is_empty() && req.custom_epsgs.is_empty() && !req.cog {
        vec![TileGrid::WebMercator]
    } else {
        req.grids.clone()
    };
    grids.extend(req.custom_epsgs.iter().copied().map(TileGrid::Custom));
    let opts = GenerateOptions {
        inputs,
        output_dir: state.output_dir.clone(),
        name: req.name.clone(),
        grids,
        min_zoom: req.min_zoom,
        max_zoom: req.max_zoom,
        format: req.format.unwrap_or_default(),
        resampling: req.resampling.unwrap_or_default(),
        processes: req.processes,
        cog: req.cog,
        keep_intermediate: false,
    };

    let job_id = state.next_job_id();
    let job = Arc::new(std::sync::Mutex::new(Job {
        id: job_id.clone(),
        name: req.name,
        status: JobStatus::Running,
        log: Vec::new(),
        stage_index: 0,
        stage_total: 0,
        percent: None,
        report: None,
        error: None,
    }));
    state.jobs.insert(job_id.clone(), job.clone());

    // Run the generation in the background; the handler returns immediately.
    let job_for_task = job.clone();
    actix_web::rt::spawn(async move {
        let progress_job = job_for_task.clone();
        let result = generate(&gdal, &opts, move |ev| {
            if let Ok(mut j) = progress_job.lock() {
                apply_event(&mut j, &ev);
            }
        })
        .await;

        if let Ok(mut j) = job_for_task.lock() {
            match result {
                Ok(report) => {
                    j.status = JobStatus::Done;
                    j.report = Some(report);
                }
                Err(e) => {
                    j.status = JobStatus::Failed;
                    j.error = Some(e.to_string());
                    j.log.push(format!("error: {e}"));
                }
            }
        }
    });

    HttpResponse::Accepted().json(StartJobResponse { job_id })
}

/// Fold a streamed [`ProgressEvent`] into the job's mutable state.
fn apply_event(job: &mut Job, ev: &ProgressEvent) {
    match ev {
        ProgressEvent::Stage { stage, index, total, grid } => {
            job.stage_index = *index;
            job.stage_total = *total;
            job.percent = None;
            let g = grid.map(|g| format!(" [{}]", g.slug())).unwrap_or_default();
            job.log.push(format!("▶ stage {index}/{total}: {stage}{g}"));
        }
        ProgressEvent::Percent { percent, .. } => {
            job.percent = Some(*percent);
        }
        ProgressEvent::Log { message } => {
            job.log.push(message.clone());
            // Keep the log bounded.
            if job.log.len() > 500 {
                let drop = job.log.len() - 500;
                job.log.drain(0..drop);
            }
        }
        ProgressEvent::Done { .. } => {}
        ProgressEvent::Failed { error } => {
            job.error = Some(error.clone());
        }
    }
}
