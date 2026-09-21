//! LOOP-002: durable turn issue, fenced decision acceptance, idempotent
//! replay, stale rejection, loop_epoch restart bump.

use domain::generated::contract::{Complete, InvokeEffect, LoopDecision, loop_decision};
use domain::ids::{
    ActorId, AdapterId, CommandId, DaemonInstanceId, DecisionId, EnvironmentId, PrincipalId, RunId,
    TaskId,
};
use domain::resource::WorkspaceAccessMode;
use domain::run::RunState;
use domain::security::{ConformanceState, TrustState};
use domain::time::SystemClock;
use errors::codes::ErrorCode;
use kernel_store::models::NewRun;
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use prost::Message;
use runtime::decision::{self, AcceptOutcome, DecisionInstruction};
use runtime::loop_turn::{self, turn_state};
use tempfile::TempDir;
use testkit::ids::DeterministicIds;

const SEED: i64 = 1_700_000_000_000;
const NOW: i64 = 1_700_000_000_000;

async fn open() -> (SqliteKernelStore, u64, TempDir) {
    let dir = tempfile::tempdir().expect("tmp");
    let store = SqliteKernelStore::open(StoreConfig {
        path: dir.path().join("k.db"),
        pool_max_connections: 4,
        busy_timeout_ms: 5_000,
    })
    .await
    .expect("store");
    let ids = DeterministicIds::new(SEED);
    let epoch = store
        .acquire_daemon_fence(DaemonInstanceId::new(&ids))
        .await
        .expect("fence")
        .epoch
        .0;
    (store, epoch, dir)
}

async fn txn<'s>(store: &'s SqliteKernelStore, epoch: u64) -> Box<dyn KernelTxn + 's> {
    let ids = DeterministicIds::new(SEED + 1);
    store
        .begin_write(TxContext {
            daemon_epoch: epoch,
            principal_id: PrincipalId::new(&ids),
            command_id: CommandId::new(&ids),
            correlation_id: None,
        })
        .await
        .expect("txn")
}

async fn insert_run(tx: &mut dyn KernelTxn, ids: &DeterministicIds) -> RunId {
    let run = RunId::new(ids);
    let task_id = TaskId::new(ids);
    runtime::task::ensure_task(
        tx,
        kernel_store::models::NewTask {
            task_id,
            session_id: None,
            created_by_actor_id: ActorId::new(ids),
            task_kind: "test".into(),
            payload: b"{}".to_vec(),
            created_at_ms: NOW,
        },
    )
    .await
    .expect("task");
    tx.runs()
        .insert(NewRun {
            run_id: run,
            task_id,
            session_id: None,
            parent_run_id: None,
            state: RunState::Running,
            recovery: domain::run::RecoveryDisposition::Normal,
            loop_epoch: 0,
            step_sequence: 0,
            input_event_cursor: domain::ids::EventCursor::new(
                domain::ids::EventStreamKey::new(format!("run/{run}")).unwrap(),
                0,
            ),
            cancellation_epoch: 0,
            resolved_environment_id: None,
            agent_spec_id: None,
            agent_spec_version: None,
            agent_spec_digest: None,
            requested_profile: String::new(),
            workspace_uri: None,
            created_at_ms: NOW,
        })
        .await
        .expect("run");
    run
}

