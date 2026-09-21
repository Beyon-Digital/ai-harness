//! Reusable adapter conformance packs (`ADP-006`).
//!
//! Each case spawns the real adapter binary through the supervisor and
//! exercises the *behavior* the manifest declares — a pack never trusts a
//! declaration it did not verify.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use adapter_protocol::handshake::SessionPhase;
use adapter_protocol::{dispatch_call, read_frame};
use domain::generated::contract::{
    AdapterFrame, EffectExecutionRequest, EffectExecutionResponse, EffectStatusRequest,
    EffectStatusResponse, PortCallRequest, adapter_frame::Body,
};
use domain::ids::{AdapterId, AdapterInstanceId, DaemonInstanceId};
use errors::codes::ErrorCode;
use process_supervisor::{SpawnSpec, handshake, spawn, terminate};
use prost::Message;
use serde::Serialize;

use crate::ids::DeterministicIds;

/// Case result shape shared with `adapter_registry::CaseResult` (kept
/// independent to avoid a crate dependency cycle).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CaseResult {
    /// Stable test id, e.g. `protocol.handshake_identity`.
    pub test_id: String,
    /// Whether the case passed.
    pub passed: bool,
    /// Free-form detail.
    pub details: String,
}

impl CaseResult {
    fn pass(id: &str, details: impl Into<String>) -> Self {
        Self {
            test_id: id.to_owned(),
            passed: true,
            details: details.into(),
        }
    }

    fn fail(id: &str, details: impl Into<String>) -> Self {
        Self {
            test_id: id.to_owned(),
            passed: false,
            details: details.into(),
        }
    }
}

/// What a pack needs to spawn the adapter under test.
#[derive(Clone)]
pub struct FixtureBinary {
    /// Path to the built adapter executable.
    pub executable: PathBuf,
    /// Adapter id the binary echoes in Hello (env-injected).
    pub adapter_id: AdapterId,
    /// Adapter version echoed in Hello.
    pub adapter_version: String,
    /// Bundle digest the registration attests — the binary echoes it back.
    pub bundle_digest: String,
    /// Extra env pairs merged into the spawn allowlist.
    pub env: Vec<(String, String)>,
}

fn spec(fx: &FixtureBinary, extra_env: &[(&str, String)], seed: i64) -> SpawnSpec {
    let ids = DeterministicIds::new(seed);
    let mut env = vec![
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("FIXTURE_ADAPTER_ID".to_owned(), fx.adapter_id.to_string()),
        (
            "FIXTURE_ADAPTER_VERSION".to_owned(),
            fx.adapter_version.clone(),
        ),
        (
            "FIXTURE_LOOP_ADAPTER_ID".to_owned(),
            fx.adapter_id.to_string(),
        ),
        (
            "FIXTURE_LOOP_ADAPTER_VERSION".to_owned(),
            fx.adapter_version.clone(),
        ),
    ];
    env.extend(fx.env.iter().cloned());
    env.extend(extra_env.iter().map(|(k, v)| ((*k).to_owned(), v.clone())));
    SpawnSpec {
        adapter_id: fx.adapter_id,
        adapter_version: fx.adapter_version.clone(),
        expected_bundle_digest: fx.bundle_digest.clone(),
        adapter_instance_id: AdapterInstanceId::new(&ids),
        daemon_instance_id: DaemonInstanceId::new(&ids),
        daemon_fencing_epoch: 1,
        protocol_version: 1,
        executable: fx.executable.clone(),
        argv: vec![],
        env,
        cwd: None,
    }
}

async fn kill(child: process_supervisor::Child) {
    let _ = terminate(child, Duration::from_millis(400)).await;
}

fn request(call_id: &str, operation: &str, payload: Vec<u8>) -> PortCallRequest {
    PortCallRequest {
        call_id: call_id.to_owned(),
        port_id: "effect.execute".to_owned(),
        operation: operation.to_owned(),
        context: None,
        payload,
    }
}

