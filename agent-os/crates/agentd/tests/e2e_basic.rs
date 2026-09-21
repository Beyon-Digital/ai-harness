//! INT-001: process-level end-to-end — real daemon boot, API-driven run
//! lifecycle through the real fixture loop adapter, restart retention, and
//! idle shutdown. Nothing reaches into the store to force progress.
#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use agentd::bootstrap::{Daemon, DaemonConfig, boot};
use domain::ids::RunId;
use serde_json::Value;

const CONFIG_YAML: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/default.yaml");
const FIXTURE_MANIFEST: &str = include_str!("../../../fixtures/agent-loop/adapter.manifest.json");

/// The fixture loop binary built by `cargo build/test --workspace`.
fn fixture_loop_binary() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/fixture-agent-loop");
    assert!(
        path.exists(),
        "fixture-agent-loop binary missing at {}; run `cargo build --workspace` first",
        path.display()
    );
    path
}

/// Assemble a content-addressed adapter bundle dir: manifest + entrypoint +
/// `bundle.lock`, the shape `register`/`verify_for_spawn` expect.
fn make_fixture_bundle(root: &Path) -> PathBuf {
    let bundle = root.join("fixture-loop");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(
        bundle.join(adapter_registry::manifest::MANIFEST_FILE),
        FIXTURE_MANIFEST,
    )
    .unwrap();
    let bin = bundle.join("fixture-agent-loop");
    std::fs::copy(fixture_loop_binary(), &bin).unwrap();
    adapter_registry::write_lock(&bundle).unwrap();
    bundle
}

const COMPLETE_SCRIPT: &str = r#"[{"complete":{"output_ref":"e2e://output/1"}}]"#;

async fn boot_daemon(runtime_dir: &Path, bundles: Vec<PathBuf>) -> Daemon {
    boot_daemon_scripts(runtime_dir, bundles, HashMap::new()).await
}

async fn boot_daemon_scripts(
    runtime_dir: &Path,
    bundles: Vec<PathBuf>,
    loop_scripts: HashMap<RunId, String>,
) -> Daemon {
    boot(DaemonConfig {
        runtime_dir: runtime_dir.to_path_buf(),
        config_doc: Some(PathBuf::from(CONFIG_YAML)),
        adapter_bundles: bundles,
        loop_scripts,
        poll: std::time::Duration::from_millis(10),
        json_logs: false,
    })
    .await
    .expect("daemon boot")
}

async fn cli(socket: &Path, args: &[&str]) -> Value {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    agentctl::run(&args, socket)
        .await
        .expect("agentctl command")
}

const RUN_COMPLETED: i64 = 9;

/// Poll `agentctl get-run` until `state` matches (no sleeps — yield-based).
async fn wait_for_state(socket: &Path, run_id: &str, want: i64, deadline_ms: u128) -> Value {
    let deadline = Instant::now() + std::time::Duration::from_millis(deadline_ms as u64);
    loop {
        let run = cli(socket, &["get-run", run_id]).await;
        if run["state"] == want {
            return run;
        }
        assert!(
            Instant::now() < deadline,
            "run {run_id} never reached state {want}: {run}"
        );
        tokio::task::yield_now().await;
    }
}