/// Seeds a frozen environment with an `effect.execute` binding and the
/// adapter registration it points at, then attaches it to `run`.
async fn bind_effect_environment(
    tx: &mut dyn KernelTxn,
    ids: &DeterministicIds,
    run: RunId,
) -> EnvironmentId {
    let environment = EnvironmentId::new(ids);
    let adapter = AdapterId::new(ids);
    tx.environments()
        .insert_environment(kernel_store::models::NewResolvedEnvironment {
            environment_id: environment,
            run_id: run,
            agent_spec_id: domain::ids::AgentSpecId::new(ids),
            agent_spec_version: "v1".into(),
            agent_spec_digest: "d".into(),
            agent_loop_id: "loop".into(),
            agent_loop_version: "1".into(),
            agent_loop_digest: "d".into(),
            config_generation_id: domain::ids::ConfigGenerationId::new(ids),
            workspace_uri: None,
            workspace_base_revision: None,
            workspace_mode: WorkspaceAccessMode::ReadOnly,
            model_provider: None,
            model_id: None,
            model_parameters: None,
            kernel_version: "test".into(),
            protocol_versions: b"[]".to_vec(),
            capability_grant_ids: b"[]".to_vec(),
            approval_request_ids: b"[]".to_vec(),
            created_at_ms: NOW,
        })
        .await
        .expect("env");
    tx.environments()
        .insert_bindings(
            environment,
            vec![kernel_store::models::NewResolvedBinding {
                port_id: "effect.execute".to_owned(),
                adapter_id: adapter,
                adapter_version: "1.0.0".to_owned(),
                adapter_digest: "digest".to_owned(),
                capabilities: b"[]".to_vec(),
            }],
        )
        .await
        .expect("binding");
    tx.adapters()
        .insert_registration(kernel_store::models::NewAdapterRegistration {
            adapter_id: adapter,
            version: "1.0.0".to_owned(),
            bundle_digest: "digest".to_owned(),
            manifest_digest: "m".to_owned(),
            runtime_type: "process".to_owned(),
            implemented_ports: br#"[{"port_id":"effect.execute","version":1}]"#.to_vec(),
            capabilities: b"{}".to_vec(),
            trust_state: TrustState::Trusted,
            conformance_state: ConformanceState::Passed,
            created_at_ms: NOW,
        })
        .await
        .expect("registration");
    let row = tx.runs().get(run).await.expect("r").expect("run row");
    let moved = tx
        .runs()
        .cas_update(
            run,
            kernel_store::models::RunCas {
                run_revision: row.run_revision,
                state: None,
                cancellation_epoch: None,
            },
            kernel_store::models::RunPatch {
                resolved_environment_id: Some(environment),
                bump_revision: true,
                ..Default::default()
            },
        )
        .await
        .expect("env patch");
    assert!(moved);
    environment
}

fn decision_for(
    turn: &kernel_store::models::LoopTurnRow,
    decision_id: DecisionId,
    body: loop_decision::Decision,
) -> LoopDecision {
    LoopDecision {
        run_id: turn.run_id.to_string(),
        run_revision: turn.run_revision,
        loop_epoch: turn.loop_epoch,
        step_sequence: turn.step_sequence,
        input_event_cursor: turn.input_event_cursor.to_string(),
        turn_id: turn.turn_id.to_string(),
        decision_id: decision_id.to_string(),
        decision: Some(body),
    }
}

