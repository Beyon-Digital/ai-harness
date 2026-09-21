//! INT-001: process-level end-to-end — real daemon boot, API-driven run
//! lifecycle through the real fixture loop adapter, restart retention, and
//! idle shutdown. Nothing reaches into the store to force progress.
#![cfg(unix)]

mod common;

use std::collections::HashMap;
use std::time::Instant;

use common::*;
use domain::ids::RunId;

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
