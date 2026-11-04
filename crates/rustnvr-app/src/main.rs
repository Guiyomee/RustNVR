//! `rustnvr` command line.

use anyhow::Context as _;
use clap::{Parser, Subcommand};
use rustnvr_app::Nvr;
use rustnvr_app::logging::{self, LogFormat};
use rustnvr_core::config::{Config, RtspTransport};
use rustnvr_core::events::event_bus;
use rustnvr_core::{CameraId, RtspTarget, Secret};
use rustnvr_stream::{SessionOptions, probe};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "rustnvr",
    version,
    about = "Lightweight network video recorder"
)]
struct Cli {
    /// Log format.
    #[arg(long, global = true, value_enum, default_value = "text")]
    log_format: LogFormat,
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum TransportArg {
    Tcp,
    Udp,
}

#[derive(Subcommand)]
enum Command {
    /// Runs the NVR until SIGINT/SIGTERM.
    Run {
        /// Configuration file.
        #[arg(short, long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Validates a configuration file and exits.
    CheckConfig {
        /// Configuration file.
        #[arg(short, long, default_value = "config.toml")]
        config: PathBuf,
    },
    /// Tests the connection to a camera: lists its streams and waits for a key frame.
    Probe {
        /// RTSP URL. Prefer --username/--password-env over credentials in the URL.
        #[arg(long, conflicts_with = "camera")]
        url: Option<String>,
        /// User name.
        #[arg(long, requires = "url")]
        username: Option<String>,
        /// Environment variable holding the password (kept out of shell history).
        #[arg(long, requires = "url")]
        password_env: Option<String>,
        /// Probe the main stream of a camera from the configuration file instead.
        #[arg(long)]
        camera: Option<CameraId>,
        /// Configuration file (with --camera).
        #[arg(short, long, default_value = "config.toml")]
        config: PathBuf,
        /// RTP transport (with --url).
        #[arg(long, value_enum, default_value = "tcp")]
        transport: TransportArg,
        /// Timeout per step, and for the first key frame, in seconds.
        #[arg(long, default_value_t = 10)]
        timeout_seconds: u64,
        /// Only DESCRIBE; do not wait for video.
        #[arg(long)]
        no_frame: bool,
    },
    /// Repairs segments interrupted by a crash (also done automatically at startup).
    Recover {
        /// Recordings directory.
        #[arg(long)]
        dir: PathBuf,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    logging::init(cli.log_format);
    match run(cli.command).await {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("{e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(command: Command) -> anyhow::Result<ExitCode> {
    match command {
        Command::Run { config } => {
            let config = Config::load(&config)?;
            if config.cameras.is_empty() {
                tracing::warn!("no camera configured");
            }
            let nvr = Nvr::start(&config, event_bus(1024)).await?;
            wait_for_shutdown_signal().await?;
            Ok(if nvr.shutdown().await {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
        Command::CheckConfig { config } => {
            let c = Config::load(&config)?;
            println!(
                "configuration OK: {} camera(s), {} enabled, recordings in {}",
                c.cameras.len(),
                c.cameras.iter().filter(|c| c.enabled).count(),
                c.storage.recordings_dir.display()
            );
            Ok(ExitCode::SUCCESS)
        }
        Command::Probe {
            url,
            username,
            password_env,
            camera,
            config,
            transport,
            timeout_seconds,
            no_frame,
        } => {
            let timeout = Duration::from_secs(timeout_seconds);
            let (target, mut opts) = match (url, camera) {
                (Some(url), _) => {
                    let password = match password_env {
                        Some(var) => {
                            Some(Secret::new(std::env::var(&var).with_context(|| {
                                format!("environment variable {var} not set")
                            })?))
                        }
                        None => None,
                    };
                    let target = RtspTarget::parse(&url, username.as_deref(), password)?;
                    let transport = match transport {
                        TransportArg::Tcp => RtspTransport::Tcp,
                        TransportArg::Udp => RtspTransport::Udp,
                    };
                    (
                        target,
                        SessionOptions {
                            transport,
                            ..SessionOptions::default()
                        },
                    )
                }
                (None, Some(id)) => {
                    let c = Config::load(&config)?;
                    let cam = c
                        .cameras
                        .iter()
                        .find(|c| c.id == id)
                        .with_context(|| format!("camera {id} not found in configuration"))?;
                    let target = cam.main.resolve().map_err(anyhow::Error::msg)?;
                    (target, SessionOptions::from_config(&cam.main))
                }
                (None, None) => anyhow::bail!("give --url or --camera"),
            };
            opts.connect_timeout = timeout;
            opts.stall_timeout = timeout;
            println!("probing {}", target.redacted());
            let report = probe(&target, &opts, !no_frame).await?;
            for (i, s) in report.streams.iter().enumerate() {
                let mark = if Some(i) == report.selected { "*" } else { " " };
                let fps = s.framerate.map(|f| format!(" {f} fps")).unwrap_or_default();
                println!(
                    "{mark} stream {i}: {}/{} clock {} Hz{fps}",
                    s.media, s.encoding, s.clock_rate
                );
            }
            if let Some(tool) = &report.tool {
                println!("  server tool: {tool}");
            }
            match (&report.selected, &report.video) {
                (None, _) => {
                    println!("no H.264/H.265 video stream: this camera cannot be recorded");
                    return Ok(ExitCode::FAILURE);
                }
                (Some(_), Some(v)) => println!(
                    "OK: {} ({}) {}x{}, first key frame after {} ms",
                    v.codec,
                    v.rfc6381_codec,
                    v.width,
                    v.height,
                    v.time_to_key_frame.as_millis()
                ),
                (Some(_), None) => println!("OK: DESCRIBE succeeded (video not requested)"),
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Recover { dir } => {
            let report =
                tokio::task::spawn_blocking(move || rustnvr_recorder::recover_tree(&dir)).await?;
            for p in &report.recovered {
                println!("recovered {}", p.display());
            }
            for p in &report.deleted {
                println!("deleted (no complete fragment) {}", p.display());
            }
            for (p, e) in &report.failed {
                println!("FAILED {}: {e}", p.display());
            }
            Ok(if report.failed.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            })
        }
    }
}

async fn wait_for_shutdown_signal() -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).context("cannot install SIGTERM handler")?;
        tokio::select! {
            r = tokio::signal::ctrl_c() => r.context("cannot listen for SIGINT")?,
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c()
        .await
        .context("cannot listen for Ctrl-C")?;
    Ok(())
}
