//! `agentd` — the Agent OS microkernel daemon.
//!
//! ```text
//! agentd [--runtime-dir <dir>] [--config <doc.yaml>]
//!        [--adapter-bundle <dir>]... [--json]
//! ```
//!
//! Boots the composition root (lock, store, fence, recovery, registry,
//! coordinator, journal, workers, control/event APIs) and runs until
//! SIGINT/SIGTERM triggers drain-first shutdown.
#![forbid(unsafe_code)]

use std::path::PathBuf;

use agentd::bootstrap::{DaemonConfig, boot};

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> std::process::ExitCode {
    let mut config = DaemonConfig::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(f, v)| (f, Some(v.to_owned())));
        let mut take = || inline.clone().or_else(|| args.next());
        match flag {
            "--runtime-dir" => {
                let Some(value) = take() else {
                    eprintln!("agentd: --runtime-dir requires a value");
                    return std::process::ExitCode::FAILURE;
                };
                config.runtime_dir = PathBuf::from(value);
            }
            "--config" => {
                let Some(value) = take() else {
                    eprintln!("agentd: --config requires a value");
                    return std::process::ExitCode::FAILURE;
                };
                config.config_doc = Some(PathBuf::from(value));
            }
            "--adapter-bundle" => {
                let Some(value) = take() else {
                    eprintln!("agentd: --adapter-bundle requires a value");
                    return std::process::ExitCode::FAILURE;
                };
                config.adapter_bundles.push(PathBuf::from(value));
            }
            "--json" => config.json_logs = true,
            "--help" | "-h" => {
                println!(
                    "agentd --runtime-dir <dir> [--config <doc.yaml>] \\\n  [--adapter-bundle <dir>]... [--json]"
                );
                return std::process::ExitCode::SUCCESS;
            }
            other => {
                eprintln!("agentd: unknown flag {other}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }

    let daemon = match boot(config).await {
        Ok(daemon) => daemon,
        Err(error) => {
            eprintln!("agentd: boot failed: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    println!("agentd: listening on {}", daemon.socket_path().display());

    // SIGINT/SIGTERM -> drain-first shutdown.
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigint = signal(SignalKind::interrupt()).ok();
        let mut sigterm = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = async { if let Some(s) = sigint.as_mut() { s.recv().await } else { std::future::pending().await } } => {}
            _ = async { if let Some(s) = sigterm.as_mut() { s.recv().await } else { std::future::pending().await } } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
    tracing::info!("shutdown requested — draining");
    daemon.initiate_shutdown();
    match daemon.wait().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("agentd: shutdown error: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