#[tokio::test]
async fn issue_then_accept_advances_run() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 2);
    let mut tx = txn(&store, epoch).await;
    let run = insert_run(&mut *tx, &ids).await;

    // Binding the frozen environment bumps the run revision; bind before
    // the turn is issued so the issued fence already covers it.
    bind_effect_environment(&mut *tx, &ids, run).await;
    let turn = loop_turn::issue_turn(&mut *tx, &ids, run, NOW)
        .await
        .expect("issue");
    assert_eq!(turn.state, turn_state::ISSUED);
    assert_eq!(turn.step_sequence, 1);
    let dec_id = DecisionId::new(&ids);
    let dec = decision_for(
        &turn,
        dec_id,
        loop_decision::Decision::InvokeEffect(InvokeEffect {
            operation: "fixture.increment_counter".into(),
            payload: b"p".to_vec(),
            effect_claim: b"claim".to_vec(),
        }),
    );
    let bytes = dec.encode_to_vec();
    let outcome = decision::accept_decision(&mut *tx, &ids, &SystemClock, run, &dec, &bytes, NOW)
        .await
        .expect("accept");
    let AcceptOutcome::Accepted {
        row: drow,
        instruction,
    } = outcome
    else {
        panic!("expected Accepted");
    };
    assert_eq!(drow.decision_type, "InvokeEffect");
    assert_eq!(
        instruction,
        DecisionInstruction::InvokeEffect {
            operation: "fixture.increment_counter".into(),
            payload: b"p".to_vec(),
            effect_claim: b"claim".to_vec(),
        }
    );
    let turn_row = tx
        .loop_turns()
        .get_turn(turn.turn_id)
        .await
        .expect("t")
        .expect("row");
    assert_eq!(turn_row.state, turn_state::ACCEPTED);
    let row = tx.runs().get(run).await.expect("get").expect("run");
    assert_eq!(row.state, RunState::WaitingTool);
    // The effect was prepared atomically inside the accept transaction.
    let effects = tx.effects().list_by_run(run).await.expect("effects");
    assert_eq!(effects.len(), 1);
    assert_eq!(effects[0].state, domain::effect::EffectState::Prepared);
    assert_eq!(effects[0].operation, "fixture.increment_counter");
}

#[tokio::test]
async fn duplicate_decision_replays_without_side_effects() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 3);
    let mut tx = txn(&store, epoch).await;
    let run = insert_run(&mut *tx, &ids).await;
    let turn = loop_turn::issue_turn(&mut *tx, &ids, run, NOW)
        .await
        .expect("issue");
    let dec_id = DecisionId::new(&ids);
    let dec = decision_for(
        &turn,
        dec_id,
        loop_decision::Decision::Complete(Complete {
            output_ref: "artifact://a".into(),
        }),
    );
    let bytes = dec.encode_to_vec();
    decision::accept_decision(&mut *tx, &ids, &SystemClock, run, &dec, &bytes, NOW)
        .await
        .expect("first accept");
    let rev_after_first = tx
        .runs()
        .get(run)
        .await
        .expect("g")
        .expect("r")
        .run_revision;

    // Replay: same decision_id, would otherwise re-run side effects.
    let outcome = decision::accept_decision(&mut *tx, &ids, &SystemClock, run, &dec, &bytes, NOW)
        .await
        .expect("replay");
    let AcceptOutcome::Replayed { prior } = outcome else {
        panic!("expected Replayed");
    };
    assert_eq!(prior.decision_id, dec_id);
    let row = tx.runs().get(run).await.expect("get").expect("run");
    assert_eq!(row.run_revision, rev_after_first, "replay moved nothing");
    assert_eq!(row.state, RunState::Completed);
}

#[tokio::test]
async fn stale_fences_are_rejected() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 4);
    let mut tx = txn(&store, epoch).await;
    let run = insert_run(&mut *tx, &ids).await;
    let turn = loop_turn::issue_turn(&mut *tx, &ids, run, NOW)
        .await
        .expect("issue");

    for (label, _idx) in [
        ("revision", 0usize),
        ("epoch", 1),
        ("step", 2),
        ("cursor", 3),
    ] {
        // Fresh turn for each rejection (a rejected decision marks the
        // issued turn stale).
        let t = if label == "revision" {
            turn.clone()
        } else {
            loop_turn::issue_turn(&mut *tx, &ids, run, NOW)
                .await
                .expect("issue")
        };
        let mut dec = decision_for(
            &t,
            DecisionId::new(&ids),
            loop_decision::Decision::Complete(Complete {
                output_ref: "artifact://x".into(),
            }),
        );
        match label {
            "revision" => dec.run_revision += 9,
            "epoch" => dec.loop_epoch += 9,
            "step" => dec.step_sequence += 9,
            "cursor" => dec.input_event_cursor = "run/other:9".into(),
            _ => unreachable!(),
        }
        let bytes = dec.encode_to_vec();
        let err = decision::accept_decision(&mut *tx, &ids, &SystemClock, run, &dec, &bytes, NOW)
            .await
            .expect_err(&format!("stale {label}"));
        assert_eq!(err.code(), ErrorCode::Conflict, "{label}");
        let row = tx
            .loop_turns()
            .get_turn(t.turn_id)
            .await
            .expect("t")
            .expect("row");
        assert_eq!(row.state, turn_state::STALE, "{label}");
    }
    // No side effects: run revision only moved by issue_turn calls.
    let row = tx.runs().get(run).await.expect("g").expect("r");
    assert_eq!(row.state, RunState::Running);
    assert!(row.output_ref.is_none());
}

