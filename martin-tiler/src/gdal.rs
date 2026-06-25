//! Locating the GDAL toolchain and running its command-line tools.
//!
//! The engine shells out to the proven GDAL utilities (`gdalinfo`, `gdalbuildvrt`,
//! `gdalwarp`, `gdal2tiles`) rather than re-implementing reprojection/resampling.
//! This module hides all the platform-specific pain of finding an OSGeo4W / system
//! install and wiring up `GDAL_DATA`, `PROJ_LIB` and (for `gdal2tiles`) `PYTHONHOME`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt as _, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::error::{TilerError, TilerResult};

/// A resolved GDAL installation: where its tools live and the environment they need.
#[derive(Clone, Debug)]
pub struct GdalEnv {
    /// Directory containing `gdalinfo`, `gdalwarp`, ... (the GDAL `bin`).
    pub bin_dir: PathBuf,
    /// Resolved `gdal2tiles` launcher (an `.exe`, a `.py`, or a bare name).
    pub gdal2tiles: PathBuf,
    /// A Python interpreter for running `gdal2tiles` as `python -m osgeo_utils.gdal2tiles`,
    /// which is relocation-safe (the `gdal2tiles.exe` launcher has a hardcoded shebang).
    pub python: Option<PathBuf>,
    /// Extra environment variables every GDAL command needs.
    pub env: BTreeMap<String, String>,
}

/// Append the platform executable suffix (`.exe` on Windows) to a tool name.
fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Read immediate subdirectories of `dir` whose name starts with `prefix` (cheap glob).
fn subdirs_starting_with(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir())
                && entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with(prefix))
            {
                out.push(entry.path());
            }
        }
    }
    out
}

/// Candidate GDAL `bin` directories, most-specific first.
fn candidate_bin_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    if let Ok(b) = std::env::var("MARTIN_GDAL_BIN") {
        dirs.push(PathBuf::from(b));
    }
    if let Ok(p) = std::env::var("MARTIN_GDAL_PREFIX") {
        dirs.push(PathBuf::from(p).join("bin"));
    }
    // Anything already on PATH.
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            dirs.push(dir);
        }
    }
    // Well-known install locations.
    let known: &[&str] = if cfg!(windows) {
        &[
            r"C:\OSGeo4W\bin",
            r"C:\OSGeo4W64\bin",
            r"C:\Program Files\GDAL",
            r"C:\Program Files\QGIS\bin",
        ]
    } else {
        &[
            "/usr/bin",
            "/usr/local/bin",
            "/opt/homebrew/bin",
            "/opt/local/bin",
        ]
    };
    dirs.extend(known.iter().map(PathBuf::from));
    dirs
}

/// Pick the first existing path from `candidates`.
fn first_existing(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|p| p.exists())
}

impl GdalEnv {
    /// Discover a usable GDAL installation, or explain what was tried.
    pub fn discover() -> TilerResult<Self> {
        let gdalinfo = exe("gdalinfo");
        let mut tried = Vec::new();

        let bin_dir = candidate_bin_dirs()
            .into_iter()
            .find(|d| {
                tried.push(d.display().to_string());
                d.join(&gdalinfo).exists()
            })
            .ok_or_else(|| TilerError::GdalNotFound(tried.join(", ")))?;

        let prefix = bin_dir.parent().unwrap_or(&bin_dir).to_path_buf();

        let mut env = BTreeMap::new();

        // GDAL_DATA: keep an existing valid value, else derive from the prefix.
        if let Some(dir) = std::env::var_os("GDAL_DATA").map(PathBuf::from).filter(|p| p.exists()) {
            env.insert("GDAL_DATA".into(), dir.display().to_string());
        } else if let Some(dir) = first_existing([
            prefix.join("apps/gdal/share/gdal"),
            prefix.join("share/gdal"),
            prefix.join("share/gdal"),
            bin_dir.join("../share/gdal"),
        ]) {
            env.insert("GDAL_DATA".into(), dir.display().to_string());
        }

        // PROJ_LIB / PROJ_DATA: needs the directory containing proj.db.
        if let Some(dir) = ["PROJ_LIB", "PROJ_DATA"]
            .iter()
            .filter_map(|k| std::env::var_os(k))
            .map(PathBuf::from)
            .find(|p| p.join("proj.db").exists())
        {
            env.insert("PROJ_LIB".into(), dir.display().to_string());
        } else if let Some(dir) = first_existing([
            prefix.join("share/proj"),
            prefix.join("apps/proj/share/proj"),
            bin_dir.join("../share/proj"),
        ]) {
            env.insert("PROJ_LIB".into(), dir.display().to_string());
        }

        // Discover a Python interpreter for gdal2tiles. We prefer running
        // `python -m osgeo_utils.gdal2tiles` because it is relocation-safe — the
        // `gdal2tiles.exe`/`.py` launchers carry a hardcoded shebang to the original
        // install and break when the GDAL tree is copied into a portable bundle.
        let mut pythonhome: Option<PathBuf> = None;
        let mut python_exe: Option<PathBuf> = None;
        let set_python = |home: PathBuf, exe_path: PathBuf| (Some(home), Some(exe_path));
        // 1) explicit override
        if let Some(p) = std::env::var_os("MARTIN_GDAL_PYTHON").map(PathBuf::from).filter(|p| p.exists())
        {
            let home = p.parent().map_or_else(|| p.clone(), Path::to_path_buf);
            (pythonhome, python_exe) = set_python(home, p);
        }
        // 2) OSGeo4W layout: <prefix>/apps/Python*/python.exe
        if python_exe.is_none() && cfg!(windows) {
            for py in subdirs_starting_with(&prefix.join("apps"), "Python") {
                let cand = py.join(exe("python"));
                if cand.exists() {
                    (pythonhome, python_exe) = set_python(py, cand);
                    break;
                }
            }
        }
        // 3) Portable bundle layout: a `python/` dir next to the GDAL prefix (ROOT/python)
        if python_exe.is_none() {
            if let Some(root) = prefix.parent() {
                let home = root.join("python");
                let cand = home.join(exe("python"));
                if cand.exists() {
                    (pythonhome, python_exe) = set_python(home, cand);
                }
            }
        }
        // 4) PYTHONHOME env
        if python_exe.is_none() {
            if let Some(ph) = std::env::var_os("PYTHONHOME").map(PathBuf::from) {
                let cand = ph.join(exe("python"));
                if cand.exists() {
                    (pythonhome, python_exe) = set_python(ph, cand);
                }
            }
        }

        // gdal2tiles.exe/.py launcher, used only if no python interpreter was found.
        let mut g2t_candidates: Vec<PathBuf> = Vec::new();
        if cfg!(windows) {
            for py in subdirs_starting_with(&prefix.join("apps"), "Python") {
                g2t_candidates.push(py.join("Scripts").join(exe("gdal2tiles")));
            }
        }
        g2t_candidates.push(bin_dir.join(exe("gdal2tiles")));
        g2t_candidates.push(bin_dir.join("gdal2tiles.py"));
        g2t_candidates.push(bin_dir.join("gdal2tiles"));
        let gdal2tiles = first_existing(g2t_candidates).unwrap_or_else(|| {
            PathBuf::from(if cfg!(windows) { "gdal2tiles.py" } else { "gdal2tiles" })
        });

        if let Some(ph) = &pythonhome {
            env.insert("PYTHONHOME".into(), ph.display().to_string());
        }
        // CPython 3.8+ disables the legacy PATH-based DLL search for extension modules;
        // this tells the osgeo bindings to honor PATH so `_gdal.pyd` finds the GDAL DLLs.
        env.insert("USE_PATH_FOR_GDAL_PYTHON".into(), "YES".into());
        // Keep PROJ offline by default (no CDN grid fetches that could hang behind proxies).
        env.entry("PROJ_NETWORK".into()).or_insert_with(|| "OFF".into());

        // Make sure the GDAL bin dir (DLLs, helper exes) is first on PATH for child processes.
        let existing_path = std::env::var("PATH").unwrap_or_default();
        let sep = if cfg!(windows) { ";" } else { ":" };
        env.insert(
            "PATH".into(),
            format!("{}{sep}{existing_path}", bin_dir.display()),
        );

        Ok(Self {
            bin_dir,
            gdal2tiles,
            python: python_exe,
            env,
        })
    }

