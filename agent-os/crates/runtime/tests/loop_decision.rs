//! LOOP-002: durable turn issue, fenced decision acceptance, idempotent
//! replay, stale rejection, loop_epoch restart bump.

use domain::generated::contract::{Complete, InvokeEffect, LoopDecision, loop_decision};
use domain::ids::{ActorId, CommandId, DaemonInstanceId, DecisionId, PrincipalId, RunId, TaskId};
use domain::run::RunState;
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
            created_at_ms: NOW,
        })
        .await
        .expect("run");
    run
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

    let turn = loop_turn::issue_turn(&mut *tx, &ids, run, NOW)
        .await
        .expect("issue");
    assert_eq!(turn.state, turn_state::ISSUED);
    assert_eq!(turn.step_sequence, 1);
    assert_eq!(turn.run_revision, 1);
    let row = tx.runs().get(run).await.expect("get").expect("run");
    assert_eq!(row.step_sequence, 1);
    assert_eq!(row.run_revision, 1);

    let dec_id = DecisionId::new(&ids);
    let dec = decision_for(
        &turn,
        dec_id,
        loop_decision::Decision::InvokeEffect(InvokeEffect {
            operation: "fs.write".into(),
            payload: b"p".to_vec(),
            effect_claim: b"claim".to_vec(),
        }),
    );
    let bytes = dec.encode_to_vec();
    let outcome = decision::accept_decision(&mut *tx, run, &dec, &bytes, NOW)
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
            operation: "fs.write".into(),
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
    assert_eq!(row.state, RunState::Running);
    assert_eq!(row.run_revision, 2);
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
    decision::accept_decision(&mut *tx, run, &dec, &bytes, NOW)
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
    let outcome = decision::accept_decision(&mut *tx, run, &dec, &bytes, NOW)
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
        let err = decision::accept_decision(&mut *tx, run, &dec, &bytes, NOW)
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
    let err = decision::accept_decision(&mut *tx, run, &dec, &bytes, NOW)
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
    decision::accept_decision(&mut *tx, run, &dec, &bytes, NOW)
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
