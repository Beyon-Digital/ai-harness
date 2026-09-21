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

static BUNDLE_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Unique bundle dir name per call — an aborted daemon leaves its fixture
/// processes exiting asynchronously, so re-using a path races `ETXTBSY`.
fn unique_bundle(root: &Path, name: &str) -> PathBuf {
    let seq = BUNDLE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    root.join(format!("{name}-{seq}"))
}

/// Assemble a content-addressed adapter bundle dir: manifest + entrypoint +
/// `bundle.lock`, the shape `register`/`verify_for_spawn` expect.
pub fn make_fixture_bundle(root: &Path) -> PathBuf {
    let bundle = unique_bundle(root, "fixture-loop");
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

/// The fixture effect-adapter binary built by `cargo build/test --workspace`.
pub fn fixture_effect_binary() -> PathBuf {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/fixture-effect-adapter");
    assert!(
        path.exists(),
        "fixture-effect-adapter binary missing at {}; run `cargo build --workspace` first",
        path.display()
    );
    path
}

/// Manifest text for the fixture effect adapter (`effect.execute@1`).
pub const EFFECT_MANIFEST: &str =
    include_str!("../../../../fixtures/effect-adapter/adapter.manifest.json");

/// Assemble a bundle dir for the fixture effect adapter.
pub fn make_effect_bundle(root: &Path) -> PathBuf {
    let bundle = unique_bundle(root, "fixture-effect");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(
        bundle.join(adapter_registry::manifest::MANIFEST_FILE),
        EFFECT_MANIFEST,
    )
    .unwrap();
    std::fs::copy(
        fixture_effect_binary(),
        bundle.join("fixture-effect-adapter"),
    )
    .unwrap();
    adapter_registry::write_lock(&bundle).unwrap();
    bundle
}

/// Base64 encoder (fixture scripts embed payloads/drafts as base64).
pub fn b64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
        if c.len() < 3 {
            out.replace_range(out.len() - (3 - c.len()).., &"=".repeat(3 - c.len()));
        }
    }
    out
}

/// Boot a daemon with loop scripts and extra env for spawned effect
/// adapters (e.g. `FIXTURE_CRASH_BEFORE_RESPONSE`).
pub async fn boot_daemon_full(
    runtime_dir: &Path,
    bundles: Vec<PathBuf>,
    loop_scripts: HashMap<RunId, String>,
    effect_env: HashMap<String, String>,
) -> Daemon {
    // `local-trusted` binds `effect.execute` to the fixture effect adapter —
    // every booted daemon needs its bundle registered.
    let mut bundles = bundles;
    let bundle_root = runtime_dir.join("bundles");
    std::fs::create_dir_all(&bundle_root).unwrap();
    bundles.push(make_effect_bundle(&bundle_root));
    boot(DaemonConfig {
        runtime_dir: runtime_dir.to_path_buf(),
        config_doc: Some(PathBuf::from(CONFIG_YAML)),
        adapter_bundles: bundles,
        loop_scripts,
        effect_env,
        poll: std::time::Duration::from_millis(10),
        json_logs: false,
    })
    .await
    .expect("daemon boot")
}

/// `get-run` without panicking on not-found.
pub async fn try_get_run(socket: &Path, run_id: &str) -> Option<Value> {
    let args = vec!["get-run".to_owned(), run_id.to_owned()];
    agentctl::run(&args, socket).await.ok()
}

/// Wait until `get-run` reports a terminal state; returns the state.
pub async fn wait_terminal(socket: &Path, run_id: &str) -> i64 {
    let deadline = Instant::now() + std::time::Duration::from_millis(30_000);
    loop {
        let run = cli(socket, &["get-run", run_id]).await;
        let state = run["state"].as_i64().unwrap();
        if state >= 9 {
            return state;
        }
        assert!(
            Instant::now() < deadline,
            "run {run_id} never terminalized: {run}"
        );
        tokio::task::yield_now().await;
    }
}

pub async fn boot_daemon(runtime_dir: &Path, bundles: Vec<PathBuf>) -> Daemon {
    boot_daemon_scripts(runtime_dir, bundles, HashMap::new()).await
}

pub async fn boot_daemon_scripts(
    runtime_dir: &Path,
    bundles: Vec<PathBuf>,
    loop_scripts: HashMap<RunId, String>,
) -> Daemon {
    boot_daemon_full(runtime_dir, bundles, loop_scripts, HashMap::new()).await
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
