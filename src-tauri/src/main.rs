//! CamFunge GUI shell. All camera, GPU and virtual-camera work happens in `camfunge-service`; this process
//! only makes sure one is running and opens the control window, which talks to it over WebSocket.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{Manager, RunEvent, WebviewUrl, WebviewWindowBuilder};

const DEFAULT_PORT: u16 = 7272;
const SERVICE: &str = "camfunge-service";

fn port() -> u16 {
    std::env::var("CAMFUNGE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT)
}

fn service_running(port: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_millis(300)).is_ok()
}

/// The service binary: next to this executable (bundles, and `cargo build` of the workspace), else on PATH.
fn service_binary() -> PathBuf {
    let beside = std::env::current_exe().ok().and_then(|exe| exe.parent().map(|dir| dir.join(format!("{SERVICE}{}", std::env::consts::EXE_SUFFIX))));
    beside.filter(|p| p.exists()).unwrap_or_else(|| PathBuf::from(SERVICE))
}

/// Starts the service unless one is already listening (e.g. started by hand or as a system service), in which
/// case the GUI just attaches to it and leaves it running on exit.
fn start_service(port: u16, dev_origin: Option<&str>) -> Option<Child> {
    if service_running(port) {
        eprintln!("camfunge: attaching to the service already on port {port}");
        if let Some(origin) = dev_origin {
            eprintln!("camfunge: (if the GUI cannot connect, that service needs `--allow-origin {origin}`)");
        }
        return None;
    }
    let binary = service_binary();
    let mut args = vec!["--port".to_string(), port.to_string(), "--exit-with-stdin".into()];
    if let Some(origin) = dev_origin {
        args.extend(["--allow-origin".into(), origin.to_string()]);
    }
    // Stdin stays open for as long as we live; when it closes (even if we crash) the service shuts down.
    match Command::new(&binary).args(&args).stdin(Stdio::piped()).stdout(Stdio::null()).spawn() {
        Ok(child) => Some(child),
        Err(e) => {
            eprintln!("camfunge: could not start {}: {e}", binary.display());
            None
        }
    }
}

fn stop_service(mut child: Child) {
    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
}

struct Service(Mutex<Option<Child>>);

fn main() {
    let port = port();
    let app = tauri::Builder::default()
        .setup(move |app| {
            // Under `tauri dev` the page comes from the CLI's dev server (e.g. http://127.0.0.1:1430), not
            // tauri://localhost, so the service must be told to accept that origin too.
            let dev_origin = tauri::is_dev().then(|| app.config().build.dev_url.as_ref().map(|u| u.origin().ascii_serialization())).flatten();
            app.manage(Service(Mutex::new(start_service(port, dev_origin.as_deref()))));
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("CamFunge")
                .inner_size(1280.0, 800.0)
                .min_inner_size(480.0, 320.0)
                .initialization_script(format!("window.CAMFUNGE_SERVICE = 'ws://127.0.0.1:{port}';"))
                .build()?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            if let Some(child) = handle.state::<Service>().0.lock().unwrap().take() {
                stop_service(child);
            }
        }
    });
}
