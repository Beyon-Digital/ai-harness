//! local-memory over the real adapter protocol: handshake identity,
//! memory.put/get round-trip as durable effects, and `status` reconcile
//! across a process restart.

use std::path::Path;
use std::time::{Duration, Instant};

use adapter_protocol::handshake::SessionPhase;
use adapter_protocol::{dispatch_call, write_frame};
use domain::generated::contract::{
    AdapterFrame, AdapterPing, EffectExecutionRequest, EffectExecutionResponse,
    EffectStatusRequest, EffectStatusResponse, PortCallRequest, adapter_frame::Body,
};
use domain::ids::AdapterId;
use process_supervisor::{SpawnSpec, handshake, spawn, terminate};
use prost::Message;
use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_local-memory");
const MANIFEST: &str = include_str!("../adapter.manifest.json");

struct Rig {
    _dir: TempDir,
    spec: SpawnSpec,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().expect("dir");
    let manifest: serde_json::Value = serde_json::from_str(MANIFEST).unwrap();
    let adapter_id: AdapterId = manifest["id"].as_str().unwrap().parse().unwrap();
    let store = dir.path().join("store.json").to_string_lossy().to_string();
    Rig {
        spec: SpawnSpec {
            adapter_id,
            adapter_version: manifest["version"].as_str().unwrap().to_owned(),
            expected_bundle_digest: "sha256:memory-fixture".to_owned(),
            adapter_instance_id: "01905c5e-0000-7000-8000-0000000000a1".parse().unwrap(),
            daemon_instance_id: "01905c5e-0000-7000-8000-0000000000d1".parse().unwrap(),
            daemon_fencing_epoch: 1,
            protocol_version: 1,
            executable: Path::new(BIN).to_path_buf(),
            argv: vec![],
            env: vec![
                ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
                ("FIXTURE_ADAPTER_ID".to_owned(), adapter_id.to_string()),
                (
                    "FIXTURE_ADAPTER_VERSION".to_owned(),
                    manifest["version"].as_str().unwrap().to_owned(),
                ),
                ("FIXTURE_STORE".to_owned(), store.clone()),
            ],
            cwd: None,
        },
        _dir: dir,
    }
}

fn call(
    child: &mut process_supervisor::Child,
    phase: &mut SessionPhase,
    call_id: &str,
    operation: &str,
    payload: impl Message,
) -> domain::generated::contract::PortCallResponse {
    dispatch_call(
        child.ipc(),
        phase,
        PortCallRequest {
            call_id: call_id.to_owned(),
            port_id: "effect.execute".to_owned(),
            operation: operation.to_owned(),
            context: None,
            payload: payload.encode_to_vec(),
        },
        Instant::now() + Duration::from_secs(3),
    )
    .expect("call")
}

fn exec_payload(op: &str, body: serde_json::Value) -> EffectExecutionRequest {
    let mut v = body;
    v["op"] = serde_json::Value::String(op.to_owned());
    EffectExecutionRequest {
        effect_id: "eff-1".to_owned(),
        operation_id: op.to_owned(),
        request_hash: "h".to_owned(),
        fencing_token: 1,
        payload: serde_json::to_vec(&v).unwrap(),
    }
}

fn exec_resp(resp: &domain::generated::contract::PortCallResponse) -> EffectExecutionResponse {
    EffectExecutionResponse::decode(resp.payload.as_slice()).expect("EffectExecutionResponse")
}

fn decode_data_uri(uri: &str) -> serde_json::Value {
    let b64 = uri
        .strip_prefix("data:application/json;base64,")
        .expect("json data uri");
    // Minimal decoder: the adapter emits unpadded base64.
    let bytes = {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut v = Vec::new();
        let mut acc = 0u32;
        let mut n = 0;
        for ch in b64.bytes().take_while(|&c| c != b'=') {
            acc = acc << 6 | T.iter().position(|&t| t == ch).unwrap() as u32;
            n += 1;
            if n == 4 {
                v.extend_from_slice(&acc.to_be_bytes()[1..]);
                acc = 0;
                n = 0;
            }
        }
        match n {
            2 => v.push((acc >> 4) as u8),
            3 => v.extend_from_slice(&(acc >> 2).to_be_bytes()[2..]),
            _ => {}
        }
        v
    };
    serde_json::from_slice(&bytes).expect("json")
}

