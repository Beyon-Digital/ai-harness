//! EVT-003 dispatcher integration tests.
//!
//! The dispatcher runs against a real `SqliteKernelStore` in a temp runtime
//! root, a test-local in-memory journal with the idempotent append semantics
//! of the SQLite journal, and a recorder `LiveSink`. Iterations are driven by
//! direct `dispatch_once` calls; there are no sleeps.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use domain::ids::{CommandId, DaemonInstanceId, EventId, EventStreamKey, PrincipalId, RunId};
use domain::provider::SystemIdProvider;
use domain::security::{RetentionClass, SensitivityClass};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::dispatcher::{AFTER_JOURNAL_APPEND, DispatchOutcome, EventDispatcher, LiveSink};
use events::journal::{AppendResult, EventJournalPort, ReadResult};
use events::outbox::DraftEvent;
use events::{EventEnvelope, StreamKey};
use kernel_store::models::OutboxEventRow;
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use tempfile::TempDir;
use testkit::clock::TestClock;
use testkit::faults::ArmedFaults;
use testkit::ids::DeterministicIds;

const NOW_MS: i64 = 1_700_000_000_000;
const EVENT_TYPE: &str = "dispatcher.spec.recorded";
const RUN_A: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70";
const RUN_B: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e71";

fn event_id(byte: u8) -> EventId {
    EventId::from_str(&format!("018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e{byte:02x}"))
        .expect("sample event id is canonical")
}

fn run_stream(seed: &str) -> StreamKey {
    StreamKey::run(RunId::from_str(seed).expect("sample run id is canonical"))
}

fn stream_key(stream: &StreamKey) -> EventStreamKey {
    EventStreamKey::new(stream.as_str()).expect("stream key text is valid")
}

fn tx_context(epoch: u64, principal: PrincipalId) -> TxContext {
    TxContext {
        daemon_epoch: epoch,
        principal_id: principal,
        command_id: CommandId::new(&SystemIdProvider),
        correlation_id: None,
    }
}

async fn open_store(dir: &TempDir) -> (Arc<SqliteKernelStore>, u64) {
    let store = SqliteKernelStore::open(StoreConfig {
        path: dir.path().join("kernel.db"),
        pool_max_connections: 1,
        busy_timeout_ms: 5_000,
    })
    .await
    .expect("store opens");
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&SystemIdProvider))
        .await
        .expect("daemon fence is acquired");
    (Arc::new(store), fence.epoch.0)
}

async fn stage_events(
    store: &SqliteKernelStore,
    epoch: u64,
    principal: PrincipalId,
    stream: &StreamKey,
    events: &[(EventId, Vec<u8>)],
) {
    let mut txn = store
        .begin_write(tx_context(epoch, principal))
        .await
        .expect("write transaction opens");
    for (id, payload) in events {
        events::outbox::stage(
            &mut *txn,
            DraftEvent {
                event_id: *id,
                stream_key: stream_key(stream),
                event_type: EVENT_TYPE.to_owned(),
                payload: payload.clone(),
                sensitivity: SensitivityClass::Internal,
                retention: RetentionClass::Standard,
                correlation_id: None,
                causation_id: None,
            },
        )
        .await
        .expect("outbox stage succeeds");
    }
    txn.commit().await.expect("write transaction commits");
}

async fn unpublished(
    store: &SqliteKernelStore,
    epoch: u64,
    principal: PrincipalId,
) -> Vec<OutboxEventRow> {
    let mut txn = store
        .begin_write(tx_context(epoch, principal))
        .await
        .expect("scan transaction opens");
    let rows = txn
        .streams()
        .scan_unpublished(u32::MAX)
        .await
        .expect("scan succeeds");
    txn.rollback().await.expect("scan transaction rolls back");
    rows
}

fn dispatcher(
    store: &Arc<SqliteKernelStore>,
    journal: Arc<MemoryJournal>,
    sink: Arc<dyn LiveSink>,
    faults: Arc<ArmedFaults>,
    principal: PrincipalId,
) -> EventDispatcher {
    EventDispatcher::new(
        Arc::clone(store) as Arc<dyn KernelStore>,
        journal,
        sink,
        faults,
        Arc::new(TestClock::new(NOW_MS)),
        Arc::new(DeterministicIds::new(NOW_MS)),
        principal,
    )
}

fn assert_unavailable(error: &KernelError) {
    assert_eq!(error.code(), ErrorCode::Unavailable);
    assert_eq!(error.retry_class(), RetryClass::Safe);
}

