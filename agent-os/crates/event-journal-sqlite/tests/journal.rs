//! Integration tests for the SQLite event journal (EVT-002).
//!
//! These tests pin the append rules from the task brief: expected-sequence
//! enforcement, idempotent exact duplicates, conflicts on one position, gap
//! rejection, ordered reads with a false retention flag, private file modes,
//! and a single winner under concurrent same-position appends.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use domain::ids::RunId;
use domain::provider::SystemIdProvider;
use domain::security::{RetentionClass, SensitivityClass};
use errors::codes::{ErrorCode, RetryClass};
use event_journal::{AppendResult, EventJournalPort};
use event_journal_sqlite::{JournalConfig, SqliteEventJournal};
use events::{CatalogClassificationPolicy, EventBuilder, EventEnvelope, StreamKey};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Connection, SqliteConnection, SqlitePool};
use tempfile::TempDir;
use tokio::sync::Barrier;

const BUSY_TIMEOUT_MS: u64 = 5_000;
const EVENT_TYPE: &str = "journal.spec.recorded";

fn policy() -> CatalogClassificationPolicy {
    CatalogClassificationPolicy::embedded().expect("embedded catalog parses")
}

fn run_key() -> StreamKey {
    StreamKey::run(RunId::new(&SystemIdProvider))
}

fn envelope(stream: &StreamKey, sequence: u64, marker: u8) -> EventEnvelope {
    EventBuilder::new(EVENT_TYPE, 1, stream.clone())
        .sequence(sequence)
        .occurred_at_ms(1_700_000_000_000 + sequence as i64)
        .sensitivity(SensitivityClass::Internal)
        .retention(RetentionClass::Standard)
        .payload(vec![marker, sequence as u8])
        .build(&policy())
        .expect("envelope builds")
}

fn batch(stream: &StreamKey, sequences: std::ops::RangeInclusive<u64>) -> Vec<EventEnvelope> {
    sequences
        .map(|sequence| envelope(stream, sequence, 0x40 | sequence as u8))
        .collect()
}

fn journal_config(path: &Path) -> JournalConfig {
    JournalConfig {
        path: path.to_path_buf(),
        busy_timeout_ms: BUSY_TIMEOUT_MS,
    }
}

async fn open_journal() -> (TempDir, PathBuf, Arc<SqliteEventJournal>) {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("events.db");
    let journal = SqliteEventJournal::open(journal_config(&path))
        .await
        .expect("journal opens");
    (dir, path, Arc::new(journal))
}

fn spawn_append(
    journal: Arc<SqliteEventJournal>,
    barrier: Arc<Barrier>,
    key: StreamKey,
    envelope: EventEnvelope,
) -> tokio::task::JoinHandle<errors::Result<AppendResult>> {
    tokio::spawn(async move {
        barrier.wait().await;
        journal.append(&key, 0, &[envelope]).await
    })
}

#[tokio::test]
async fn append_then_read_returns_ordered_events() {
    let (_dir, _path, journal) = open_journal().await;
    let key = run_key();
    let events = batch(&key, 1..=3);

    let appended = journal
        .append(&key, 0, &events)
        .await
        .expect("append succeeds");
    assert_eq!(appended.final_sequence, 3);

    let read = journal
        .read_stream(&key, 0, 10)
        .await
        .expect("read succeeds");
    assert!(!read.retention_gap);
    assert_eq!(read.events, events);

    let page = journal
        .read_stream(&key, 1, 1)
        .await
        .expect("paged read succeeds");
    assert_eq!(page.events, vec![events[1].clone()]);

    let from_head = journal
        .read_stream(&key, 3, 10)
        .await
        .expect("read at head succeeds");
    assert!(from_head.events.is_empty());
    assert!(!from_head.retention_gap);

    let past_head = journal
        .read_stream(&key, 99, 10)
        .await
        .expect("read past head succeeds");
    assert!(past_head.events.is_empty());
    assert!(!past_head.retention_gap);

    let zero_limit = journal
        .read_stream(&key, 0, 0)
        .await
        .expect("zero limit read succeeds");
    assert!(zero_limit.events.is_empty());
}

