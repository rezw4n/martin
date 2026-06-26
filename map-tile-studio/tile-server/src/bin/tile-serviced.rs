//! Map Tile Studio — headless tile service.
//!
//! Serves the generated maps folder over HTTP so tile URLs keep working with the
//! desktop app closed, and (installed as an OS service) across reboots.
//!
//! Usage:
//!   tile-serviced run        [--maps <dir>] [--bind <addr:port>]   # foreground
//!   tile-serviced install    [--maps <dir>] [--bind <addr:port>]   # register + start (admin/root)
//!   tile-serviced uninstall                                        # remove (admin/root)
//!   tile-serviced start | stop                                     # control an installed service
//!   tile-serviced status                                           # print state
//!   tile-serviced --help
//!
//! Defaults: --maps = the Map Tile Studio maps folder, --bind = 0.0.0.0:7765 (LAN).

use std::net::SocketAddr;
use std::path::PathBuf;

use mts_tile_server::service;

const DEFAULT_BIND: &str = "0.0.0.0:7765";

fn main() {
    let mut args = std::env::args().skip(1);
    let mut cmd = String::new();
    let mut maps: Option<PathBuf> = None;
    let mut bind: Option<String> = None;

    while let Some(a) = args.next() {
        match a.as_str() {
            "--help" | "-h" => return print_help(),
            "--maps" => maps = args.next().map(PathBuf::from),
            "--bind" => bind = args.next(),
            other if !other.starts_with('-') && cmd.is_empty() => cmd = other.to_string(),
            _ => {}
        }
    }

    let maps = maps.unwrap_or_else(default_maps_dir);
    let bind: SocketAddr = bind
        .as_deref()
        .unwrap_or(DEFAULT_BIND)
        .parse()
        .unwrap_or_else(|_| DEFAULT_BIND.parse().expect("valid default bind"));

    let result: Result<(), String> = match cmd.as_str() {
        "" | "run" => {
            let _ = std::fs::create_dir_all(&maps);
            eprintln!(
                "tile-serviced: serving {} on http://{bind}/{{source}}/{{z}}/{{x}}/{{y}}",
                maps.display()
            );
            mts_tile_server::serve_blocking(maps, bind).map_err(|e| e.to_string())
        }
        // Invoked by the Windows Service Control Manager (registered by `install`).
        "service-run" => run_service(maps, bind),
        "install" => {
            let _ = std::fs::create_dir_all(&maps);
            service::install(maps, bind).inspect(|()| {
                println!("Installed and started `{}` on {bind}.", service::DISPLAY_NAME);
            })
        }
        "uninstall" => service::uninstall().inspect(|()| println!("Service removed.")),
        "start" => service::set_running(true).inspect(|()| println!("Service started.")),
        "stop" => service::set_running(false).inspect(|()| println!("Service stopped.")),
        "status" => {
            println!("{}", service::status());
            Ok(())
        }
        other => {
            eprintln!("tile-serviced: unknown command `{other}`\n");
            print_help();
            std::process::exit(2);
        }
    };

    if let Err(e) = result {
        eprintln!("tile-serviced: {e}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
fn run_service(maps: PathBuf, bind: SocketAddr) -> Result<(), String> {
    service::run_as_service(maps, bind)
}

// On Linux systemd runs `run` directly; `service-run` just serves in the foreground.
#[cfg(not(windows))]
fn run_service(maps: PathBuf, bind: SocketAddr) -> Result<(), String> {
    mts_tile_server::serve_blocking(maps, bind).map_err(|e| e.to_string())
}

fn print_help() {
    eprintln!(
        "Map Tile Studio tile service\n\n\
         USAGE:\n  \
         tile-serviced run        [--maps <dir>] [--bind <addr:port>]   foreground\n  \
         tile-serviced install    [--maps <dir>] [--bind <addr:port>]   register + start (admin/root)\n  \
         tile-serviced uninstall                                        remove (admin/root)\n  \
         tile-serviced start | stop                                     control an installed service\n  \
         tile-serviced status                                           print state\n\n\
         OPTIONS:\n  \
         --maps <dir>        Folder of generated maps to serve (.mbtiles / .tif)\n  \
         --bind <addr:port>  Address to listen on (default {DEFAULT_BIND})\n  \
         -h, --help          Show this help"
    );
}

/// Default maps folder (matches the desktop app's output location).
fn default_maps_dir() -> PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))
    };
    base.unwrap_or_else(std::env::temp_dir)
        .join("MapTileStudio")
        .join("maps")
}