/// Test-local journal with the SQLite journal's exact append semantics.
#[derive(Default)]
struct MemoryJournal {
    rows: Mutex<HashMap<String, Vec<EventEnvelope>>>,
    failing: AtomicBool,
}

impl MemoryJournal {
    fn new() -> Self {
        Self::default()
    }

    fn set_failing(&self, failing: bool) {
        self.failing.store(failing, Ordering::SeqCst);
    }

    fn rows_for(&self, stream: &StreamKey) -> Vec<EventEnvelope> {
        self.rows
            .lock()
            .expect("journal lock is not poisoned")
            .get(stream.as_str())
            .cloned()
            .unwrap_or_default()
    }

    fn total_rows(&self) -> usize {
        self.rows
            .lock()
            .expect("journal lock is not poisoned")
            .values()
            .map(Vec::len)
            .sum()
    }

    fn contains(&self, event: &EventEnvelope) -> bool {
        self.rows
            .lock()
            .expect("journal lock is not poisoned")
            .get(event.stream_key.as_str())
            .is_some_and(|rows| {
                rows.iter().any(|row| {
                    row.event_id == event.event_id
                        && row.sequence == event.sequence
                        && row.to_bytes() == event.to_bytes()
                })
            })
    }
}

#[allow(clippy::type_complexity)]
impl EventJournalPort for MemoryJournal {
    fn append<'life0, 'life1, 'life2, 'async_trait>(
        &'life0 self,
        stream_key: &'life1 StreamKey,
        expected_sequence: u64,
        batch: &'life2 [EventEnvelope],
    ) -> Pin<Box<dyn Future<Output = errors::Result<AppendResult>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        'life2: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            if self.failing.load(Ordering::SeqCst) {
                return Err(KernelError::new(
                    ErrorCode::Unavailable,
                    RetryClass::Safe,
                    "test journal is unavailable",
                ));
            }
            if batch.is_empty() {
                return Err(KernelError::new(
                    ErrorCode::InvalidArgument,
                    RetryClass::Never,
                    "test journal append batch is empty",
                ));
            }
            let mut next = expected_sequence + 1;
            for event in batch {
                if event.stream_key != *stream_key {
                    return Err(KernelError::new(
                        ErrorCode::InvalidArgument,
                        RetryClass::Never,
                        "test journal batch belongs to a different stream",
                    ));
                }
                if event.sequence != next {
                    return Err(KernelError::new(
                        ErrorCode::InvalidArgument,
                        RetryClass::Never,
                        "test journal batch is not contiguous",
                    ));
                }
                next += 1;
            }
            let final_sequence = next - 1;

            let mut rows = self.rows.lock().expect("journal lock is not poisoned");
            let recorded = rows.entry(stream_key.as_str().to_owned()).or_default();
            let head = recorded.last().map(|row| row.sequence).unwrap_or(0);
            if head == expected_sequence {
                recorded.extend(batch.iter().cloned());
            } else {
                for event in batch {
                    match recorded.iter().find(|row| row.sequence == event.sequence) {
                        None => {
                            return Err(KernelError::new(
                                ErrorCode::FailedPrecondition,
                                RetryClass::Never,
                                "test journal expected sequence does not match the head",
                            ));
                        }
                        Some(existing) => {
                            if existing.event_id != event.event_id
                                || existing.to_bytes() != event.to_bytes()
                            {
                                return Err(KernelError::new(
                                    ErrorCode::Conflict,
                                    RetryClass::Never,
                                    "test journal position holds a different event",
                                ));
                            }
                        }
                    }
                }
            }
            Ok(AppendResult { final_sequence })
        })
    }

    fn read_stream<'life0, 'life1, 'async_trait>(
        &'life0 self,
        stream_key: &'life1 StreamKey,
        from_sequence: u64,
        limit: u32,
    ) -> Pin<Box<dyn Future<Output = errors::Result<ReadResult>> + Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(async move {
            let rows = self.rows.lock().expect("journal lock is not poisoned");
            let events = rows
                .get(stream_key.as_str())
                .map(|stored| {
                    stored
                        .iter()
                        .filter(|row| row.sequence > from_sequence)
                        .take(usize::try_from(limit).unwrap_or(usize::MAX))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            Ok(ReadResult {
                events,
                retention_gap: false,
            })
        })
    }
}

#[derive(Default)]
struct RecorderSink {
    delivered: Mutex<Vec<EventEnvelope>>,
}

