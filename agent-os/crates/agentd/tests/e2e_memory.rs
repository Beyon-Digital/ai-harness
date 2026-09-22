//! Memory ops end-to-end: `config/memory.yaml` binds `effect.execute` to
//! the `local-memory` adapter for the `local-memory` profile; a scripted
//! run performs `memory.put` then `memory.get` as durable, fenced,
//! reconciled effects — and the store survives on disk.
#![cfg(unix)]

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use agentd::bootstrap::{DaemonConfig, boot};
use common::*;
use domain::ids::RunId;
use kernel_store::KernelStore;
use serde_json::json;

const MEMORY_CFG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/memory.yaml");
const MEMORY_ADAPTER_UUID: &str = "01905c5e-0000-7000-8000-11a7c3d90003";
const MEMORY_MANIFEST: &str = include_str!("../../../fixtures/local-memory/adapter.manifest.json");

fn memory_binary() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/local-memory");
    assert!(
        path.exists(),
        "local-memory binary missing at {}; run `cargo build --workspace` first",
        path.display()
    );
    path
}

/// Assemble the local-memory bundle dir (manifest + entrypoint + lock).
fn make_memory_bundle(root: &Path) -> PathBuf {
    let bundle = root.join("local-memory-bundle");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(
        bundle.join(adapter_registry::manifest::MANIFEST_FILE),
        MEMORY_MANIFEST,
    )
    .unwrap();
    std::fs::copy(memory_binary(), bundle.join("local-memory")).unwrap();
    adapter_registry::write_lock(&bundle).unwrap();
    bundle
}

fn mem_script() -> String {
    let put = b64(serde_json::to_vec(&json!({
        "op": "memory.put",
        "namespace": "demo",
        "memory_id": "m1",
        "record": {"note": "hello world"},
    }))
    .unwrap()
    .as_slice());
    let get = b64(serde_json::to_vec(&json!({
        "op": "memory.get",
        "namespace": "demo",
        "memory_id": "m1",
    }))
    .unwrap()
    .as_slice());
    format!(
        r#"[{{"invoke_effect":{{"operation":"memory.put","payload":"{put}"}}}},{{"invoke_effect":{{"operation":"memory.get","payload":"{get}"}}}},{{"complete":{{"output_ref":"e2e://output/mem"}}}}]"#
    )
}

#[tokio::test]
async fn e2e_memory_put_get_as_durable_effects() {
    let dir = tempfile::tempdir().unwrap();
    let runtime_dir = dir.path().join("runtime");
    let run_id = "01905c5e-0000-7000-8000-00e05e700001";
    let mut scripts = HashMap::new();
    scripts.insert(RunId::from_str(run_id).unwrap(), mem_script());
    // memory.yaml contains both local-trusted (fixture effect) and
    // local-memory profiles — validation requires all three bundles.
    let bundles = vec![
        make_fixture_bundle(dir.path()),
        make_effect_bundle(dir.path()),
        make_memory_bundle(dir.path()),
    ];
    let daemon = boot(DaemonConfig {
        runtime_dir: runtime_dir.clone(),
        config_doc: Some(PathBuf::from(MEMORY_CFG)),
        adapter_bundles: bundles,
        loop_scripts: scripts,
        loop_env: HashMap::new(),
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
    let spec_path = dir.path().join("spec-mem.json");
    std::fs::write(&spec_path, &spec_body).unwrap();
    use sha2::Digest;
    let digest = sha2::Sha256::digest(&spec_body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let spec_id = "01999999-0000-7000-8000-00000000a0ee";
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
        ],
    )
    .await;

    assert_eq!(wait_terminal(&socket, run_id).await, RUN_COMPLETED);

    // Both memory ops committed as durable effect rows.
    let run: RunId = run_id.parse().unwrap();
    let mut txn = daemon.store().begin_read().await.expect("read txn");
    let rows = txn.effects().list_by_run(run).await.expect("list effects");
    assert_eq!(rows.len(), 2, "expected two effect rows: {rows:?}");
    assert!(rows.iter().all(|r| format!("{:?}", r.state) == "Committed"));
    let get = rows
        .iter()
        .find(|r| r.operation == "memory.get")
        .expect("memory.get row");
    let result_ref = get.result_ref.clone().unwrap_or_default();
    let b64body = result_ref
        .strip_prefix("data:application/json;base64,")
        .expect("json data uri");
    // The committed get result carries the stored record envelope.
    let decoded = {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let body: Vec<u8> = b64body.bytes().filter(|b| *b != b'=').collect();
        for chunk in body.chunks(4) {
            let v: Vec<u8> = chunk
                .iter()
                .filter_map(|b| T.iter().position(|t| t == b).map(|p| p as u8))
                .collect();
            let n = (u32::from(v[0]) << 18)
                | (u32::from(*v.get(1).unwrap_or(&0)) << 12)
                | v.get(2).map_or(0, |c| u32::from(*c) << 6)
                | v.get(3).map_or(0, |c| u32::from(*c));
            out.push((n >> 16) as u8);
            if v.len() > 2 {
                out.push((n >> 8) as u8);
            }
            if v.len() > 3 {
                out.push(n as u8);
            }
        }
        String::from_utf8(out).unwrap()
    };
    let record: serde_json::Value = serde_json::from_str(&decoded).unwrap();
    assert_eq!(record["record"]["note"], "hello world");
    // Phase-10 envelope: sensitivity class + write provenance.
    assert_eq!(record["sensitivity"], "internal");
    assert!(record["provenance"]["effect_id"].is_string());
    assert!(record["provenance"]["fencing_token"].is_number());
    assert!(record["provenance"]["written_at_ms"].is_number());

    // The durable store landed on disk under the runtime dir.
    let store: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            runtime_dir.join(format!("fixture-store-{MEMORY_ADAPTER_UUID}.json")),
        )
        .expect("memory store file"),
    )
    .unwrap();
    assert_eq!(
        store["memory"]["demo"]["m1"]["record"]["note"],
        "hello world"
    );
    assert_eq!(store["memory"]["demo"]["m1"]["sensitivity"], "internal");
}

