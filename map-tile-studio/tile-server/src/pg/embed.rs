//! Lifecycle of the app's **bundled** PostgreSQL + PostGIS cluster.
//!
//! The portable binaries ship beside the executable in `pgsql/` (bin, lib, share);
//! the data cluster is created on first run under the app data dir. All four
//! callers (GUI startup, the headless `tile-serviced`, the import command, and the
//! registry) funnel through [`PgEmbed::ensure_running`], which is idempotent — if a
//! cluster is already up on the port it is left alone.

use std::io;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use super::config::PgConnection;

/// Resolved paths for the bundled cluster.
#[derive(Clone, Debug)]
pub struct PgEmbed {
    /// `…/pgsql` — the portable PostgreSQL install (bin, lib, share).
    pub root: PathBuf,
    /// `…/pgsql/bin`.
    pub bin: PathBuf,
    /// The data cluster directory (created by `initdb`).
    pub data: PathBuf,
    /// PROJ search dir (holds `proj.db`) so `ST_Transform` can reproject.
    pub proj_lib: PathBuf,
}

fn err(msg: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::Other, msg.to_string())
}

/// True if `dir` exists and contains at least one entry.
fn dir_non_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut rd| rd.next().is_some())
}

/// Compare a path against a path-string reported by PostgreSQL (`SHOW
/// data_directory`), tolerating slash + case differences on Windows.
fn same_dir(a: &Path, b: &str) -> bool {
    if let (Ok(x), Ok(y)) = (std::fs::canonicalize(a), std::fs::canonicalize(Path::new(b))) {
        return x == y;
    }
    let norm = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_lowercase();
    norm(&a.to_string_lossy()) == norm(b)
}

/// Build a `Command` for a bundled tool, suppressing the console window on Windows.
fn tool(bin: &Path, exe: &str) -> Command {
    let name = if cfg!(windows) { format!("{exe}.exe") } else { exe.to_string() };
    let mut c = Command::new(bin.join(name));
    c.stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        c.creation_flags(CREATE_NO_WINDOW);
    }
    c
}

/// Locate the PROJ data dir inside a portable PostGIS install
/// (`share/contrib/postgis-*/proj`), falling back to a sensible default.
fn find_proj_lib(root: &Path) -> PathBuf {
    let contrib = root.join("share").join("contrib");
    if let Ok(rd) = std::fs::read_dir(&contrib) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("postgis-"))
            {
                let proj = p.join("proj");
                if proj.join("proj.db").is_file() {
                    return proj;
                }
            }
        }
    }
    contrib.join("postgis-3.6").join("proj")
}

impl PgEmbed {
    /// Resolve paths from the portable install root and the data dir.
    #[must_use]
    pub fn new(root: PathBuf, data: PathBuf) -> Self {
        let bin = root.join("bin");
        let proj_lib = find_proj_lib(&root);
        Self { root, bin, data, proj_lib }
    }

    /// True when the bundled binaries are actually present (so we can manage a cluster).
    #[must_use]
    pub fn binaries_present(&self) -> bool {
        self.bin.join("postgres.exe").is_file() || self.bin.join("postgres").is_file()
    }

