//! Shared harness for agentd integration tests: fixture bundle assembly,
//! daemon boot, `agentctl` invocation, and poll-based state waits.
#![cfg(unix)]
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use agentd::bootstrap::{Daemon, DaemonConfig, boot};
use domain::ids::RunId;
use serde_json::Value;

pub const CONFIG_YAML: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/default.yaml");
pub const FIXTURE_MANIFEST: &str =
    include_str!("../../../../fixtures/agent-loop/adapter.manifest.json");

/// The fixture loop binary built by `cargo build/test --workspace`.
pub fn fixture_loop_binary() -> PathBuf {
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
pub fn make_fixture_bundle(root: &Path) -> PathBuf {
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

pub const COMPLETE_SCRIPT: &str = r#"[{"complete":{"output_ref":"e2e://output/1"}}]"#;

pub async fn boot_daemon(runtime_dir: &Path, bundles: Vec<PathBuf>) -> Daemon {
    boot_daemon_scripts(runtime_dir, bundles, HashMap::new()).await
}

pub async fn boot_daemon_scripts(
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

pub async fn cli(socket: &Path, args: &[&str]) -> Value {
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    agentctl::run(&args, socket)
        .await
        .expect("agentctl command")
}

pub const RUN_COMPLETED: i64 = 9;

/// Poll `agentctl get-run` until `state` matches (no sleeps — yield-based).
pub async fn wait_for_state(socket: &Path, run_id: &str, want: i64, deadline_ms: u128) -> Value {
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
pub async fn make_session_and_spec(socket: &Path, dir: &Path) -> (String, String, String) {
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
pub fn create_run_args<'a>(
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