/// Regression: a settled effect must be fed to the loop only on the turn
/// right after the decision that created it. After a `wait` resume, the
/// old effect must NOT be replayed in `LoopInput.events` — otherwise a
/// loop that parses the newest `model.chat` result would re-issue the
/// same decision forever. `FIXTURE_LOOP_RECORD_DIR` dumps each input.
#[tokio::test]
async fn e2e_settled_effects_are_fed_once_not_replayed() {
    let dir = tempfile::tempdir().unwrap();
    let runtime_dir = dir.path().join("runtime");
    let inputs_dir = dir.path().join("inputs");
    let run_id = "01905c5e-0000-7000-8000-00e05e700002";
    let put = b64(serde_json::to_vec(&json!({
        "op": "memory.put", "namespace": "replay",
        "memory_id": "k", "record": {"v": 1},
    }))
    .unwrap()
    .as_slice());
    let get = b64(serde_json::to_vec(&json!({
        "op": "memory.get", "namespace": "replay", "memory_id": "k",
    }))
    .unwrap()
    .as_slice());
    let t1 = "01905c5e-0000-7000-8000-00f1ef11aa01";
    let t2 = "01905c5e-0000-7000-8000-00f1ef11aa02";
    let script = format!(
        r#"[{{"invoke_effect":{{"operation":"memory.put","payload":"{put}"}}}},{{"wait":{{"reason":"pause","timer_id":"{t1}"}}}},{{"invoke_effect":{{"operation":"memory.get","payload":"{get}"}}}},{{"wait":{{"reason":"pause","timer_id":"{t2}"}}}},{{"complete":{{"output_ref":"e2e://output/replay"}}}}]"#
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
        config_doc: Some(PathBuf::from(MEMORY_CFG)),
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
    let spec_path = dir.path().join("spec-replay.json");
    std::fs::write(&spec_path, &spec_body).unwrap();
    use sha2::Digest;
    let digest = sha2::Sha256::digest(&spec_body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let spec_id = "01999999-0000-7000-8000-00000000a0ef";
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
        ],
    )
    .await;

    assert_eq!(wait_terminal(&socket, run_id).await, RUN_COMPLETED);

    let fed_events = |step: u32| -> Vec<serde_json::Value> {
        let file: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(inputs_dir.join(format!("turn-{step}.json")))
                .unwrap_or_else(|e| panic!("missing turn-{step}.json: {e}")),
        )
        .unwrap();
        let doc: serde_json::Value = serde_json::from_str(file["events"].as_str().unwrap_or("[]"))
            .unwrap_or_else(|_| panic!("turn-{step} events not JSON"));
        doc.as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e["operation"].as_str() != Some("kernel.op_counts"))
            .collect()
    };
    let fed_counts = |step: u32| -> serde_json::Value {
        let file: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(inputs_dir.join(format!("turn-{step}.json"))).unwrap(),
        )
        .unwrap();
        serde_json::from_str::<serde_json::Value>(file["events"].as_str().unwrap_or("[]"))
            .unwrap()
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|e| e["operation"].as_str() == Some("kernel.op_counts"))
            .and_then(|m| m.get("op_counts").cloned())
            .unwrap_or_default()
    };
    // Step 1's invoke_effect committed ⇒ fed to the step-2 turn.
    let turn2 = fed_events(2);
    assert_eq!(turn2.len(), 1, "turn 2 should see the put effect");
    assert_eq!(turn2[0]["operation"], "memory.put");
    // op_counts summarizes all run history and travels alongside real
    // settled entries — step 3's feed is empty (step 2 was `wait`), so
    // it carries no marker; step 4's feed shows both ops counted.
    let raw3: serde_json::Value = serde_json::from_str(
        &serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(inputs_dir.join("turn-3.json")).unwrap(),
        )
        .unwrap()["events"]
            .as_str()
            .unwrap_or("[]"),
    )
    .unwrap();
    assert_eq!(raw3, serde_json::json!([]), "empty feed stays a bare []");
    // Step 2 was `wait` — its outcome must not replay to the step-3 turn.
    assert_eq!(fed_events(3), Vec::<serde_json::Value>::new());
    // Step 3's invoke_effect ⇒ fed to step 4; the step-4 wait clears step 5.
    assert_eq!(fed_events(4).len(), 1);
    assert_eq!(fed_counts(4)["memory.put"], 1);
    assert_eq!(fed_counts(4)["memory.get"], 1);
    assert_eq!(fed_events(5), Vec::<serde_json::Value>::new());
}
