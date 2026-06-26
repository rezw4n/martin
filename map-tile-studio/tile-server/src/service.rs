//! Install / control / run the tile server as an OS background service.
//!
//! Windows → a native Windows Service (auto-start, runs as LocalSystem, survives
//! reboot, no login needed). Linux → a `systemd` unit. The public API is the same
//! on both; the platform specifics live in `imp`.

use std::net::SocketAddr;
use std::path::PathBuf;

/// Windows service key name.
pub const SERVICE_NAME: &str = "MapTileStudioTiles";
/// systemd unit + human display name.
pub const UNIT_NAME: &str = "maptilestudio-tiles";
pub const DISPLAY_NAME: &str = "Map Tile Studio Tile Service";

/// Register + start the service (auto-start on boot). Needs admin/root.
pub fn install(maps_dir: PathBuf, bind: SocketAddr) -> Result<(), String> {
    imp::install(maps_dir, bind)
}

/// Stop + remove the service. Needs admin/root.
pub fn uninstall() -> Result<(), String> {
    imp::uninstall()
}

/// Start (`true`) or stop (`false`) an already-installed service.
pub fn set_running(start: bool) -> Result<(), String> {
    imp::set_running(start)
}

/// Human-readable service state (e.g. "running", "stopped", "not installed").
pub fn status() -> String {
    imp::status()
}

/// Entry point used when the OS service controller starts us (Windows only).
#[cfg(windows)]
pub fn run_as_service(maps_dir: PathBuf, bind: SocketAddr) -> Result<(), String> {
    imp::run_as_service(maps_dir, bind)
}

/* ──────────────────────────────── Windows ──────────────────────────────── */
#[cfg(windows)]
mod imp {
    use super::{SocketAddr, PathBuf, SERVICE_NAME, DISPLAY_NAME};
    use std::ffi::OsString;
    use std::sync::mpsc;
    use std::sync::OnceLock;
    use std::time::Duration;

    use windows_service::service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_dispatcher;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    const FIREWALL_RULE: &str = "MapTileStudioTiles";
    const NO_WINDOW: u32 = 0x0800_0000;

    /// Allow inbound TCP on the service port so other LAN machines can connect.
    fn open_firewall(port: u16) {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "delete",
                "rule",
                &format!("name={FIREWALL_RULE}"),
            ])
            .creation_flags(NO_WINDOW)
            .status();
        let _ = std::process::Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                &format!("name={FIREWALL_RULE}"),
                "dir=in",
                "action=allow",
                "protocol=TCP",
                &format!("localport={port}"),
            ])
            .creation_flags(NO_WINDOW)
            .status();
    }

    fn close_firewall() {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "delete",
                "rule",
                &format!("name={FIREWALL_RULE}"),
            ])
            .creation_flags(NO_WINDOW)
            .status();
    }

    fn manager(access: ServiceManagerAccess) -> Result<ServiceManager, String> {
        ServiceManager::local_computer(None::<&str>, access)
            .map_err(|e| format!("cannot access the Windows service manager — run as Administrator ({e})"))
    }

    pub fn install(maps_dir: PathBuf, bind: SocketAddr) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let info = ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe,
            launch_arguments: vec![
                OsString::from("service-run"),
                OsString::from("--maps"),
                maps_dir.into_os_string(),
                OsString::from("--bind"),
                OsString::from(bind.to_string()),
            ],
            dependencies: vec![],
            account_name: None, // LocalSystem
            account_password: None,
        };
        let mgr = manager(ServiceManagerAccess::CREATE_SERVICE)?;
        let service = mgr
            .create_service(&info, ServiceAccess::CHANGE_CONFIG | ServiceAccess::START)
            .map_err(|e| format!("create service (run as Administrator?): {e}"))?;
        let _ = service.set_description("Serves Map Tile Studio tile maps over HTTP (XYZ tiles).");
        service.start::<OsString>(&[]).map_err(|e| e.to_string())?;
        open_firewall(bind.port());
        Ok(())
    }

    pub fn uninstall() -> Result<(), String> {
        let mgr = manager(ServiceManagerAccess::CONNECT)?;
        let service = mgr
            .open_service(
                SERVICE_NAME,
                ServiceAccess::STOP | ServiceAccess::DELETE | ServiceAccess::QUERY_STATUS,
            )
            .map_err(|e| format!("open service (run as Administrator?): {e}"))?;
        let _ = service.stop();
        service.delete().map_err(|e| e.to_string())?;
        close_firewall();
        Ok(())
    }

    pub fn set_running(start: bool) -> Result<(), String> {
        let mgr = manager(ServiceManagerAccess::CONNECT)?;
        let access = if start { ServiceAccess::START } else { ServiceAccess::STOP };
        let service = mgr.open_service(SERVICE_NAME, access).map_err(|e| e.to_string())?;
        if start {
            service.start::<OsString>(&[]).map_err(|e| e.to_string())?;
        } else {
            service.stop().map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn status() -> String {
        let Ok(mgr) = manager(ServiceManagerAccess::CONNECT) else {
            return "unknown".into();
        };
        match mgr.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
            Err(_) => "not installed".into(),
            Ok(service) => match service.query_status() {
                Ok(s) => match s.current_state {
                    ServiceState::Running => "running".into(),
                    ServiceState::Stopped => "stopped".into(),
                    ServiceState::StartPending => "starting".into(),
                    ServiceState::StopPending => "stopping".into(),
                    other => format!("{other:?}").to_lowercase(),
                },
                Err(_) => "installed".into(),
            },
        }
    }

    // Config handed from `main` (which parsed the binPath args) to the SCM-invoked
    // `service_main`, since the latter gets the StartService args, not binPath args.
    static CONFIG: OnceLock<(PathBuf, SocketAddr)> = OnceLock::new();

    pub fn run_as_service(maps_dir: PathBuf, bind: SocketAddr) -> Result<(), String> {
        let _ = CONFIG.set((maps_dir, bind));
        service_dispatcher::start(SERVICE_NAME, ffi_service_main).map_err(|e| e.to_string())
    }

    windows_service::define_windows_service!(ffi_service_main, service_main);

    fn service_main(_args: Vec<OsString>) {
        let _ = run_service();
    }

    fn status_with(state: ServiceState, accept: ServiceControlAccept) -> ServiceStatus {
        ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: state,
            controls_accepted: accept,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        }
    }

    fn run_service() -> windows_service::Result<()> {
        let (maps_dir, bind) = CONFIG.get().cloned().expect("config set before dispatch");

        let (stop_tx, stop_rx) = mpsc::channel();
        let handler = move |control| match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                let _ = stop_tx.send(());
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        };
        let status_handle = service_control_handler::register(SERVICE_NAME, handler)?;
        status_handle
            .set_service_status(status_with(ServiceState::Running, ServiceControlAccept::STOP))?;

        // Serve on detached worker threads; we just wait here for the stop signal.
        std::thread::spawn(move || {
            let _ = crate::serve_blocking(maps_dir, bind);
        });
        let _ = stop_rx.recv();

        status_handle
            .set_service_status(status_with(ServiceState::Stopped, ServiceControlAccept::empty()))?;
        Ok(())
    }
}

