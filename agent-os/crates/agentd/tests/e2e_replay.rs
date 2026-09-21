//! `agentctl replay RUN_ID` — pages the run's event stream and decodes
//! each `LoopDecisionAccepted` payload into a readable summary.
#![cfg(unix)]

mod common;

use std::collections::HashMap;
use std::str::FromStr;

use common::*;
use domain::ids::RunId;

#[tokio::test]
async fn e2e_replay_decodes_run_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let runtime_dir = dir.path().join("runtime");
    let run_id = "01905c5e-0000-7000-8000-00de91e70001";
    let mut scripts = HashMap::new();
    scripts.insert(RunId::from_str(run_id).unwrap(), COMPLETE_SCRIPT.to_owned());
    let _daemon =
        boot_daemon_scripts(&runtime_dir, vec![make_fixture_bundle(dir.path())], scripts).await;

    let socket = runtime_dir.join("control.sock");
    let (session_id, spec_id, spec_digest) = make_session_and_spec(&socket, dir.path()).await;
    let args = create_run_args(&session_id, run_id, &spec_id, &spec_digest);
    cli(&socket, &args).await;
    assert_eq!(wait_terminal(&socket, run_id).await, RUN_COMPLETED);

    // The journal trails the run row slightly — poll until the accepted
    // decision is flushed into the stream.
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(15_000);
    let out = loop {
        let out = cli(&socket, &["replay", run_id]).await;
        let has_decision = out["events"]
            .as_array()
            .map(|e| e.iter().any(|e| e["event_type"] == "LoopDecisionAccepted"))
            .unwrap_or(false);
        if has_decision {
            break out;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "decision never journaled: {out}"
        );
        tokio::task::yield_now().await;
    };
    assert_eq!(out["run_id"], run_id);
    let events = out["events"].as_array().expect("events array");
    assert!(events.len() >= 3, "expected lifecycle events: {out}");
    // Sequences are strictly ordered starting at 1.
    for (i, e) in events.iter().enumerate() {
        assert_eq!(e["sequence"].as_u64().unwrap(), (i + 1) as u64);
    }
    // The completed run's LoopDecisionAccepted decodes to `complete`.
    let decoded: Vec<_> = events
        .iter()
        .filter(|e| e["event_type"] == "LoopDecisionAccepted")
        .collect();
    assert!(
        !decoded.is_empty(),
        "expected decoded decisions in {events:?}"
    );
    assert_eq!(decoded[0]["decision"]["kind"], "complete");
    assert_eq!(decoded[0]["decision"]["step_sequence"], 1);
}