#[tokio::test]
async fn memory_put_get_over_protocol() {
    let r = rig();
    let mut child = spawn(&r.spec).expect("spawn");
    let hello = handshake(&mut child, "n", 3_000).expect("handshake");
    assert_eq!(hello.adapter_id, r.spec.adapter_id.to_string());
    assert_eq!(hello.adapter_version, r.spec.adapter_version);

    // ping/pong mid-session
    write_frame(
        child.ipc(),
        &AdapterFrame {
            body: Some(Body::Ping(AdapterPing {
                nonce: "pp".to_owned(),
            })),
        },
    )
    .expect("ping");
    child
        .ipc()
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let pong = adapter_protocol::read_frame(child.ipc())
        .expect("frame")
        .and_then(|f| f.body);
    assert!(matches!(pong, Some(Body::Pong(_))));

    let mut phase = SessionPhase::Ready;
    let put = exec_resp(&call(
        &mut child,
        &mut phase,
        "c1",
        "execute",
        exec_payload(
            "memory.put",
            serde_json::json!({"namespace": "notes", "record": {"text": "hello"}}),
        ),
    ));
    assert_eq!(put.status, "succeeded");
    assert!(
        put.result_ref.starts_with("memory://notes/"),
        "put result_ref: {}",
        put.result_ref
    );
    let memory_id = put
        .result_ref
        .trim_start_matches("memory://notes/")
        .to_owned();

    let get = exec_resp(&call(
        &mut child,
        &mut phase,
        "c2",
        "execute",
        exec_payload(
            "memory.get",
            serde_json::json!({"namespace": "notes", "memory_id": memory_id}),
        ),
    ));
    assert_eq!(get.status, "succeeded");
    let envelope = decode_data_uri(&get.result_ref);
    assert_eq!(envelope["record"], serde_json::json!({"text": "hello"}));
    assert_eq!(envelope["sensitivity"], "internal");
    assert!(envelope["provenance"]["effect_id"].is_string());

    // duplicate execute replays the recorded result
    let replay = exec_resp(&call(
        &mut child,
        &mut phase,
        "c3",
        "execute",
        exec_payload(
            "memory.put",
            serde_json::json!({"namespace": "notes", "record": {"text": "other"}}),
        ),
    ));
    assert_eq!(replay.status, "succeeded");
    assert_eq!(replay.result_ref, put.result_ref);

    let _ = terminate(child, Duration::from_millis(300)).await;

    // Phase-10 authorities: a `secret` record cannot be downgraded by a
    // lower-class put, namespaces reject non-authority characters, and a
    // bad class name is invalid.
    let mut child = spawn(&r.spec).expect("respawn for sensitivity");
    handshake(&mut child, "n", 3_000).expect("handshake");
    let mut phase = SessionPhase::Ready;
    let mut secret_req = exec_payload(
        "memory.put",
        serde_json::json!({
            "namespace": "notes", "memory_id": "sec1",
            "record": {"k": 1}, "sensitivity": "secret",
        }),
    );
    secret_req.operation_id = "secret-op".to_owned();
    let secret = exec_resp(&call(&mut child, &mut phase, "s0", "execute", secret_req));
    assert_eq!(secret.status, "succeeded");
    let mut downgrade_req = exec_payload(
        "memory.put",
        serde_json::json!({
            "namespace": "notes", "memory_id": "sec1",
            "record": {"k": 2}, "sensitivity": "public",
        }),
    );
    downgrade_req.operation_id = "downgrade-op".to_owned();
    let downgrade = exec_resp(&call(
        &mut child,
        &mut phase,
        "s1d",
        "execute",
        downgrade_req,
    ));
    assert_eq!(downgrade.status, "failed");
    assert_eq!(downgrade.error_code, "sensitivity_downgrade");
    let mut raise_req = exec_payload(
        "memory.put",
        serde_json::json!({
            "namespace": "notes", "memory_id": "sec1",
            "record": {"k": 3}, "sensitivity": "secret",
        }),
    );
    raise_req.operation_id = "raise-op".to_owned();
    let raise = exec_resp(&call(&mut child, &mut phase, "s1r", "execute", raise_req));
    assert_eq!(raise.status, "succeeded");
    let mut bad_ns = exec_payload(
        "memory.put",
        serde_json::json!({"namespace": "../escape", "record": {}}),
    );
    bad_ns.operation_id = "badns-op".to_owned();
    let bad = exec_resp(&call(&mut child, &mut phase, "s2", "execute", bad_ns));
    assert_eq!(bad.status, "failed");
    let _ = terminate(child, Duration::from_millis(300)).await;

    // status reconcile across a restart: a fresh process reads the same store.
    let mut child = spawn(&r.spec).expect("respawn");
    handshake(&mut child, "n", 3_000).expect("handshake");
    let mut phase = SessionPhase::Ready;
    let status = call(
        &mut child,
        &mut phase,
        "s1",
        "status",
        EffectStatusRequest {
            effect_id: "eff-1".to_owned(),
            operation_id: "memory.get".to_owned(),
            provider_operation_ref: String::new(),
        },
    );
    let s = EffectStatusResponse::decode(status.payload.as_slice()).expect("EffectStatusResponse");
    assert_eq!(s.status, "succeeded");
    let _ = terminate(child, Duration::from_millis(300)).await;
}
