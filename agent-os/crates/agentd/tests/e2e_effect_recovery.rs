//! INT-003: effect crash/restart and reconciliation end-to-end — a real
//! daemon dispatches `fixture.increment_counter` to the fixture effect
//! adapter; the process dies after the side effect but before the response
//! is acked, the daemon is aborted, restarted under a higher fencing epoch,
//! and reconciles by operation ID — the counter is applied exactly once.
//! The `fixture.unreconcilable_counter` contract drives the same shape into
//! `Unknown` + blocked run, exited only via an explicit `resolve-effect`.
#![cfg(unix)]

mod common;

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::time::Instant;

use common::*;
use domain::ids::RunId;
use kernel_store::KernelStore;

const EFFECT_OP: &str = "fixture.increment_counter";
const UNRECONCILABLE_OP: &str = "fixture.unreconcilable_counter";
const EFFECT_ADAPTER_UUID: &str = "01905c5e-0000-7000-8000-e11ec7ad01ef";
const UNKNOWN_STATE: i64 = 8;

fn script(operation: &str) -> String {
    format!(
        r#"[{{"invoke_effect":{{"operation":"{operation}","payload":"{}"}}}},{{"complete":{{"output_ref":"e2e://output/1"}}}}]"#,
        b64(b"{}"),
    )
}

fn store_path(runtime_dir: &Path) -> std::path::PathBuf {
    runtime_dir.join(format!("fixture-store-{EFFECT_ADAPTER_UUID}.json"))
}

/// Counter entries recorded by the fixture store (operation_id -> count).
fn store_counts(runtime_dir: &Path) -> usize {
    std::fs::read_to_string(store_path(runtime_dir))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.as_object().map(|o| o.len()))
        .unwrap_or(0)
}

async fn create_run(socket: &Path, dir: &Path, run_id: &str) {
    let (session_id, spec_id, digest) = make_session_and_spec(socket, dir).await;
    cli(
        socket,
        &[
            "create-run",
            "--run-id",
            run_id,
            "--session-id",
            &session_id,
            "--agent-spec-id",
            &spec_id,
            "--spec-version",
            "v1",
            "--spec-digest",
            &digest,
            "--task-id",
            run_id,
            "--task-kind",
            "agent",
            "--profile",
            "local-trusted",
        ],
    )
    .await;
}

/// Read the run's effect rows straight from the kernel store.
async fn effect_rows(daemon: &agentd::bootstrap::Daemon, run_id: &str) -> Vec<String> {
    let run: RunId = run_id.parse().unwrap();
    let mut txn = daemon.store().begin_read().await.expect("read txn");
    txn.effects()
        .list_by_run(run)
        .await
        .expect("list effects")
        .iter()
        .map(|e| format!("{:?}", e.state))
        .collect()
}

/// Poll until the effect row reaches `want` (`EffectState` debug name).
async fn wait_effect_state(
    daemon: &agentd::bootstrap::Daemon,
    run_id: &str,
    want: &str,
    deadline_ms: u128,
) {
    let deadline = Instant::now() + std::time::Duration::from_millis(deadline_ms as u64);
    loop {
        if effect_rows(daemon, run_id).await.iter().any(|s| s == want) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "effect never reached {want}: {:?}",
            effect_rows(daemon, run_id).await
        );
        tokio::task::yield_now().await;
    }
}

/// Poll until the fixture store has `n` recorded operations (proves the
/// side effect landed before we crash the daemon).
async fn wait_store_len(runtime_dir: &Path, n: usize, deadline_ms: u128) {
    let deadline = Instant::now() + std::time::Duration::from_millis(deadline_ms as u64);
    while store_counts(runtime_dir) < n {
        assert!(
            Instant::now() < deadline,
            "fixture store never reached {n} operations"
        );
        tokio::task::yield_now().await;
    }
}

/// Effect execution on the happy path: dispatch -> ack -> commit -> resume.
#[tokio::test]
async fn e2e_effect_executes_normally() {
    let dir = tempfile::tempdir().unwrap();
    let runtime_dir = dir.path().join("runtime");
    let run_id = "01905c5e-0000-7000-8000-effec7000001";
    let mut scripts = HashMap::new();
    scripts.insert(RunId::from_str(run_id).unwrap(), script(EFFECT_OP));
    let daemon =
        boot_daemon_scripts(&runtime_dir, vec![make_fixture_bundle(dir.path())], scripts).await;
    let socket = runtime_dir.join("control.sock");
    create_run(&socket, dir.path(), run_id).await;
    assert_eq!(wait_terminal(&socket, run_id).await, RUN_COMPLETED);
    assert_eq!(effect_rows(&daemon, run_id).await, vec!["Committed"]);
    assert_eq!(store_counts(&runtime_dir), 1);
}