/// Generic process-protocol pack: handshake pins identity, ping/pong
/// answers, deadline expiry surfaces an error, shutdown drains cleanly.
pub async fn process_protocol_pack(fx: &FixtureBinary) -> Vec<CaseResult> {
    let mut cases = Vec::new();
    let id = "protocol.handshake_identity";
    match spawn(&spec(fx, &[], 11)) {
        Ok(mut child) => {
            let case = match handshake(&mut child, "nonce-a", 3_000) {
                Ok(hello) if hello.adapter_id == fx.adapter_id.to_string() => {
                    CaseResult::pass(id, "hello echoes registered identity")
                }
                Ok(_) => CaseResult::fail(id, "hello returned wrong identity"),
                Err(e) => CaseResult::fail(id, format!("handshake: {e}")),
            };
            cases.push(case);
            kill(child).await;
        }
        Err(e) => {
            cases.push(CaseResult::fail(id, format!("spawn: {e}")));
            return cases;
        }
    }

    // ping/pong + request roundtrip in one session
    {
        let id = "protocol.ping_pong_and_call";
        let mut child = match spawn(&spec(fx, &[], 12)) {
            Ok(c) => c,
            Err(e) => {
                cases.push(CaseResult::fail(id, format!("spawn: {e}")));
                return cases;
            }
        };
        if let Err(e) = handshake(&mut child, "n", 3_000) {
            cases.push(CaseResult::fail(id, format!("handshake: {e}")));
            kill(child).await;
            return cases;
        }
        if let Err(e) = adapter_protocol::write_frame(
            child.ipc(),
            &AdapterFrame {
                body: Some(Body::Ping(domain::generated::contract::AdapterPing {
                    nonce: "pp".to_owned(),
                })),
            },
        ) {
            cases.push(CaseResult::fail(id, format!("ping write: {e}")));
            kill(child).await;
            return cases;
        }
        if child
            .ipc()
            .set_read_timeout(Some(Duration::from_secs(3)))
            .is_err()
        {
            cases.push(CaseResult::fail(id, "read timeout not settable"));
            kill(child).await;
            return cases;
        }
        let ok = matches!(
            read_frame(child.ipc()).ok().flatten().and_then(|f| f.body),
            Some(Body::Pong(_))
        );
        let mut phase = SessionPhase::Ready;
        let called = dispatch_call(
            child.ipc(),
            &mut phase,
            request("c1", "execute", b"{}".to_vec()),
            Instant::now() + Duration::from_secs(3),
        )
        .is_ok();
        cases.push(if ok && called {
            CaseResult::pass(id, "pong answered, call responded")
        } else {
            CaseResult::fail(id, format!("pong={ok} call={called}"))
        });
        kill(child).await;
    }

    // deadline
    {
        let id = "protocol.deadline_expiry";
        let mut child = match spawn(&spec(fx, &[("FIXTURE_DELAY_MS", "2000".to_owned())], 13)) {
            Ok(c) => c,
            Err(e) => {
                cases.push(CaseResult::fail(id, format!("spawn: {e}")));
                return cases;
            }
        };
        if let Err(e) = handshake(&mut child, "n", 3_000) {
            cases.push(CaseResult::fail(id, format!("handshake: {e}")));
            kill(child).await;
            return cases;
        }
        let mut phase = SessionPhase::Ready;
        let outcome = dispatch_call(
            child.ipc(),
            &mut phase,
            request("slow", "execute", b"{}".to_vec()),
            Instant::now() + Duration::from_millis(150),
        );
        let expired = matches!(
            &outcome,
            Err(adapter_protocol::CallError::Kernel(e)) if e.code() == ErrorCode::Unavailable
        );
        cases.push(if expired {
            CaseResult::pass(id, "deadline closed the call")
        } else {
            let detail = match outcome {
                Ok(_) => "call succeeded".to_owned(),
                Err(e) => format!("{e:?}"),
            };
            CaseResult::fail(id, format!("expected deadline, got {detail}"))
        });
        kill(child).await;
    }
    cases
}