/* ───────────────────────────────── Linux ───────────────────────────────── */
#[cfg(unix)]
mod imp {
    use super::{SocketAddr, PathBuf, DISPLAY_NAME, UNIT_NAME};

    fn unit_path() -> String {
        format!("/etc/systemd/system/{UNIT_NAME}.service")
    }

    pub fn install(maps_dir: PathBuf, bind: SocketAddr) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let unit = format!(
            "[Unit]\n\
             Description={DISPLAY_NAME}\n\
             After=network-online.target\n\
             Wants=network-online.target\n\n\
             [Service]\n\
             Type=simple\n\
             ExecStart={exe} run --maps {maps} --bind {bind}\n\
             Restart=on-failure\n\
             RestartSec=2\n\n\
             [Install]\n\
             WantedBy=multi-user.target\n",
            exe = exe.display(),
            maps = maps_dir.display(),
        );
        std::fs::write(unit_path(), unit)
            .map_err(|e| format!("write {} (run with sudo?): {e}", unit_path()))?;
        systemctl(&["daemon-reload"])?;
        systemctl(&["enable", "--now", &format!("{UNIT_NAME}.service")])
    }

    pub fn uninstall() -> Result<(), String> {
        let _ = systemctl(&["disable", "--now", &format!("{UNIT_NAME}.service")]);
        std::fs::remove_file(unit_path()).map_err(|e| e.to_string())?;
        systemctl(&["daemon-reload"])
    }

    pub fn set_running(start: bool) -> Result<(), String> {
        let verb = if start { "start" } else { "stop" };
        systemctl(&[verb, &format!("{UNIT_NAME}.service")])
    }

    pub fn status() -> String {
        std::process::Command::new("systemctl")
            .args(["is-active", &format!("{UNIT_NAME}.service")])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "not installed".into())
    }

    fn systemctl(args: &[&str]) -> Result<(), String> {
        let st = std::process::Command::new("systemctl")
            .args(args)
            .status()
            .map_err(|e| format!("systemctl not available: {e}"))?;
        if st.success() {
            Ok(())
        } else {
            Err(format!("`systemctl {}` failed (need root?)", args.join(" ")))
        }
    }
}