/// The crash the whole design exists for: adapter applies the mutation then
/// dies before the response. Daemon aborts before any ack persists; the next
/// epoch reconciles by operation ID instead of redispatching — count == 1.
#[tokio::test]
async fn e2e_crash_before_ack_reconciles_exactly_once() {
    let dir = tempfile::tempdir().unwrap();
    let runtime_dir = dir.path().join("runtime");
    let run_id = "01905c5e-0000-7000-8000-effec7000002";
    let mut scripts = HashMap::new();
    scripts.insert(RunId::from_str(run_id).unwrap(), script(EFFECT_OP));
    let mut effect_env = HashMap::new();
    effect_env.insert("FIXTURE_CRASH_BEFORE_RESPONSE".to_owned(), "1".to_owned());
    let daemon = boot_daemon_full(
        &runtime_dir,
        vec![make_fixture_bundle(dir.path())],
        scripts.clone(),
        effect_env,
    )
    .await;
    let socket = runtime_dir.join("control.sock");
    create_run(&socket, dir.path(), run_id).await;

    // Side effect landed; adapter died before the response could be acked.
    wait_store_len(&runtime_dir, 1, 10_000).await;
    wait_effect_state(&daemon, run_id, "Dispatched", 10_000).await;
    daemon.abort().await;

    // Restart under a higher fencing epoch WITHOUT the crash flag.
    let daemon =
        boot_daemon_scripts(&runtime_dir, vec![make_fixture_bundle(dir.path())], scripts).await;
    assert_eq!(wait_terminal(&socket, run_id).await, RUN_COMPLETED);
    assert_eq!(effect_rows(&daemon, run_id).await, vec!["Committed"]);
    // Exactly-once: the reconciled status observation committed the recorded
    // result; no second mutation ever ran.
    assert_eq!(store_counts(&runtime_dir), 1);
}

/// `fixture.unreconcilable_counter` declares an impossible reconciliation:
/// after the crash the row must go `Unknown` and the run blocks until an
/// operator resolves it through the Control API.
#[tokio::test]
async fn e2e_unreconcilable_effect_blocks_until_resolved() {
    let dir = tempfile::tempdir().unwrap();
    let runtime_dir = dir.path().join("runtime");
    let run_id = "01905c5e-0000-7000-8000-effec7000003";
    let mut scripts = HashMap::new();
    scripts.insert(RunId::from_str(run_id).unwrap(), script(UNRECONCILABLE_OP));
    let mut effect_env = HashMap::new();
    effect_env.insert("FIXTURE_CRASH_BEFORE_RESPONSE".to_owned(), "1".to_owned());
    let daemon = boot_daemon_full(
        &runtime_dir,
        vec![make_fixture_bundle(dir.path())],
        scripts.clone(),
        effect_env,
    )
    .await;
    let socket = runtime_dir.join("control.sock");
    create_run(&socket, dir.path(), run_id).await;
    wait_store_len(&runtime_dir, 1, 10_000).await;
    wait_effect_state(&daemon, run_id, "Dispatched", 10_000).await;
    daemon.abort().await;

    let daemon =
        boot_daemon_scripts(&runtime_dir, vec![make_fixture_bundle(dir.path())], scripts).await;
    // Startup recovery marks the unsafe dispatch Unknown; the run must stay
    // parked at WaitingTool — never auto-resumed, never retried.
    wait_effect_state(&daemon, run_id, "Unknown", 10_000).await;
    let run = wait_for_state(&socket, run_id, 4, 10_000).await;
    assert_eq!(run["state"], 4);
    {
        let rid: RunId = run_id.parse().unwrap();
        let mut txn = daemon.store().begin_read().await.unwrap();
        let row = txn.runs().get(rid).await.unwrap().unwrap();
        assert_eq!(
            row.recovery,
            domain::run::RecoveryDisposition::BlockedUnknownEffect
        );
    }

    let effect_id = {
        let run: RunId = run_id.parse().unwrap();
        let mut txn = daemon.store().begin_read().await.unwrap();
        txn.effects()
            .list_by_run(run)
            .await
            .unwrap()
            .first()
            .unwrap()
            .effect_id
            .to_string()
    };
    cli(
        &socket,
        &[
            "resolve-effect",
            "--effect-id",
            &effect_id,
            "--expected-state",
            &UNKNOWN_STATE.to_string(),
            "--action",
            "mark_succeeded",
            "--result-ref",
            "fixture://counter/1",
            "--reason",
            "operator verified the mutation",
        ],
    )
    .await;
    assert_eq!(wait_terminal(&socket, run_id).await, RUN_COMPLETED);
    assert_eq!(effect_rows(&daemon, run_id).await, vec!["Committed"]);
    assert_eq!(store_counts(&runtime_dir), 1);
}