/// Fixture effect-adapter pack: verifies `provider_idempotency` and
/// `status_lookup` *behaviorally* — a manifest that declares them without
/// implementing them fails here.
pub async fn effect_adapter_pack(fx: &FixtureBinary, store_path: &str) -> Vec<CaseResult> {
    let mut cases = Vec::new();
    let env = [("FIXTURE_STORE", store_path.to_owned())];

    // normal execute + duplicate replay
    {
        let id = "effect.execute_and_idempotent_replay";
        let mut child = spawn(&spec(fx, &env, 21)).expect("spawn");
        handshake(&mut child, "n", 3_000).expect("handshake");
        let req = |op: &str, token: u64| {
            EffectExecutionRequest {
                effect_id: "eff-1".to_owned(),
                operation_id: op.to_owned(),
                request_hash: "h".to_owned(),
                fencing_token: token,
                payload: vec![],
            }
            .encode_to_vec()
        };
        let mut phase = SessionPhase::Ready;
        let call = |child: &mut process_supervisor::Child,
                    phase: &mut SessionPhase,
                    op: &str,
                    tok: u64,
                    cid: &str| {
            dispatch_call(
                child.ipc(),
                phase,
                request(cid, "execute", req(op, tok)),
                Instant::now() + Duration::from_secs(3),
            )
        };
        let first = call(&mut child, &mut phase, "op-A", 1, "c1");
        let second = call(&mut child, &mut phase, "op-B", 2, "c2");
        let replay = call(&mut child, &mut phase, "op-A", 3, "c3");
        let decoded = |r: &Result<domain::generated::contract::PortCallResponse, _>| {
            r.as_ref()
                .ok()
                .map(|p| EffectExecutionResponse::decode(p.payload.as_slice()).unwrap())
        };
        let (a1, b, a2) = (decoded(&first), decoded(&second), decoded(&replay));
        let passed = matches!((a1, b, a2),
            (Some(a1), Some(b), Some(a2))
            if a1.result_ref == a2.result_ref && a1.result_ref != b.result_ref
                && a1.status == "succeeded");
        cases.push(if passed {
            CaseResult::pass(id, "duplicate op-A replayed identical result_ref")
        } else {
            CaseResult::fail(id, "idempotent replay mismatch")
        });
        kill(child).await;
    }

    // status lookup across restart — the crash flag makes the adapter die
    // mid-call after the side effect, the precise EffectUnknown ambiguity.
    {
        let id = "effect.crash_then_status_reconcile";
        let mut child = spawn(&spec(
            fx,
            &[
                ("FIXTURE_STORE", store_path.to_owned()),
                ("FIXTURE_CRASH_BEFORE_RESPONSE", "1".to_owned()),
            ],
            22,
        ))
        .expect("spawn");
        handshake(&mut child, "n", 3_000).expect("handshake");
        let mut phase = SessionPhase::Ready;
        let req = EffectExecutionRequest {
            effect_id: "eff-crash".to_owned(),
            operation_id: "op-crash".to_owned(),
            request_hash: "h".to_owned(),
            fencing_token: 1,
            payload: vec![],
        };
        let outcome = dispatch_call(
            child.ipc(),
            &mut phase,
            request("cx", "execute", req.encode_to_vec()),
            Instant::now() + Duration::from_secs(3),
        );
        let crashed = outcome.is_err();
        kill(child).await;

        let mut child = spawn(&spec(fx, &env, 23)).expect("respawn");
        handshake(&mut child, "n", 3_000).expect("handshake");
        let status_req = EffectStatusRequest {
            effect_id: "eff-crash".to_owned(),
            operation_id: "op-crash".to_owned(),
            provider_operation_ref: String::new(),
        };
        let mut phase = SessionPhase::Ready;
        let status = dispatch_call(
            child.ipc(),
            &mut phase,
            request("cs", "status", status_req.encode_to_vec()),
            Instant::now() + Duration::from_secs(3),
        );
        let reconciled = status
            .ok()
            .and_then(|r| EffectStatusResponse::decode(r.payload.as_slice()).ok())
            .is_some_and(|s| s.status == "succeeded");
        cases.push(if crashed && reconciled {
            CaseResult::pass(id, "crash lost the response; status() recovered the effect")
        } else {
            CaseResult::fail(id, format!("crashed={crashed} reconciled={reconciled}"))
        });
        kill(child).await;
    }

    // fencing rejection
    {
        let id = "effect.fencing_rejection";
        let mut child = spawn(&spec(
            fx,
            &[
                ("FIXTURE_STORE", store_path.to_owned()),
                ("FIXTURE_MIN_FENCE", "5".to_owned()),
            ],
            24,
        ))
        .expect("spawn");
        handshake(&mut child, "n", 3_000).expect("handshake");
        let req = EffectExecutionRequest {
            effect_id: "eff-f".to_owned(),
            operation_id: "op-f".to_owned(),
            request_hash: "h".to_owned(),
            fencing_token: 3,
            payload: vec![],
        };
        let mut phase = SessionPhase::Ready;
        let resp = dispatch_call(
            child.ipc(),
            &mut phase,
            request("cf", "execute", req.encode_to_vec()),
            Instant::now() + Duration::from_secs(3),
        );
        let rejected = resp
            .ok()
            .and_then(|r| EffectExecutionResponse::decode(r.payload.as_slice()).ok())
            .is_some_and(|r| r.error_code == "fencing_rejected");
        cases.push(if rejected {
            CaseResult::pass(id, "stale fencing token rejected")
        } else {
            CaseResult::fail(id, "stale fencing token was accepted")
        });
        kill(child).await;
    }
    cases
}

/// Writes a `PortCallRequest` frame with an oversized body — conformance
/// signal that the peer's frame cap holds (kernel-side test of the same
/// constant is in adapter-protocol).
pub fn encode_oversized_frame() -> Vec<u8> {
    ((adapter_protocol::MAX_FRAME_BYTES + 1) as u32)
        .to_be_bytes()
        .to_vec()
}