    /// The command and leading args to run gdal2tiles. Prefers
    /// `python -m osgeo_utils.gdal2tiles` (relocation-safe), falling back to the launcher.
    #[must_use]
    pub fn gdal2tiles_invocation(&self) -> (PathBuf, Vec<String>) {
        if let Some(py) = &self.python {
            (py.clone(), vec!["-m".to_string(), "osgeo_utils.gdal2tiles".to_string()])
        } else {
            (self.gdal2tiles.clone(), Vec::new())
        }
    }

    /// Absolute path of a GDAL `bin` tool (with the platform exe suffix).
    #[must_use]
    pub fn tool(&self, name: &str) -> PathBuf {
        self.bin_dir.join(exe(name))
    }

    /// Apply the discovered environment to a command.
    fn apply_env(&self, cmd: &mut Command) {
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
    }

    /// Run a command to completion, capturing all of stdout (used for `gdalinfo -json`).
    pub async fn run_capture(&self, program: &Path, args: &[String]) -> TilerResult<String> {
        let mut cmd = Command::new(program);
        cmd.args(args);
        self.apply_env(&mut cmd);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let output = cmd.output().await?;
        if !output.status.success() {
            return Err(TilerError::CommandFailed {
                cmd: format!("{} {}", program.display(), args.join(" ")),
                status: output.status.to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Run a command, streaming every output line to `on_line`, and fail on non-zero exit.
    ///
    /// stdout and stderr are merged so progress lines (which GDAL writes to either)
    /// all reach the callback in order; the tail of the output is kept for error reports.
    pub async fn run_streaming(
        &self,
        program: &Path,
        args: &[String],
        mut on_line: impl FnMut(&str),
    ) -> TilerResult<()> {
        let mut cmd = Command::new(program);
        cmd.args(args);
        self.apply_env(&mut cmd);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            TilerError::Other(format!("failed to launch {}: {e}", program.display()))
        })?;

        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let tx2 = tx.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(line);
            }
        });
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx2.send(line);
            }
        });

        // Keep the last lines of output for diagnostics.
        let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        while let Some(line) = rx.recv().await {
            if tail.len() >= 40 {
                tail.pop_front();
            }
            tail.push_back(line.clone());
            on_line(&line);
        }

        let status = child.wait().await?;
        if !status.success() {
            return Err(TilerError::CommandFailed {
                cmd: format!("{} {}", program.display(), args.join(" ")),
                status: status.to_string(),
                stderr: tail.into_iter().collect::<Vec<_>>().join("\n"),
            });
        }
        Ok(())
    }
}

/// Best-effort parse of a trailing GDAL dotted-progress percentage (`"... 60 ..."`) from a line.
#[must_use]
pub fn parse_progress_percent(line: &str) -> Option<f64> {
    // GDAL prints progress as `0...10...20...30...` and finally `... - done`.
    let last = line
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<u32>().ok())
        .filter(|n| *n <= 100)
        .next_back()?;
    Some(f64::from(last))
}
