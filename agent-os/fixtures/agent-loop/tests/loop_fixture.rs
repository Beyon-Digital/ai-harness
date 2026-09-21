//! LOOP-001: the fixture AgentLoop process over the private protocol —
//! deterministic scripted decisions echoing run revision/epoch/step/
//! cursor/turn, plus delayed and stale-response behavior.

use std::path::Path;
use std::time::{Duration, Instant};

use adapter_protocol::handshake::SessionPhase;
use adapter_protocol::{CallError, dispatch_call};
use adapter_registry::manifest::manifest_digest;
use adapter_registry::registry::{compute_bundle_digest, write_lock};
use domain::generated::contract::{LoopDecision, LoopInput, PortCallRequest, loop_decision};
use domain::ids::AdapterId;
use process_supervisor::{SpawnSpec, handshake, spawn, terminate};
use prost::Message;
use tempfile::TempDir;
use testkit::ids::DeterministicIds;

const BIN: &str = env!("CARGO_BIN_EXE_fixture-agent-loop");
const MANIFEST: &str = include_str!("../adapter.manifest.json");
const SEED: i64 = 1_700_000_000_000;

struct Rig {
    /// Keeps the tempdir alive for the Rig's lifetime.
    _dir: TempDir,
    spec: SpawnSpec,
}

fn rig(script: &str, extra_env: &[(&str, &str)]) -> Rig {
    let dir = tempfile::tempdir().expect("dir");
    let manifest: serde_json::Value = serde_json::from_str(MANIFEST).unwrap();
    let adapter_id: AdapterId = manifest["id"].as_str().unwrap().parse().unwrap();
    let mut env = vec![
        ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
        ("FIXTURE_LOOP_SCRIPT".to_owned(), script.to_owned()),
        ("FIXTURE_LOOP_ADAPTER_ID".to_owned(), adapter_id.to_string()),
        (
            "FIXTURE_LOOP_ADAPTER_VERSION".to_owned(),
            manifest["version"].as_str().unwrap().to_owned(),
        ),
    ];
    env.extend(
        extra_env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string())),
    );
    let ids = DeterministicIds::new(SEED);
    Rig {
        spec: SpawnSpec {
            adapter_id,
            adapter_version: manifest["version"].as_str().unwrap().to_owned(),
            expected_bundle_digest: "sha256:loop-fixture".to_owned(),
            adapter_instance_id: domain::ids::AdapterInstanceId::new(&ids),
            daemon_instance_id: domain::ids::DaemonInstanceId::new(&ids),
            daemon_fencing_epoch: 9,
            protocol_version: 1,
            executable: Path::new(BIN).to_path_buf(),
            argv: vec![],
            env,
            cwd: None,
            isolation: process_supervisor::spawn::Isolation::None,
        },
        _dir: dir,
    }
}

fn input(step: u64) -> PortCallRequest {
    PortCallRequest {
        call_id: format!("call-{step}"),
        port_id: "agent_loop".to_owned(),
        operation: "next".to_owned(),
        context: None,
        payload: LoopInput {
            run_id: "run-1".to_owned(),
            run_revision: 42,
            loop_epoch: 7,
            step_sequence: step,
            input_event_cursor: "cursor-17".to_owned(),
            turn_id: "turn-3".to_owned(),
            state: vec![],
            events: vec![],
        }
        .encode_to_vec(),
    }
}

fn decision(resp: &domain::generated::contract::PortCallResponse) -> LoopDecision {
    LoopDecision::decode(resp.payload.as_slice()).expect("LoopDecision")
}