impl RecorderSink {
    fn delivered(&self) -> Vec<EventEnvelope> {
        self.delivered
            .lock()
            .expect("sink lock is not poisoned")
            .clone()
    }
}

impl LiveSink for RecorderSink {
    fn publish(&self, event: &EventEnvelope) {
        self.delivered
            .lock()
            .expect("sink lock is not poisoned")
            .push(event.clone());
    }
}

/// Sink that fails if any delivered event is not already durable in the journal.
struct JournalCheckingSink {
    journal: Arc<MemoryJournal>,
    delivered: AtomicUsize,
    missing: AtomicUsize,
}

impl JournalCheckingSink {
    fn new(journal: Arc<MemoryJournal>) -> Self {
        Self {
            journal,
            delivered: AtomicUsize::new(0),
            missing: AtomicUsize::new(0),
        }
    }
}

impl LiveSink for JournalCheckingSink {
    fn publish(&self, event: &EventEnvelope) {
        if !self.journal.contains(event) {
            self.missing.fetch_add(1, Ordering::SeqCst);
        }
        self.delivered.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn happy_path_appends_marks_and_delivers_in_order() {
    let dir = TempDir::new().expect("temp dir");
    let (store, epoch) = open_store(&dir).await;
    let principal = PrincipalId::new(&SystemIdProvider);
    let stream = run_stream(RUN_A);
    let journal = Arc::new(MemoryJournal::new());
    let sink = Arc::new(RecorderSink::default());
    let faults = Arc::new(ArmedFaults::new());
    let dispatcher = dispatcher(
        &store,
        Arc::clone(&journal),
        Arc::clone(&sink) as Arc<dyn LiveSink>,
        Arc::clone(&faults),
        principal,
    );

    stage_events(
        &store,
        epoch,
        principal,
        &stream,
        &[
            (event_id(0x70), vec![1]),
            (event_id(0x71), vec![2]),
            (event_id(0x72), vec![3]),
        ],
    )
    .await;

    let outcome = dispatcher
        .dispatch_once(16, epoch)
        .await
        .expect("dispatch succeeds");

    assert_eq!(
        outcome,
        DispatchOutcome {
            scanned: 3,
            published: 3,
            backlog: 0,
        }
    );

    let rows = journal.rows_for(&stream);
    let sequences: Vec<u64> = rows.iter().map(|row| row.sequence).collect();
    let ids: Vec<EventId> = rows.iter().map(|row| row.event_id).collect();
    assert_eq!(sequences, vec![1, 2, 3], "journal rows keep sequence order");
    assert_eq!(
        ids,
        vec![event_id(0x70), event_id(0x71), event_id(0x72)],
        "journal rows keep staging order"
    );

    let delivered: Vec<EventId> = sink
        .delivered()
        .iter()
        .map(|event| event.event_id)
        .collect();
    assert_eq!(delivered, ids, "sink receives the published order");

    assert!(
        unpublished(&store, epoch, principal).await.is_empty(),
        "successful append is followed by journal publication marks"
    );
    assert!(
        !faults.is_triggered(AFTER_JOURNAL_APPEND),
        "no fault fired on the happy path"
    );
}

#[tokio::test]
async fn crash_after_append_reappends_without_duplicates() {
    let dir = TempDir::new().expect("temp dir");
    let (store, epoch) = open_store(&dir).await;
    let principal = PrincipalId::new(&SystemIdProvider);
    let stream = run_stream(RUN_A);
    let journal = Arc::new(MemoryJournal::new());
    let sink = Arc::new(RecorderSink::default());
    let faults = Arc::new(ArmedFaults::new());
    let dispatcher = dispatcher(
        &store,
        Arc::clone(&journal),
        Arc::clone(&sink) as Arc<dyn LiveSink>,
        Arc::clone(&faults),
        principal,
    );

    stage_events(
        &store,
        epoch,
        principal,
        &stream,
        &[
            (event_id(0x70), vec![1]),
            (event_id(0x71), vec![2]),
            (event_id(0x72), vec![3]),
        ],
    )
    .await;

    faults.arm(AFTER_JOURNAL_APPEND);
    let error = dispatcher
        .dispatch_once(16, epoch)
        .await
        .expect_err("the armed fault aborts the iteration");
    assert_unavailable(&error);
    assert!(faults.is_triggered(AFTER_JOURNAL_APPEND));

    assert_eq!(
        journal.total_rows(),
        3,
        "journal rows are durable before the crash point"
    );
    assert_eq!(
        sink.delivered().len(),
        0,
        "nothing is delivered before publication marks"
    );
    assert_eq!(
        unpublished(&store, epoch, principal).await.len(),
        3,
        "the crash point leaves every row unmarked"
    );

    let outcome = dispatcher
        .dispatch_once(16, epoch)
        .await
        .expect("recovery dispatch succeeds");
    assert_eq!(
        outcome,
        DispatchOutcome {
            scanned: 3,
            published: 3,
            backlog: 0,
        }
    );
    assert_eq!(
        journal.total_rows(),
        3,
        "idempotent re-append adds no duplicate rows"
    );
    assert_eq!(sink.delivered().len(), 3, "recovery publishes every event");
    assert!(
        unpublished(&store, epoch, principal).await.is_empty(),
        "recovery marks every row"
    );
}

#[tokio::test]
async fn unavailable_journal_grows_the_backlog_and_commands_still_commit() {
    let dir = TempDir::new().expect("temp dir");
    let (store, epoch) = open_store(&dir).await;
    let principal = PrincipalId::new(&SystemIdProvider);
    let stream = run_stream(RUN_A);
    let journal = Arc::new(MemoryJournal::new());
    let sink = Arc::new(RecorderSink::default());
    let faults = Arc::new(ArmedFaults::new());
    let dispatcher = dispatcher(
        &store,
        Arc::clone(&journal),
        Arc::clone(&sink) as Arc<dyn LiveSink>,
        Arc::clone(&faults),
        principal,
    );

    stage_events(
        &store,
        epoch,
        principal,
        &stream,
        &[(event_id(0x70), vec![1])],
    )
    .await;
    journal.set_failing(true);

    let error = dispatcher
        .dispatch_once(16, epoch)
        .await
        .expect_err("an unavailable journal surfaces");
    assert_unavailable(&error);
    assert_eq!(
        sink.delivered().len(),
        0,
        "no delivery without journal rows"
    );
    assert_eq!(unpublished(&store, epoch, principal).await.len(), 1);

    stage_events(
        &store,
        epoch,
        principal,
        &stream,
        &[(event_id(0x71), vec![2])],
    )
    .await;
    assert_eq!(
        unpublished(&store, epoch, principal).await.len(),
        2,
        "commands keep committing, so the backlog grows"
    );
    assert_eq!(journal.total_rows(), 0);

    journal.set_failing(false);
    let outcome = dispatcher
        .dispatch_once(16, epoch)
        .await
        .expect("recovery dispatch succeeds");
    assert_eq!(
        outcome,
        DispatchOutcome {
            scanned: 2,
            published: 2,
            backlog: 0,
        }
    );
    assert_eq!(journal.total_rows(), 2, "each event has exactly one row");
    assert_eq!(sink.delivered().len(), 2);
    assert!(unpublished(&store, epoch, principal).await.is_empty());
}

#[tokio::test]
async fn multi_stream_backlog_keeps_per_stream_order() {
    let dir = TempDir::new().expect("temp dir");
    let (store, epoch) = open_store(&dir).await;
    let principal = PrincipalId::new(&SystemIdProvider);
    let stream_a = run_stream(RUN_A);
    let stream_b = run_stream(RUN_B);
    let journal = Arc::new(MemoryJournal::new());
    let sink = Arc::new(RecorderSink::default());
    let faults = Arc::new(ArmedFaults::new());
    let dispatcher = dispatcher(
        &store,
        Arc::clone(&journal),
        Arc::clone(&sink) as Arc<dyn LiveSink>,
        Arc::clone(&faults),
        principal,
    );

    stage_events(
        &store,
        epoch,
        principal,
        &stream_a,
        &[(event_id(0x70), vec![1])],
    )
    .await;
    stage_events(
        &store,
        epoch,
        principal,
        &stream_b,
        &[(event_id(0x71), vec![2])],
    )
    .await;
    stage_events(
        &store,
        epoch,
        principal,
        &stream_a,
        &[(event_id(0x72), vec![3])],
    )
    .await;
    stage_events(
        &store,
        epoch,
        principal,
        &stream_b,
        &[(event_id(0x73), vec![4])],
    )
    .await;

    let outcome = dispatcher
        .dispatch_once(16, epoch)
        .await
        .expect("dispatch succeeds");
    assert_eq!(outcome.scanned, 4);
    assert_eq!(outcome.published, 4);

    let rows_a = journal.rows_for(&stream_a);
    let rows_b = journal.rows_for(&stream_b);
    assert_eq!(
        rows_a.iter().map(|row| row.event_id).collect::<Vec<_>>(),
        vec![event_id(0x70), event_id(0x72)],
        "stream A keeps sequence order"
    );
    assert_eq!(
        rows_b.iter().map(|row| row.event_id).collect::<Vec<_>>(),
        vec![event_id(0x71), event_id(0x73)],
        "stream B keeps sequence order"
    );
    assert_eq!(
        rows_a.iter().map(|row| row.sequence).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        rows_b.iter().map(|row| row.sequence).collect::<Vec<_>>(),
        vec![1, 2]
    );

    let delivered: Vec<EventId> = sink
        .delivered()
        .iter()
        .map(|event| event.event_id)
        .collect();
    assert_eq!(
        delivered,
        vec![
            event_id(0x70),
            event_id(0x72),
            event_id(0x71),
            event_id(0x73)
        ],
        "sink delivery follows the ordered scan"
    );
    assert!(unpublished(&store, epoch, principal).await.is_empty());
}

#[tokio::test]
async fn no_sink_delivery_precedes_journal_acceptance() {
    let dir = TempDir::new().expect("temp dir");
    let (store, epoch) = open_store(&dir).await;
    let principal = PrincipalId::new(&SystemIdProvider);
    let stream = run_stream(RUN_A);
    let journal = Arc::new(MemoryJournal::new());
    let sink = Arc::new(JournalCheckingSink::new(Arc::clone(&journal)));
    let faults = Arc::new(ArmedFaults::new());
    let dispatcher = dispatcher(
        &store,
        Arc::clone(&journal),
        Arc::clone(&sink) as Arc<dyn LiveSink>,
        Arc::clone(&faults),
        principal,
    );

    stage_events(
        &store,
        epoch,
        principal,
        &stream,
        &[(event_id(0x70), vec![1]), (event_id(0x71), vec![2])],
    )
    .await;

    dispatcher
        .dispatch_once(16, epoch)
        .await
        .expect("dispatch succeeds");

    assert_eq!(sink.delivered.load(Ordering::SeqCst), 2);
    assert_eq!(
        sink.missing.load(Ordering::SeqCst),
        0,
        "every delivered event is already durable in the journal"
    );
}

#[tokio::test]
async fn empty_outbox_is_a_no_op() {
    let dir = TempDir::new().expect("temp dir");
    let (store, epoch) = open_store(&dir).await;
    let principal = PrincipalId::new(&SystemIdProvider);
    let journal = Arc::new(MemoryJournal::new());
    let sink = Arc::new(RecorderSink::default());
    let faults = Arc::new(ArmedFaults::new());
    let dispatcher = dispatcher(
        &store,
        Arc::clone(&journal),
        Arc::clone(&sink) as Arc<dyn LiveSink>,
        Arc::clone(&faults),
        principal,
    );

    let outcome = dispatcher
        .dispatch_once(16, epoch)
        .await
        .expect("an empty scan succeeds");
    assert_eq!(
        outcome,
        DispatchOutcome {
            scanned: 0,
            published: 0,
            backlog: 0,
        }
    );
    assert_eq!(journal.total_rows(), 0);
    assert_eq!(sink.delivered().len(), 0);
}

#[tokio::test]
async fn stale_epoch_rejects_the_iteration_before_any_append() {
    let dir = TempDir::new().expect("temp dir");
    let (store, epoch) = open_store(&dir).await;
    let principal = PrincipalId::new(&SystemIdProvider);
    let stream = run_stream(RUN_A);
    let journal = Arc::new(MemoryJournal::new());
    let sink = Arc::new(RecorderSink::default());
    let faults = Arc::new(ArmedFaults::new());
    let dispatcher = dispatcher(
        &store,
        Arc::clone(&journal),
        Arc::clone(&sink) as Arc<dyn LiveSink>,
        Arc::clone(&faults),
        principal,
    );

    stage_events(
        &store,
        epoch,
        principal,
        &stream,
        &[(event_id(0x70), vec![1])],
    )
    .await;

    let error = dispatcher
        .dispatch_once(16, epoch + 1)
        .await
        .expect_err("a stale epoch is rejected");
    assert_eq!(error.code(), ErrorCode::FailedPrecondition);
    assert_eq!(journal.total_rows(), 0);
    assert_eq!(sink.delivered().len(), 0);
    assert_eq!(unpublished(&store, epoch, principal).await.len(), 1);
}