#[tokio::test]
async fn exact_duplicate_append_is_idempotent() {
    let (_dir, _path, journal) = open_journal().await;
    let key = run_key();
    let events = batch(&key, 1..=3);

    journal
        .append(&key, 0, &events)
        .await
        .expect("first append");
    let duplicate = journal
        .append(&key, 0, &events)
        .await
        .expect("exact duplicate is idempotent");
    assert_eq!(duplicate.final_sequence, 3);

    let fourth = envelope(&key, 4, 0x44);
    journal
        .append(&key, 3, std::slice::from_ref(&fourth))
        .await
        .expect("later append");

    let retried = journal
        .append(&key, 0, &events)
        .await
        .expect("old batch stays idempotent after further appends");
    assert_eq!(retried.final_sequence, 3);

    let read = journal
        .read_stream(&key, 0, 10)
        .await
        .expect("read succeeds");
    assert_eq!(read.events.len(), 4);
    assert_eq!(read.events[3], fourth);
}

#[tokio::test]
async fn duplicate_position_with_a_different_event_is_a_conflict() {
    let (_dir, _path, journal) = open_journal().await;
    let key = run_key();
    let first = envelope(&key, 1, 0x01);
    journal
        .append(&key, 0, std::slice::from_ref(&first))
        .await
        .expect("first append");

    let intruder = envelope(&key, 1, 0x02);
    let error = journal
        .append(&key, 0, &[intruder])
        .await
        .expect_err("a different event id at the same position conflicts");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let read = journal
        .read_stream(&key, 0, 10)
        .await
        .expect("read succeeds");
    assert_eq!(read.events, vec![first.clone()]);

    let second = envelope(&key, 2, 0x03);
    journal
        .append(&key, 1, std::slice::from_ref(&second))
        .await
        .expect("second append");

    let conflicting_tail = envelope(&key, 2, 0x04);
    let error = journal
        .append(&key, 0, &[first.clone(), conflicting_tail])
        .await
        .expect_err("a batch that matches a prefix and conflicts later is rejected whole");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let read = journal
        .read_stream(&key, 0, 10)
        .await
        .expect("read succeeds");
    assert_eq!(read.events, vec![first, second]);
}

#[tokio::test]
async fn append_rejects_a_batch_that_would_leave_a_gap() {
    let (_dir, _path, journal) = open_journal().await;
    let key = run_key();

    // The caller believes position 1 is already the head, but the stream is
    // empty; writing position 2 would strand position 1.
    let second = envelope(&key, 2, 0x02);
    let error = journal
        .append(&key, 1, std::slice::from_ref(&second))
        .await
        .expect_err("a batch that would leave a gap is rejected");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(error.retry_class(), RetryClass::Never);
    let read = journal
        .read_stream(&key, 0, 10)
        .await
        .expect("read succeeds");
    assert!(read.events.is_empty());

    // With position 1 recorded, expectations track the head again.
    let first = envelope(&key, 1, 0x01);
    journal
        .append(&key, 0, std::slice::from_ref(&first))
        .await
        .expect("first append");
    journal
        .append(&key, 1, std::slice::from_ref(&second))
        .await
        .expect("second append");
    let read = journal
        .read_stream(&key, 0, 10)
        .await
        .expect("read succeeds");
    assert_eq!(read.events, vec![first, second]);
}

#[tokio::test]
async fn non_contiguous_batches_are_rejected_before_writing() {
    let (_dir, _path, journal) = open_journal().await;
    let key = run_key();

    let one = envelope(&key, 1, 0x01);
    let three = envelope(&key, 3, 0x03);
    let error = journal
        .append(&key, 0, &[one, three])
        .await
        .expect_err("a hole inside the batch is rejected");
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let error = journal
        .append(&key, 0, &[envelope(&key, 2, 0x02)])
        .await
        .expect_err("a batch starting past expected_sequence + 1 is rejected");
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let read = journal
        .read_stream(&key, 0, 10)
        .await
        .expect("read succeeds");
    assert!(read.events.is_empty());
}

#[tokio::test]
async fn batch_envelopes_must_belong_to_the_named_stream() {
    let (_dir, _path, journal) = open_journal().await;
    let key = run_key();
    let other = run_key();

    let error = journal
        .append(&key, 0, &[envelope(&other, 1, 0x01)])
        .await
        .expect_err("an envelope from another stream is rejected");
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let read = journal
        .read_stream(&key, 0, 10)
        .await
        .expect("read succeeds");
    assert!(read.events.is_empty());
    let read = journal
        .read_stream(&other, 0, 10)
        .await
        .expect("read succeeds");
    assert!(read.events.is_empty());
}