#[tokio::test]
async fn normal_complete_decision() {
    let r = rig(r#"[{"complete":{"output_ref":"art://done"}}]"#, &[]);
    let mut child = spawn(&r.spec).expect("spawn");
    handshake(&mut child, "n", 3_000).expect("handshake");
    let mut phase = SessionPhase::Ready;
    let resp = dispatch_call(
        child.ipc(),
        &mut phase,
        input(1),
        Instant::now() + Duration::from_secs(3),
    )
    .expect("call");
    let d = decision(&resp);
    assert_eq!(d.run_revision, 42);
    assert_eq!(d.loop_epoch, 7);
    assert_eq!(d.step_sequence, 1);
    assert_eq!(d.input_event_cursor, "cursor-17");
    assert_eq!(d.turn_id, "turn-3");
    // decision_id echoes the issued turn id so retries of the same turn replay.
    assert_eq!(d.decision_id, "turn-3");
    let Some(loop_decision::Decision::Complete(c)) = d.decision else {
        panic!("expected Complete");
    };
    assert_eq!(c.output_ref, "art://done");
    let _ = terminate(child, Duration::from_millis(300)).await;
}

#[tokio::test]
async fn spawn_child_and_effect_decisions() {
    let r = rig(
        r#"[
            {"spawn_agent":{"child_request":"aGVsbG8="}},
            {"invoke_effect":{"operation":"fs.write","payload":"aGk=","effect_claim":"Y2xhaW0="}},
            {"wait":{"reason":"timer","timer_id":"t-9"}},
            {"request_approval":{"approval_draft":"ZHJhZnQ="}}
        ]"#,
        &[],
    );
    let mut child = spawn(&r.spec).expect("spawn");
    handshake(&mut child, "n", 3_000).expect("handshake");
    let mut phase = SessionPhase::Ready;
    let mut call_at = |step: u64| {
        dispatch_call(
            child.ipc(),
            &mut phase,
            input(step),
            Instant::now() + Duration::from_secs(3),
        )
        .expect("call")
    };
    let Some(loop_decision::Decision::SpawnAgent(s)) = decision(&call_at(1)).decision else {
        panic!("expected SpawnAgent");
    };
    assert_eq!(s.child_request, b"hello");
    let Some(loop_decision::Decision::InvokeEffect(e)) = decision(&call_at(2)).decision else {
        panic!("expected InvokeEffect");
    };
    assert_eq!(e.operation, "fs.write");
    assert_eq!(e.payload, b"hi");
    assert_eq!(e.effect_claim, b"claim");
    let Some(loop_decision::Decision::Wait(w)) = decision(&call_at(3)).decision else {
        panic!("expected Wait");
    };
    assert_eq!(w.timer_id, "t-9");
    assert!(matches!(
        decision(&call_at(4)).decision,
        Some(loop_decision::Decision::RequestApproval(_))
    ));
    let _ = terminate(child, Duration::from_millis(300)).await;
}

#[tokio::test]
async fn delayed_and_stale_decisions() {
    // delayed: a 400ms adapter delay vs a 100ms deadline → deadline error
    let r = rig(
        r#"[{"complete":{"output_ref":"x"}}]"#,
        &[("FIXTURE_LOOP_DELAY_MS", "400")],
    );
    let mut child = spawn(&r.spec).expect("spawn");
    handshake(&mut child, "n", 3_000).expect("handshake");
    let mut phase = SessionPhase::Ready;
    let outcome = dispatch_call(
        child.ipc(),
        &mut phase,
        input(1),
        Instant::now() + Duration::from_millis(100),
    );
    assert!(matches!(outcome, Err(CallError::Kernel(_))));
    let _ = terminate(child, Duration::from_millis(300)).await;

    // stale: the fixture reports a lowered loop epoch
    let r = rig(
        r#"[{"complete":{"output_ref":"x"}}]"#,
        &[("FIXTURE_LOOP_STALE", "1")],
    );
    let mut child = spawn(&r.spec).expect("spawn");
    handshake(&mut child, "n", 3_000).expect("handshake");
    let mut phase = SessionPhase::Ready;
    let resp = dispatch_call(
        child.ipc(),
        &mut phase,
        input(1),
        Instant::now() + Duration::from_secs(3),
    )
    .expect("call");
    assert_eq!(decision(&resp).loop_epoch, 6);
    let _ = terminate(child, Duration::from_millis(300)).await;
}

#[tokio::test]
async fn bundle_digest_validation() {
    // The fixture is packageable as an immutable bundle: register +
    // spawn-time verify accepts, mutation rejects.
    let bundle = tempfile::tempdir().expect("bundle");
    std::fs::write(bundle.path().join("adapter.manifest.json"), MANIFEST).expect("manifest");
    std::fs::copy(BIN, bundle.path().join("fixture-agent-loop")).expect("bin");
    write_lock(bundle.path()).expect("lock");
    let md = manifest_digest(MANIFEST.as_bytes());
    let d1 = compute_bundle_digest(bundle.path(), &md).expect("d1");
    std::fs::write(bundle.path().join("mutated.txt"), "x").expect("mut");
    write_lock(bundle.path()).expect("relock");
    let d2 = compute_bundle_digest(bundle.path(), &md).expect("d2");
    assert_ne!(d1, d2);
}