/// Registers the session + agent-spec binding inputs a run needs, returning
/// (session_id, spec_id, spec_digest). `spec` is written to `dir` for `--body`.
async fn make_session_and_spec(socket: &Path, dir: &Path) -> (String, String, String) {
    let session = cli(socket, &["create-session"]).await;
    let session_id = session["payload"].as_str().unwrap().to_owned();
    let spec_id = "01999999-0000-7000-8000-00000000a001";
    let body_bytes = br#"{"runtime_profile_name":"local-trusted"}"#;
    let body = dir.join("spec.json");
    std::fs::write(&body, body_bytes).unwrap();
    use sha2::Digest;
    let digest = sha2::Sha256::digest(body_bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    cli(
        socket,
        &[
            "put-agent-spec",
            spec_id,
            "--version",
            "v1",
            "--body",
            body.to_str().unwrap(),
        ],
    )
    .await;
    (session_id, spec_id.to_owned(), digest)
}

/// Arguments for `create-run` bound to the registered spec.
fn create_run_args<'a>(
    session_id: &'a str,
    run_id: &'a str,
    spec_id: &'a str,
    spec_digest: &'a str,
) -> Vec<&'a str> {
    vec![
        "create-run",
        "--session-id",
        session_id,
        "--task-kind",
        "agent",
        "--profile",
        "local-trusted",
        "--run-id",
        run_id,
        "--agent-spec-id",
        spec_id,
        "--spec-version",
        "v1",
        "--spec-digest",
        spec_digest,
    ]
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_complete_run_via_daemon_api() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = make_fixture_bundle(tmp.path());
    let runtime_dir = tmp.path().join("runtime");
    let run_id = "01999999-0000-7000-8000-00000000e2e1".to_string();
    let scripts = HashMap::from([(run_id.parse::<RunId>().unwrap(), COMPLETE_SCRIPT.to_owned())]);

    let daemon = boot_daemon_scripts(&runtime_dir, vec![bundle], scripts).await;
    let socket = daemon.socket_path().to_path_buf();

    let health = cli(&socket, &["health"]).await;
    assert_eq!(health["status"], "running", "{health}");

    let (session_id, spec_id, spec_digest) = make_session_and_spec(&socket, tmp.path()).await;

    let created = cli(
        &socket,
        &create_run_args(&session_id, &run_id, &spec_id, &spec_digest),
    )
    .await;
    assert_eq!(created["outcome_code"], "ok", "{created}");

    // Ready -> bound -> claimed -> driven to terminal by the worker.
    let run = wait_for_state(&socket, &run_id, RUN_COMPLETED, 30_000).await;
    assert!(!run["resolved_environment_id"].as_str().unwrap().is_empty());

    // Journal retained the full lifecycle evidence for the run stream
    // (published via the outbox dispatcher — poll until drained).
    let deadline = Instant::now() + std::time::Duration::from_millis(30_000);
    loop {
        let events = cli(
            &socket,
            &["events", "read", "--stream-key", &format!("run/{run_id}")],
        )
        .await;
        let types: Vec<String> = events["events"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|e| e["event_type"].as_str().map(str::to_owned))
            .collect();
        if types.iter().any(|t| t.contains("RunReady"))
            && types.iter().any(|t| t.contains("RunCompleted"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expected run lifecycle events, got {types:?}"
        );
        tokio::task::yield_now().await;
    }

    daemon.initiate_shutdown();
    daemon.wait().await.expect("clean shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_restart_after_completed_run_retains_audit_data() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = make_fixture_bundle(tmp.path());
    let runtime_dir = tmp.path().join("runtime");

    let run_id;
    {
        run_id = "01999999-0000-7000-8000-00000000e2e2".to_string();
        let scripts =
            HashMap::from([(run_id.parse::<RunId>().unwrap(), COMPLETE_SCRIPT.to_owned())]);
        let daemon = boot_daemon_scripts(&runtime_dir, vec![bundle.clone()], scripts).await;
        let socket = daemon.socket_path().to_path_buf();
        let (session_id, spec_id, spec_digest) = make_session_and_spec(&socket, tmp.path()).await;
        cli(
            &socket,
            &create_run_args(&session_id, &run_id, &spec_id, &spec_digest),
        )
        .await;
        wait_for_state(&socket, &run_id, RUN_COMPLETED, 30_000).await;
        daemon.initiate_shutdown();
        daemon.wait().await.expect("clean shutdown");
    }

    // Reboot on the same runtime dir: singleton lock released, store/journal
    // reopened, audit data still queryable through the API.
    let daemon = boot_daemon(&runtime_dir, vec![bundle]).await;
    let socket = daemon.socket_path().to_path_buf();
    let run = cli(&socket, &["get-run", &run_id]).await;
    assert_eq!(run["state"], RUN_COMPLETED, "{run}");
    let deadline = Instant::now() + std::time::Duration::from_millis(30_000);
    loop {
        let events = cli(
            &socket,
            &["events", "read", "--stream-key", &format!("run/{run_id}")],
        )
        .await;
        if !events["events"].as_array().unwrap().is_empty() {
            break;
        }
        assert!(Instant::now() < deadline, "journal must survive restart");
        tokio::task::yield_now().await;
    }
    daemon.initiate_shutdown();
    daemon.wait().await.expect("clean shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_idle_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime_dir = tmp.path().join("runtime");
    let daemon = boot_daemon(&runtime_dir, vec![make_fixture_bundle(tmp.path())]).await;
    daemon.initiate_shutdown();
    daemon.wait().await.expect("clean shutdown");

    // Singleton released: a second daemon can boot the same dir.
    let daemon = boot_daemon(&runtime_dir, vec![]).await;
    daemon.initiate_shutdown();
    daemon.wait().await.expect("clean shutdown");
}
