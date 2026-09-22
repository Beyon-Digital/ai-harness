//! Context strategy end-to-end (Phase 10): `limits.context` in the
//! active generation caps what a turn's `LoopInput` carries — the run's
//! state snapshot is truncated to `max_state_bytes`, and the
//! settled-effects batch is dropped when it exceeds
//! `max_fed_event_bytes`.
#![cfg(unix)]

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use agentd::bootstrap::{DaemonConfig, boot};
use common::*;
use domain::ids::RunId;
use serde_json::json;

const MEMORY_CFG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/memory.yaml");

fn make_memory_bundle(root: &Path) -> PathBuf {
    let bundle = root.join("local-memory-bundle");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(
        bundle.join(adapter_registry::manifest::MANIFEST_FILE),
        include_str!("../../../fixtures/local-memory/adapter.manifest.json"),
    )
    .unwrap();
    let bin = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/local-memory");
    std::fs::copy(bin, bundle.join("local-memory")).unwrap();
    adapter_registry::write_lock(&bundle).unwrap();
    bundle
}

#[tokio::test]
async fn e2e_context_limits_shape_loop_input() {
    let dir = tempfile::tempdir().unwrap();
    let runtime_dir = dir.path().join("runtime");
    let inputs_dir = dir.path().join("inputs");

    // Patch the shipped config's context caps down so the test payload
    // exceeds them.
    let yaml = std::fs::read_to_string(MEMORY_CFG)
        .unwrap()
        .replace("max_state_bytes: 65536", "max_state_bytes: 40")
        .replace("max_fed_event_bytes: 65536", "max_fed_event_bytes: 20");
    let cfg = dir.path().join("context.yaml");
    std::fs::write(&cfg, &yaml).unwrap();

    let run_id = "01905c5e-0000-7000-8000-00c07e700001";
    let put = b64(serde_json::to_vec(&json!({
        "op": "memory.put", "namespace": "ctx",
        "memory_id": "k", "record": {"v": 1},
    }))
    .unwrap()
    .as_slice());
    let script = format!(
        r#"[{{"invoke_effect":{{"operation":"memory.put","payload":"{put}"}}}},{{"complete":{{"output_ref":"e2e://output/ctx"}}}}]"#
    );
    let mut scripts = HashMap::new();
    scripts.insert(RunId::from_str(run_id).unwrap(), script);
    let mut loop_env = HashMap::new();
    loop_env.insert(
        "FIXTURE_LOOP_RECORD_DIR".to_owned(),
        inputs_dir.to_str().unwrap().to_owned(),
    );
    let bundles = vec![
        make_fixture_bundle(dir.path()),
        make_effect_bundle(dir.path()),
        make_memory_bundle(dir.path()),
    ];
    let _daemon = boot(DaemonConfig {
        runtime_dir: runtime_dir.clone(),
        config_doc: Some(cfg),
        adapter_bundles: bundles,
        loop_scripts: scripts,
        loop_env,
        effect_env: HashMap::new(),
        poll: std::time::Duration::from_millis(10),
        json_logs: false,
    })
    .await
    .expect("daemon boot");

    let socket = runtime_dir.join("control.sock");
    let session = cli(&socket, &["create-session"]).await;
    let session_id = session["payload"].as_str().unwrap().to_owned();
    let spec_body = serde_json::to_vec(&json!({"runtime_profile_name": "local-memory"})).unwrap();
    let spec_path = dir.path().join("spec-ctx.json");
    std::fs::write(&spec_path, &spec_body).unwrap();
    use sha2::Digest;
    let digest = sha2::Sha256::digest(&spec_body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let spec_id = "01999999-0000-7000-8000-00000000a0c1";
    cli(
        &socket,
        &[
            "put-agent-spec",
            spec_id,
            "--version",
            "v1",
            "--body",
            spec_path.to_str().unwrap(),
        ],
    )
    .await;
    let payload_path = dir.path().join("payload.txt");
    std::fs::write(&payload_path, "x".repeat(300)).unwrap();
    cli(
        &socket,
        &[
            "create-run",
            "--run-id",
            run_id,
            "--session-id",
            &session_id,
            "--agent-spec-id",
            spec_id,
            "--spec-version",
            "v1",
            "--spec-digest",
            &digest,
            "--task-id",
            run_id,
            "--task-kind",
            "agent",
            "--profile",
            "local-memory",
            "--payload",
            payload_path.to_str().unwrap(),
        ],
    )
    .await;

    assert_eq!(wait_terminal(&socket, run_id).await, RUN_COMPLETED);

    let turn1: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(inputs_dir.join("turn-1.json")).expect("turn-1.json"),
    )
    .unwrap();
    // The 300-byte task payload is truncated to the generation's cap.
    let state = turn1["state"].as_str().unwrap_or_default();
    assert_eq!(state.len(), 40, "state should be capped: {state:?}");

    // Turn 2's fed-effects batch serializes to >20 bytes → dropped
    // entirely (even the bare `[{settled}]` array can't fit).
    let turn2: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(inputs_dir.join("turn-2.json")).expect("turn-2.json"),
    )
    .unwrap();
    let events = turn2["events"].as_str().unwrap_or_default();
    assert!(
        events.is_empty() || !events.contains("memory."),
        "settled batch must drop under the cap: {events:?}"
    );
}
