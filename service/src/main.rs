//! `camfunge-service`: the headless CamFunge engine. See `--help`.

use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use clap::Parser;

use camfunge::camera;
use camfunge::filters::FILTERS;
use camfunge::protocol::{Config, ConfigPatch};
use camfunge::server::OriginPolicy;
use camfunge::service::{self, Options, DEFAULT_PORT};
use camfunge::vcam::Choice;

/// Headless CamFunge: reads a camera, applies music-inspired filters on the GPU and publishes the result as a
/// virtual camera. Controlled over a local WebSocket (JSON) by the CamFunge GUI or any other client.
#[derive(Parser, Debug)]
#[command(version)]
struct Cli {
    /// Address to listen on for control connections. Keep it on loopback unless you trust your network.
    #[arg(long, default_value_t = IpAddr::V4(Ipv4Addr::LOCALHOST))]
    host: IpAddr,
    /// Control port (0 picks a free one; the chosen address is printed on stdout).
    #[arg(long, short, default_value_t = DEFAULT_PORT)]
    port: u16,
    /// Extra browser origins allowed to connect (Tauri's are always allowed; `*` allows any).
    #[arg(long = "allow-origin", value_name = "ORIGIN")]
    allow_origins: Vec<String>,
    /// Output width.
    #[arg(long, default_value_t = 1280)]
    width: u32,
    /// Output height.
    #[arg(long, default_value_t = 720)]
    height: u32,
    /// Output frame rate.
    #[arg(long, default_value_t = 30)]
    fps: u32,
    /// Virtual camera backend: auto, none, obs (macOS/Windows) or v4l2loopback (Linux).
    #[arg(long = "virtual-camera", default_value = "auto")]
    virtual_camera: Choice,
    /// v4l2loopback device to write to (default: the first one found).
    #[arg(long, value_name = "PATH")]
    v4l2_device: Option<PathBuf>,
    /// Initial camera id (see --list-cameras), or `test-pattern`.
    #[arg(long)]
    camera: Option<String>,
    /// Initial settings, as config-key=JSON-value pairs, e.g. `--set filter=3 --set reactTo='"sound"'`.
    #[arg(long = "set", value_name = "KEY=VALUE")]
    set: Vec<String>,
    /// Exit when stdin closes. The GUI uses this so the service never outlives it.
    #[arg(long)]
    exit_with_stdin: bool,
    /// Print the cameras and exit.
    #[arg(long)]
    list_cameras: bool,
    /// Print the filters and exit.
    #[arg(long)]
    list_filters: bool,
}

/// Builds a [`ConfigPatch`] from `key=value` pairs; values are JSON, or bare strings if not valid JSON.
fn parse_settings(pairs: &[String]) -> Result<ConfigPatch> {
    let mut obj = serde_json::Map::new();
    for pair in pairs {
        let (k, v) = pair.split_once('=').ok_or_else(|| anyhow!("--set {pair}: expected KEY=VALUE"))?;
        obj.insert(k.to_string(), serde_json::from_str(v).unwrap_or_else(|_| serde_json::Value::String(v.to_string())));
    }
    serde_json::from_value(obj.into()).context("--set")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.list_filters {
        for (i, f) in FILTERS.iter().enumerate() {
            println!("{i:>3}  {} — {}", f.name, f.artist);
        }
        return Ok(());
    }
    if cli.list_cameras {
        for c in camera::list() {
            println!("{:<40}  {}", c.id, c.name);
        }
        return Ok(());
    }

    let mut config = Config::default();
    let mut patch = parse_settings(&cli.set)?;
    if let Some(c) = cli.camera {
        patch.camera = Some(Some(c));
    }
    config.apply(&patch, FILTERS.len());
    let virtual_camera = match (cli.virtual_camera, cli.v4l2_device) {
        (Choice::Auto | Choice::V4l2Loopback { .. }, Some(device)) => Choice::V4l2Loopback { device: Some(device) },
        (choice, _) => choice,
    };
    let mut origins = OriginPolicy::default();
    origins.allowed.extend(cli.allow_origins);

    let shutdown = Arc::new(AtomicBool::new(false));
    let on_signal = shutdown.clone();
    ctrlc::set_handler(move || on_signal.store(true, Ordering::Relaxed)).context("installing Ctrl-C handler")?;
    if cli.exit_with_stdin {
        let on_eof = shutdown.clone();
        std::thread::spawn(move || {
            let _ = std::io::stdin().lock().read_to_end(&mut Vec::new());
            on_eof.store(true, Ordering::Relaxed);
        });
    }

    let opts = Options {
        listen: SocketAddr::new(cli.host, cli.port),
        origins,
        size: [cli.width, cli.height],
        fps: cli.fps,
        virtual_camera,
        config,
    };
    service::run(opts, shutdown, |addr, hello| {
        if let camfunge::protocol::ServerMessage::Hello { virtual_camera, gpu, output, .. } = hello {
            eprintln!("camfunge: rendering {}x{}@{} on {gpu}", output.width, output.height, output.fps);
            match (&virtual_camera.device, &virtual_camera.error) {
                (Some(d), _) => eprintln!("camfunge: virtual camera \"{d}\" ({})", virtual_camera.backend),
                (None, Some(e)) => eprintln!("camfunge: no virtual camera: {e}"),
                (None, None) => eprintln!("camfunge: virtual camera disabled"),
            }
        }
        // Machine-readable: the GUI and tests wait for this line.
        println!("listening on ws://{addr}");
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_parse_json_or_bare_strings() {
        let p = parse_settings(&["filter=3".into(), "reactTo=sound".into(), "mirror=false".into(), "camera=null".into()]).unwrap();
        assert_eq!(p.filter, Some(3));
        assert_eq!(p.react_to, Some(camfunge::protocol::ReactTo::Sound));
        assert_eq!(p.mirror, Some(false));
        assert_eq!(p.camera, Some(None));
        assert!(parse_settings(&["nope".into()]).is_err());
        assert!(parse_settings(&["bogus=1".into()]).is_err());
    }
}