#[tokio::test]
async fn restart_bumps_epoch_and_stales_issued() {
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 5);
    let mut tx = txn(&store, epoch).await;
    let run = insert_run(&mut *tx, &ids).await;
    let t1 = loop_turn::issue_turn(&mut *tx, &ids, run, NOW)
        .await
        .expect("t1");

    // Restart path: epoch bump stales the outstanding issued turn.
    let new_epoch = loop_turn::bump_loop_epoch(&mut *tx, run, 1, &[t1.turn_id])
        .await
        .expect("bump");
    assert_eq!(new_epoch, 1);
    let row = tx
        .loop_turns()
        .get_turn(t1.turn_id)
        .await
        .expect("t")
        .expect("r");
    assert_eq!(row.state, turn_state::STALE);

    // A decision for the old epoch is now stale (turn already stale).
    let mut dec = decision_for(
        &t1,
        DecisionId::new(&ids),
        loop_decision::Decision::Complete(Complete {
            output_ref: "artifact://x".into(),
        }),
    );
    let bytes = dec.encode_to_vec();
    let err = decision::accept_decision(&mut *tx, &ids, &SystemClock, run, &dec, &bytes, NOW)
        .await
        .expect_err("old-epoch decision rejected");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition); // turn no longer issued

    // New epoch turn carries the bumped fence.
    let t2 = loop_turn::issue_turn(&mut *tx, &ids, run, NOW)
        .await
        .expect("t2");
    assert_eq!(t2.loop_epoch, 1);
    assert_eq!(t2.step_sequence, 2);
    dec.turn_id = t2.turn_id.to_string();
    dec.run_revision = t2.run_revision;
    dec.loop_epoch = t2.loop_epoch;
    dec.step_sequence = t2.step_sequence;
    dec.decision_id = DecisionId::new(&ids).to_string();
    let bytes = dec.encode_to_vec();
    decision::accept_decision(&mut *tx, &ids, &SystemClock, run, &dec, &bytes, NOW)
        .await
        .expect("new epoch accepts");
}

#[tokio::test]
async fn issued_turn_survives_simulated_crash() {
    // Crash between issue and accept: the issued row + bumped revision
    // are durable; a second daemon can see and reconcile them.
    let (store, epoch, _d) = open().await;
    let ids = DeterministicIds::new(SEED + 6);
    {
        let mut tx = txn(&store, epoch).await;
        let run = insert_run(&mut *tx, &ids).await;
        let t = loop_turn::issue_turn(&mut *tx, &ids, run, NOW)
            .await
            .expect("issue");
        tx.commit().await.expect("commit");
        // "Crash": no accept happens.
        let mut tx = txn(&store, epoch).await;
        let row = tx
            .loop_turns()
            .get_turn(t.turn_id)
            .await
            .expect("t")
            .expect("r");
        assert_eq!(row.state, turn_state::ISSUED);
        let stale = loop_turn::bump_loop_epoch(&mut *tx, run, 1, &[t.turn_id])
            .await
            .expect("reconcile bumps epoch");
        assert_eq!(stale, 1);
        let row = tx
            .loop_turns()
            .get_turn(t.turn_id)
            .await
            .expect("t")
            .expect("r");
        assert_eq!(row.state, turn_state::STALE);
    }
}