#[tokio::test]
async fn empty_batches_are_rejected() {
    let (_dir, _path, journal) = open_journal().await;
    let key = run_key();

    let error = journal
        .append(&key, 0, &[])
        .await
        .expect_err("an empty batch is rejected");
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_same_position_appends_have_exactly_one_winner() {
    let (_dir, _path, journal) = open_journal().await;
    let key = run_key();
    let barrier = Arc::new(Barrier::new(2));

    let left = spawn_append(
        Arc::clone(&journal),
        Arc::clone(&barrier),
        key.clone(),
        envelope(&key, 1, 0x0a),
    );
    let right = spawn_append(
        Arc::clone(&journal),
        Arc::clone(&barrier),
        key.clone(),
        envelope(&key, 1, 0x0b),
    );
    let outcomes = [
        left.await.expect("left task joins"),
        right.await.expect("right task joins"),
    ];

    let winners: Vec<&AppendResult> = outcomes
        .iter()
        .filter_map(|outcome| outcome.as_ref().ok())
        .collect();
    assert_eq!(winners.len(), 1, "exactly one append may win the position");
    assert_eq!(winners[0].final_sequence, 1);

    let error = outcomes
        .iter()
        .find_map(|outcome| outcome.as_ref().err())
        .expect("the other append loses");
    assert_eq!(error.code(), ErrorCode::Conflict);
    assert_eq!(error.retry_class(), RetryClass::Never);

    let read = journal
        .read_stream(&key, 0, 10)
        .await
        .expect("read succeeds");
    assert_eq!(read.events.len(), 1);
    assert_eq!(read.events[0].sequence, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn append_waits_out_a_held_writer_lock() {
    let (_dir, path, journal) = open_journal().await;
    let key = run_key();

    let mut blocker = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(&path)
            .busy_timeout(Duration::from_millis(0)),
    )
    .await
    .expect("blocker connects");
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut blocker)
        .await
        .expect("blocker takes the writer lock");

    let handle = tokio::spawn({
        let journal = Arc::clone(&journal);
        let key = key.clone();
        let pending = envelope(&key, 1, 0x01);
        async move { journal.append(&key, 0, &[pending]).await }
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !handle.is_finished(),
        "append must wait on the writer lock instead of failing fast"
    );

    sqlx::query("COMMIT")
        .execute(&mut blocker)
        .await
        .expect("blocker releases the writer lock");
    let result = handle.await.expect("append task joins");
    assert!(result.is_ok(), "append succeeds after the lock clears");
}

#[tokio::test]
async fn journal_file_is_private_and_does_not_share_kernel_tables() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("runtime").join("events.db");
    let journal = SqliteEventJournal::open(journal_config(&path))
        .await
        .expect("journal opens");
    let key = run_key();
    journal
        .append(&key, 0, &[envelope(&key, 1, 0x01)])
        .await
        .expect("append succeeds");

    let runtime = path.parent().expect("runtime dir");
    let dir_mode = fs::metadata(runtime)
        .expect("runtime dir exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);
    let file_mode = fs::metadata(&path)
        .expect("journal file exists")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(file_mode, 0o600);

    let pool = SqlitePool::connect_with(
        SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false),
    )
    .await
    .expect("inspection connection");
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(&pool)
    .await
    .expect("table inventory");
    assert_eq!(tables, vec!["events".to_owned()]);
    pool.close().await;
}

#[tokio::test]
async fn errors_never_render_payload_bytes() {
    const CANARY: &str = "canary-payload-must-not-leak";
    let (_dir, _path, journal) = open_journal().await;
    let key = run_key();
    journal
        .append(&key, 0, &[envelope(&key, 1, 0x01)])
        .await
        .expect("first append");

    let intruder = EventBuilder::new(EVENT_TYPE, 1, key.clone())
        .sequence(1)
        .occurred_at_ms(1_700_000_000_001)
        .sensitivity(SensitivityClass::Internal)
        .retention(RetentionClass::Standard)
        .payload(CANARY.as_bytes().to_vec())
        .build(&policy())
        .expect("intruder builds");
    let error = journal
        .append(&key, 0, &[intruder])
        .await
        .expect_err("conflict");
    assert!(
        !error.to_string().contains(CANARY),
        "display leaked payload bytes"
    );
    assert!(
        !format!("{error:?}").contains(CANARY),
        "debug leaked payload bytes"
    );

    let stale = envelope(&key, 2, 0x02);
    let error = journal
        .append(&key, 0, &[stale])
        .await
        .expect_err("stale expectation");
    assert!(
        !error.to_string().contains(CANARY),
        "display leaked payload bytes"
    );
}