    /// True when the data cluster has been initialised.
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.data.join("PG_VERSION").is_file()
    }

    /// True when something is already listening on the loopback port.
    #[must_use]
    pub fn is_running(&self, port: u16) -> bool {
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        TcpStream::connect_timeout(&addr, Duration::from_millis(600)).is_ok()
    }

    /// `initdb` a fresh cluster with scram auth + the given superuser password,
    /// then grant local users full access to the data dir (so both the GUI user
    /// and a LocalSystem service can manage it).
    pub fn initdb(&self, user: &str, password: &str) -> io::Result<()> {
        std::fs::create_dir_all(&self.data)?;
        let pwfile = self.data.with_extension("pw.txt");
        std::fs::write(&pwfile, password)?;
        let status = tool(&self.bin, "initdb")
            .arg("-D")
            .arg(&self.data)
            .arg("-U")
            .arg(user)
            .arg("-A")
            .arg("scram-sha-256")
            .arg(format!("--pwfile={}", pwfile.display()))
            .arg("-E")
            .arg("UTF8")
            .arg("--locale=C")
            .status();
        let _ = std::fs::remove_file(&pwfile);
        match status {
            Ok(s) if s.success() => {}
            Ok(s) => return Err(err(format!("initdb failed: {s}"))),
            Err(e) => return Err(err(format!("initdb could not start: {e}"))),
        }
        self.grant_local_access();
        Ok(())
    }

    /// Grant the local Users group full control of the data dir (best effort) so the
    /// cluster works whether it was created by the desktop user or a system service.
    fn grant_local_access(&self) {
        #[cfg(windows)]
        {
            // *S-1-5-32-545 = BUILTIN\Users; (OI)(CI)F = inherit + full.
            let mut c = Command::new("icacls");
            c.arg(&self.data)
                .arg("/grant")
                .arg("*S-1-5-32-545:(OI)(CI)F")
                .arg("/T")
                .arg("/Q");
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
            let _ = c.status();
        }
    }

    /// Append our port/listen overrides to `postgresql.conf` (idempotent: only once).
    pub fn configure(&self, port: u16) -> io::Result<()> {
        let conf = self.data.join("postgresql.conf");
        let body = std::fs::read_to_string(&conf).unwrap_or_default();
        if body.contains("# map-tile-studio") {
            return Ok(());
        }
        let extra = format!(
            "\n# map-tile-studio\nport = {port}\nlisten_addresses = '127.0.0.1'\nmax_connections = 50\n"
        );
        std::fs::write(&conf, format!("{body}{extra}"))
    }

    /// Start the cluster with `pg_ctl -w` (waits until it accepts connections),
    /// exporting `PROJ_LIB` so reprojection works.
    pub fn start(&self, port: u16) -> io::Result<()> {
        let log = self.data.join("server.log");
        let status = tool(&self.bin, "pg_ctl")
            .env("PROJ_LIB", &self.proj_lib)
            .arg("-D")
            .arg(&self.data)
            .arg("-l")
            .arg(&log)
            .arg("-o")
            .arg(format!("-p {port}"))
            .arg("-w")
            .arg("start")
            .status()
            .map_err(|e| err(format!("pg_ctl start could not run: {e}")))?;
        if status.success() {
            Ok(())
        } else {
            Err(err(format!(
                "pg_ctl start failed: {status}. Port {port} may already be in use by another PostgreSQL. See {}",
                log.display()
            )))
        }
    }

    /// Stop the cluster (fast shutdown). Best effort.
    pub fn stop(&self) -> io::Result<()> {
        let _ = tool(&self.bin, "pg_ctl")
            .arg("-D")
            .arg(&self.data)
            .arg("-m")
            .arg("fast")
            .arg("-w")
            .arg("stop")
            .status();
        Ok(())
    }

    /// Create the target database (if missing) and enable PostGIS, via bundled `psql`.
    pub fn ensure_database(&self, conn: &PgConnection) -> io::Result<()> {
        let port = conn.port.to_string();
        let psql = |db: &str, sql: &str| -> io::Result<std::process::Output> {
            tool(&self.bin, "psql")
                .env("PGPASSWORD", &conn.password)
                .env("PROJ_LIB", &self.proj_lib)
                .arg("-h")
                .arg("127.0.0.1")
                .arg("-p")
                .arg(&port)
                .arg("-U")
                .arg(&conn.user)
                .arg("-d")
                .arg(db)
                .arg("-tAc")
                .arg(sql)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
        };

        // Sentinel: confirm the server answering on this port is OUR cluster before
        // we createdb / CREATE EXTENSION on it. Prevents mutating an unrelated
        // PostgreSQL that happens to occupy the port.
        let dd = psql("postgres", "SHOW data_directory")?;
        if !dd.status.success() {
            return Err(err(format!(
                "could not reach the bundled PostgreSQL on port {}: {}",
                conn.port,
                String::from_utf8_lossy(&dd.stderr).trim()
            )));
        }
        let server_dir = String::from_utf8_lossy(&dd.stdout).trim().to_string();
        if !same_dir(&self.data, &server_dir) {
            return Err(err(format!(
                "port {} is in use by a different PostgreSQL (data dir: {server_dir}). \
                 The bundled cluster was not started; change its port in connections.json \
                 or stop the other server.",
                conn.port
            )));
        }

        // Create the database if it does not yet exist.
        let exists = psql(
            "postgres",
            &format!(
                "SELECT 1 FROM pg_database WHERE datname='{}'",
                conn.dbname.replace('\'', "''")
            ),
        )?;
        let exists = String::from_utf8_lossy(&exists.stdout).trim() == "1";
        if !exists {
            let out = tool(&self.bin, "createdb")
                .env("PGPASSWORD", &conn.password)
                .arg("-h")
                .arg("127.0.0.1")
                .arg("-p")
                .arg(&port)
                .arg("-U")
                .arg(&conn.user)
                .arg(&conn.dbname)
                .stderr(Stdio::piped())
                .output()?;
            if !out.status.success() {
                return Err(err(format!(
                    "createdb {} failed: {}",
                    conn.dbname,
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
        }

        // Enable PostGIS (idempotent).
        let out = psql(&conn.dbname, "CREATE EXTENSION IF NOT EXISTS postgis")?;
        if !out.status.success() {
            return Err(err(format!(
                "enabling postgis failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Bring the bundled cluster fully online: init + configure if new, start if down,
    /// then ensure the database + PostGIS extension exist. Idempotent and cheap when
    /// already running.
    pub fn ensure_running(&self, conn: &PgConnection) -> io::Result<()> {
        if !self.binaries_present() {
            return Err(err(format!(
                "bundled PostgreSQL not found at {}",
                self.root.display()
            )));
        }
        if self.is_running(conn.port) {
            // Already up (possibly started by the GUI or the service): make sure the
            // database + extension exist, then we are done.
            return self.ensure_database(conn);
        }
        if self.is_initialized() {
            self.configure(conn.port)?;
        } else {
            // A non-empty data dir without PG_VERSION is a half-initialised cluster
            // (interrupted initdb / AV quarantine). initdb would refuse the dir, so
            // surface an actionable error instead of looping on a confusing failure.
            if dir_non_empty(&self.data) {
                return Err(err(format!(
                    "the PostgreSQL data directory looks broken (no PG_VERSION but not empty): {}. \
                     Delete that folder and restart the app to recreate the database.",
                    self.data.display()
                )));
            }
            self.initdb(&conn.user, &conn.password)?;
            self.configure(conn.port)?;
        }
        self.start(conn.port)?;
        self.ensure_database(conn)
    }
}

/// Default location of the bundled install (`<exe dir>/pgsql`).
#[must_use]
pub fn default_root() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("pgsql")
}

/// Default data dir (`<app data>/pgdata`), derived from the maps directory.
#[must_use]
pub fn default_data_dir(maps_dir: &Path) -> PathBuf {
    maps_dir.parent().unwrap_or(maps_dir).join("pgdata")
}
