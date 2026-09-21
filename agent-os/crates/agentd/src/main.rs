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
    if argv.first().is_some_and(|a| a == "adapter") {
        return adapter_command(&argv[1..]).await;
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
                     agentd backup --runtime-dir <dir> --out <dir>\n\
                     agentd adapter install --runtime-dir <dir> --bundle <dir> [--check]\n\
                     agentd adapter check --bundle <dir>\n\
                     agentd adapter list --runtime-dir <dir>\n\
                     agentd adapter enable|disable --runtime-dir <dir> <id-prefix> [--version v]\n\
                     agentd adapter remove --runtime-dir <dir> <id-prefix> [--version v]"
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

/// `agentd adapter <install|check|list|enable|disable|remove>` — Phase-13
/// bundle lifecycle over `<runtime_dir>/installed-adapters/`.
async fn adapter_command(args: &[String]) -> std::process::ExitCode {
    let Some(sub) = args.first().map(String::as_str) else {
        eprintln!("agentd adapter: missing subcommand (install|check|list|enable|disable|remove)");
        return std::process::ExitCode::FAILURE;
    };
    let mut runtime_dir = PathBuf::from("run");
    let mut bundle = None;
    let mut version = None;
    let mut id_prefix = None;
    let mut run_check = false;
    let mut it = args[1..].iter().peekable();
    while let Some(arg) = it.next() {
        let (flag, inline) = arg
            .split_once('=')
            .map_or((arg.as_str(), None), |(f, v)| (f, Some(v.to_owned())));
        // Value-taking flags consume the next arg only when no inline
        // `--flag=value` was given; positionals and bare flags consume
        // nothing.
        let mut value = || inline.clone().or_else(|| it.next().cloned());
        match flag {
            "--runtime-dir" => {
                if let Some(v) = value() {
                    runtime_dir = PathBuf::from(v);
                }
            }
            "--bundle" => bundle = value().map(PathBuf::from),
            "--version" => version = value(),
            "--check" => run_check = true,
            other if !other.starts_with('-') => {
                if id_prefix.is_some() {
                    eprintln!("agentd adapter: unexpected argument '{other}'");
                    return std::process::ExitCode::FAILURE;
                }
                id_prefix = Some(other.to_owned());
            }
            other => {
                eprintln!("agentd adapter: unknown flag {other}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    let fail = |e: errors::KernelError| -> std::process::ExitCode {
        eprintln!("agentd adapter {sub}: {e}");
        std::process::ExitCode::FAILURE
    };
    match sub {
        "install" => {
            let Some(bundle) = bundle else {
                eprintln!("agentd adapter install: --bundle <dir> is required");
                return std::process::ExitCode::FAILURE;
            };
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            match agentd::adapters::install(&runtime_dir, &bundle, run_check, now_ms).await {
                Ok(entry) => {
                    println!(
                        "installed {}@{} ({}) → {}/{}",
                        entry.adapter_id,
                        entry.version,
                        entry.bundle_digest,
                        runtime_dir.join(agentd::adapters::INSTALLED_DIR).display(),
                        entry.dir_name
                    );
                    if let Some(check) = &entry.check {
                        println!("  check: {check}");
                    }
                    std::process::ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            }
        }
        "check" => {
            let Some(bundle) = bundle else {
                eprintln!("agentd adapter check: --bundle <dir> is required");
                return std::process::ExitCode::FAILURE;
            };
            match agentd::adapters::check_bundle(&bundle).await {
                Ok(report) => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&report).unwrap_or_default()
                    );
                    if report.result == "pass" {
                        std::process::ExitCode::SUCCESS
                    } else {
                        std::process::ExitCode::FAILURE
                    }
                }
                Err(e) => fail(e),
            }
        }
        "list" => match agentd::adapters::load_index(&runtime_dir) {
            Ok(entries) if entries.is_empty() => {
                println!("no installed adapters");
                std::process::ExitCode::SUCCESS
            }
            Ok(entries) => {
                for e in entries {
                    println!(
                        "{}@{}\t{}\t{}\tcheck={}",
                        e.adapter_id,
                        e.version,
                        &e.bundle_digest[..e.bundle_digest.len().min(19)],
                        if e.enabled { "enabled" } else { "disabled" },
                        e.check.as_deref().unwrap_or("—"),
                    );
                }
                std::process::ExitCode::SUCCESS
            }
            Err(e) => fail(e),
        },
        "enable" | "disable" => {
            let Some(prefix) = id_prefix else {
                eprintln!("agentd adapter {sub}: <id-prefix> is required");
                return std::process::ExitCode::FAILURE;
            };
            match agentd::adapters::set_enabled(
                &runtime_dir,
                &prefix,
                version.as_deref(),
                sub == "enable",
            ) {
                Ok(e) => {
                    println!("{}@{} {}", e.adapter_id, e.version, sub);
                    std::process::ExitCode::SUCCESS
                }
                Err(e) => fail(e),
            }
        }
        "remove" => {
            let Some(prefix) = id_prefix else {
                eprintln!("agentd adapter remove: <id-prefix> is required");
                return std::process::ExitCode::FAILURE;
            };
            match agentd::adapters::remove(&runtime_dir, &prefix, version.as_deref()) {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(e) => fail(e),
            }
        }
        other => {
            eprintln!("agentd adapter: unknown subcommand '{other}'");
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
