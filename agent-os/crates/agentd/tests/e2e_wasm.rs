//! Wasm adapter runtime end-to-end (Phase 13/extension model): a
//! `runtime.type = "wasm"` bundle's module runs inside
//! `agentos-wasm-host`; WASI stdio rides the same fd-0 socketpair the
//! native adapter protocol uses, and the fixture `wasm.echo` effect
//! adapter uppercases its payload. Requires the `wasm32-wasip1` target —
//! skipped (not failed) on toolchains without it.
#![cfg(unix)]

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use common::*;
use domain::ids::RunId;
use kernel_store::KernelStore;
use serde_json::json;

const WASM_CFG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/wasm.yaml");

/// Build `wasm-echo.wasm` for `wasm32-wasip1`, or `None` when the target
/// isn't installed (CI hosts without it skip this suite).
fn wasm_module() -> Option<PathBuf> {
    let installed = std::process::Command::new("rustc")
        .args(["--print", "target-libdir", "--target", "wasm32-wasip1"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !installed {
        eprintln!("wasm32-wasip1 target not installed — skipping wasm e2e");
        return None;
    }
    let status = std::process::Command::new("cargo")
        .args(["build", "-p", "wasm-echo", "--target", "wasm32-wasip1"])
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
        .status()
        .expect("cargo build wasm-echo");
    if !status.success() {
        eprintln!("wasm-echo build failed — skipping wasm e2e");
        return None;
    }
    let wasm = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../target/wasm32-wasip1/debug/wasm-echo.wasm"
    );
    Some(PathBuf::from(wasm))
}

fn make_wasm_bundle(root: &Path, module: &Path) -> PathBuf {
    let bundle = root.join("wasm-echo-bundle");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(
        bundle.join(adapter_registry::manifest::MANIFEST_FILE),
        include_str!("../../../fixtures/wasm-echo/adapter.manifest.json"),
    )
    .unwrap();
    std::fs::copy(module, bundle.join("wasm-echo.wasm")).unwrap();
    adapter_registry::write_lock(&bundle).unwrap();
    bundle
}

#[tokio::test]
async fn e2e_wasm_adapter_executes_effect() {
    let Some(module) = wasm_module() else { return };
    assert!(
        adapter_registry::wasm_host_binary().is_some(),
        "agentos-wasm-host binary missing — build the workspace first"
    );
    let dir = tempfile::tempdir().unwrap();
    let runtime_dir = dir.path().join("runtime");

    let run_id = "01905c5e-0000-7000-8000-00000000b001";
    let payload = b64(b"hello wasm sandbox");
    let script = format!(
        r#"[{{"invoke_effect":{{"operation":"wasm.echo","payload":"{payload}"}}}},{{"complete":{{"output_ref":"e2e://output/wasm"}}}}]"#
    );
    let mut scripts = HashMap::new();
    scripts.insert(RunId::from_str(run_id).unwrap(), script);

    let mut loop_env = HashMap::new();
    let _daemon = agentd::bootstrap::boot(agentd::bootstrap::DaemonConfig {
        runtime_dir: runtime_dir.clone(),
        config_doc: Some(PathBuf::from(WASM_CFG)),
        adapter_bundles: vec![
            make_fixture_bundle(dir.path()),
            make_effect_bundle(dir.path()),
            make_wasm_bundle(dir.path(), &module),
        ],
        loop_scripts: scripts,
        loop_env: std::mem::take(&mut loop_env),
        effect_env: HashMap::new(),
        poll: std::time::Duration::from_millis(10),
        json_logs: false,
    })
    .await
    .expect("daemon boot");

    let socket = runtime_dir.join("control.sock");
    let session = cli(&socket, &["create-session"]).await;
    let session_id = session["payload"].as_str().unwrap().to_owned();
    let spec_body = serde_json::to_vec(&json!({"runtime_profile_name": "local-wasm"})).unwrap();
    let spec_path = dir.path().join("spec-wasm.json");
    std::fs::write(&spec_path, &spec_body).unwrap();
    use sha2::Digest;
    let digest = sha2::Sha256::digest(&spec_body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let spec_id = "01999999-0000-7000-8000-00000000a0e1";
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
    std::fs::write(&payload_path, "wasm run").unwrap();
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
            "local-wasm",
            "--payload",
            payload_path.to_str().unwrap(),
        ],
    )
    .await;

    assert_eq!(wait_terminal(&socket, run_id).await, RUN_COMPLETED);

    // The wasm adapter's result row commits with the uppercased payload.
    let mut txn = _daemon.store().begin_read().await.expect("read txn");
    let effects = txn
        .effects()
        .list_by_run(RunId::from_str(run_id).unwrap())
        .await
        .expect("list effects");
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].operation, "wasm.echo");
    let decoded = decode_b64(
        effects[0]
            .result_ref
            .as_deref()
            .expect("result_ref")
            .strip_prefix("data:text/plain;base64,")
            .expect("text/plain ref"),
    );
    assert_eq!(decoded, b"HELLO WASM SANDBOX");
}

fn decode_b64(s: &str) -> Vec<u8> {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let bytes: Vec<u8> = s.bytes().filter(|b| *b != b'=').collect();
    for c in bytes.chunks(4) {
        let mut n: u32 = 0;
        for (i, b) in c.iter().enumerate() {
            n |= (T.iter().position(|t| t == b).unwrap() as u32) << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if c.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if c.len() > 3 {
            out.push(n as u8);
        }
    }
    out
}
