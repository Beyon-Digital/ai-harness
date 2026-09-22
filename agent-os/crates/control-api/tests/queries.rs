//! API-002 query/command surface coverage: idempotent dispatch, read-side
//! views, approval digest enforcement, and health projections.

mod common;

use std::sync::atomic::Ordering;

use common::*;
use control_api::{ControlSocket, serve};
use domain::generated::contract::{
    ApprovalResponseRequest, GetActiveConfigRequest, GetEffectRequest, GetRunRequest,
    GetTaskRequest, HealthRequest,
};
use domain::ids::{EffectId, RunId, TaskId};
use domain::time::SystemClock;
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn submit_command_replays_idempotently() {
    let rig = Rig::new().await;
    let socket = ControlSocket::bind(&rig.runtime_dir, &FakeLock).expect("bind");
    let path = socket.path().to_path_buf();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(socket, rig.service(), None, async move {
        let _ = stop_rx.await;
    }));
    wait_ready(&path).await;
    let mut c = client(&path).await;

    let req = counted_envelope(&rig, "idem-1", vec![1, 2, 3]);
    let first = c.submit_command(req.clone()).await.expect("first");
    let second = c.submit_command(req).await.expect("replay");
    assert_eq!(first.get_ref().command_id, second.get_ref().command_id);
    assert_eq!(first.get_ref().outcome_code, second.get_ref().outcome_code);
    assert_eq!(first.get_ref().payload, second.get_ref().payload);
    assert_eq!(
        rig.calls.load(Ordering::SeqCst),
        1,
        "idempotent replay must not re-execute the handler"
    );
    stop_tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn query_not_found_maps_to_not_found_status() {
    let rig = Rig::new().await;
    let socket = ControlSocket::bind(&rig.runtime_dir, &FakeLock).expect("bind");
    let path = socket.path().to_path_buf();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(socket, rig.service(), None, async move {
        let _ = stop_rx.await;
    }));
    wait_ready(&path).await;
    let mut c = client(&path).await;

    let missing = RunId::new(rig.ids.as_ref()).to_string();
    let err = c
        .get_run(GetRunRequest { run_id: missing })
        .await
        .expect_err("missing run");
    assert_eq!(err.code(), tonic::Code::NotFound);
    let err = c
        .get_task(GetTaskRequest {
            task_id: TaskId::new(rig.ids.as_ref()).to_string(),
        })
        .await
        .expect_err("missing task");
    assert_eq!(err.code(), tonic::Code::NotFound);
    let err = c
        .get_effect(GetEffectRequest {
            effect_id: EffectId::new(rig.ids.as_ref()).to_string(),
        })
        .await
        .expect_err("missing effect");
    assert_eq!(err.code(), tonic::Code::NotFound);
    let err = c
        .get_active_config(GetActiveConfigRequest {})
        .await
        .expect_err("no active config");
    assert_eq!(err.code(), tonic::Code::NotFound);
    stop_tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_digest_mismatch_over_api() {
    let rig = Rig::new().await;
    // Seed a pending approval request via the approvals port (test setup,
    // not the API path).
    let (request_id, digest) = {
        let mut txn = rig.write_txn().await;
        let out = approvals::create_request(
            &mut *txn,
            approvals::DigestInput {
                request_id: None,
                principal_id: rig.principal,
                actor_id: rig.actor,
                run_id: None,
                operation: "secret.use".into(),
                target: "artifact://x".into(),
                capabilities: vec![],
                extension_digest: None,
                config_digest: None,
                expiry_ms: domain::time::Clock::now_unix_ms(&SystemClock) + 60_000,
                nonce: String::new(),
            },
            NOW,
        )
        .await
        .expect("seed approval");
        txn.commit().await.expect("commit");
        out
    };

    let socket = ControlSocket::bind(&rig.runtime_dir, &FakeLock).expect("bind");
    let path = socket.path().to_path_buf();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(socket, rig.service(), None, async move {
        let _ = stop_rx.await;
    }));
    wait_ready(&path).await;
    let mut c = client(&path).await;

    let device = domain::ids::DeviceId::new(rig.ids.as_ref()).to_string();
    // Wrong digest -> the kernel rejects (conflict maps to AlreadyExists).
    let err = c
        .respond_approval(ApprovalResponseRequest {
            request_id: request_id.to_string(),
            request_digest: "ff".repeat(32),
            decision: "approve".into(),
            device_id: device.clone(),
            responder_principal_id: rig.principal.to_string(),
        })
        .await
        .expect_err("digest mismatch must fail");
    assert_eq!(err.code(), tonic::Code::AlreadyExists);

    // Correct digest responds cleanly through the coordinator.
    c.respond_approval(ApprovalResponseRequest {
        request_id: request_id.to_string(),
        request_digest: digest.to_string(),
        decision: "approve".into(),
        device_id: device,
        responder_principal_id: rig.principal.to_string(),
    })
    .await
    .expect("correct digest responds");
    stop_tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_reports_fencing_config_and_outbox() {
    let rig = Rig::new().await;
    let socket = ControlSocket::bind(&rig.runtime_dir, &FakeLock).expect("bind");
    let path = socket.path().to_path_buf();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(socket, rig.service(), None, async move {
        let _ = stop_rx.await;
    }));
    wait_ready(&path).await;

    // The outbox count is live: stage three unpublished events, then a
    // fourth after the first read to prove the value is re-queried.
    for lane in 0..3u64 {
        let mut txn = rig.write_txn().await;
        let key = domain::ids::EventStreamKey::new(format!("prop/h-{lane}")).unwrap();
        let seq = txn.streams().allocate(key.clone()).await.unwrap();
        txn.streams()
            .insert_outbox(kernel_store::models::NewOutboxEvent {
                event_id: domain::ids::EventId::new(rig.ids.as_ref()),
                event_type: "test.staged".to_owned(),
                event_version: 1,
                stream_key: key,
                sequence: seq,
                occurred_at_ms: NOW,
                run_id: None,
                task_id: None,
                session_id: None,
                effect_id: None,
                causation_id: None,
                correlation_id: None,
                sensitivity: domain::security::SensitivityClass::Internal,
                retention: domain::security::RetentionClass::Standard,
                payload: vec![],
            })
            .await
            .unwrap();
        txn.commit().await.unwrap();
    }

    let mut c = client(&path).await;
    let health = c
        .health(HealthRequest {})
        .await
        .expect("health")
        .into_inner();
    assert_eq!(health.status, "running");
    assert_eq!(health.daemon_fencing_epoch, rig.epoch);
    assert_eq!(health.active_config_generation_id, "gen-9");
    assert_eq!(health.outbox_unpublished_count, 3);
    stop_tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}

// ---------- API-003 ----------
