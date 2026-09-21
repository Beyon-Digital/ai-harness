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
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().is_some_and(|a| a == "backup") {
        return backup_command(&argv[1..]).await;
    }
    let mut config = DaemonConfig::default();
    let mut args = argv.into_iter();
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
                    "agentd --runtime-dir <dir> [--config <doc.yaml>] \\\n  [--adapter-bundle <dir>]... [--json]\n\
                     agentd backup --runtime-dir <dir> --out <dir>"
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

/// `agentd backup --runtime-dir <dir> --out <dir>` — consistent snapshots
/// of `kernel.db`/`events.db` via `VACUUM INTO` (safe while the daemon is
/// running; WAL contents are folded into the snapshot).
async fn backup_command(args: &[String]) -> std::process::ExitCode {
    use sqlx::Connection as _;
    use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};

    let mut runtime_dir = None;
    let mut out = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(f, v)| (f, Some(v.to_owned())));
        let value = inline.or_else(|| it.next().cloned());
        match flag {
            "--runtime-dir" => runtime_dir = value.map(PathBuf::from),
            "--out" => out = value.map(PathBuf::from),
            other => {
                eprintln!("agentd backup: unknown flag {other}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    let (Some(runtime_dir), Some(out)) = (runtime_dir, out) else {
        eprintln!("agentd backup: --runtime-dir and --out are required");
        return std::process::ExitCode::FAILURE;
    };
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("agentd backup: cannot create {}: {e}", out.display());
        return std::process::ExitCode::FAILURE;
    }
    let mut ok = true;
    for name in ["kernel.db", "events.db"] {
        let src = runtime_dir.join(name);
        if !src.is_file() {
            // A backup missing a database is not a usable snapshot.
            eprintln!("agentd backup: missing {}", src.display());
            ok = false;
            continue;
        }
        let dst = out.join(name);
        let options = SqliteConnectOptions::new()
            .filename(&src)
            .create_if_missing(false);
        let result: Result<(), sqlx::Error> = async {
            let mut conn = SqliteConnection::connect_with(&options).await?;
            let lit = dst.display().to_string().replace('\'', "''");
            sqlx::query(&format!("VACUUM INTO '{lit}'"))
                .execute(&mut conn)
                .await?;
            conn.close().await
        }
        .await;
        match result {
            Ok(()) => println!("agentd backup: {} -> {}", src.display(), dst.display()),
            Err(e) => {
                eprintln!("agentd backup: {}: {e}", src.display());
                ok = false;
            }
        }
    }
    if ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
