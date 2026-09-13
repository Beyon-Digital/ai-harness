//! EVT-004 live bus integration tests.
//!
//! The bus is exercised through deterministic capacity pressure: a slow
//! subscriber is driven past the broadcast capacity with a healthy subscriber
//! consuming in step, so lag is capacity-driven rather than time-driven. The
//! resume test replays the missed range through a real `SqliteEventJournal`
//! in a temp runtime root. There are no sleeps.

use std::str::FromStr;
use std::time::Duration;

use domain::ids::{EventId, RunId};
use domain::security::{RetentionClass, SensitivityClass};
use event_journal_sqlite::{JournalConfig, SqliteEventJournal};
use events::StreamKey;
use events::dispatcher::LiveSink;
use events::envelope::{CatalogClassificationPolicy, EventBuilder, EventEnvelope};
use events::journal::EventJournalPort;
use events::live_bus::{EphemeralBus, LiveBus, LiveItem, LiveSubscription};
use tempfile::TempDir;

const NOW_MS: i64 = 1_700_000_000_000;
const EVENT_TYPE: &str = "evt004.spec.recorded";
const RUN: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70";

fn policy() -> CatalogClassificationPolicy {
    CatalogClassificationPolicy::embedded().expect("embedded catalog parses")
}

fn event_id(byte: u8) -> EventId {
    EventId::from_str(&format!("018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e{byte:02x}"))
        .expect("sample event id is canonical")
}

fn run_stream() -> StreamKey {
    StreamKey::run(RunId::from_str(RUN).expect("sample run id is canonical"))
}

fn envelope(sequence: u64) -> EventEnvelope {
    EventBuilder::new(EVENT_TYPE, 1, run_stream())
        .event_id(event_id(sequence as u8))
        .sequence(sequence)
        .occurred_at_ms(NOW_MS + sequence as i64)
        .sensitivity(SensitivityClass::Internal)
        .retention(RetentionClass::Standard)
        .payload(vec![sequence as u8])
        .build(&policy())
        .expect("sample envelope builds")
}

fn envelopes(sequences: impl IntoIterator<Item = u64>) -> Vec<EventEnvelope> {
    sequences.into_iter().map(envelope).collect()
}

async fn receive(subscription: &mut LiveSubscription) -> LiveItem {
    tokio::time::timeout(Duration::from_secs(10), subscription.next())
        .await
        .expect("a live item is delivered within the bounded wait")
}

fn expect_event(item: LiveItem) -> EventEnvelope {
    match item {
        LiveItem::Event(event) => event,
        LiveItem::Lagged { resume_from } => {
            panic!("expected a delivered event, got lag at {resume_from:?}")
        }
        LiveItem::BusClosed => panic!("expected a delivered event, got a closed bus"),
    }
}

#[tokio::test]
async fn subscribers_receive_published_events_in_order() {
    let bus = LiveBus::new(4);
    let mut subscription = bus.subscribe();
    assert_eq!(bus.subscriber_count(), 1);

    let events = envelopes(1..=3);
    for event in &events {
        bus.publish(event);
    }

    assert_eq!(expect_event(receive(&mut subscription).await), events[0]);
    assert_eq!(expect_event(receive(&mut subscription).await), events[1]);
    assert_eq!(expect_event(receive(&mut subscription).await), events[2]);

    drop(subscription);
    assert_eq!(bus.subscriber_count(), 0);
}

#[tokio::test]
async fn live_bus_implements_the_dispatcher_sink_seam() {
    let bus = LiveBus::new(2);
    let sink: &dyn LiveSink = &bus;
    let mut subscription = bus.subscribe();

    let event = envelope(1);
    sink.publish(&event);

    assert_eq!(expect_event(receive(&mut subscription).await), event);
}

#[tokio::test]
async fn slow_subscriber_reports_lag_with_its_last_delivered_cursor() {
    let bus = LiveBus::new(2);
    let mut slow = bus.subscribe();
    let mut healthy = bus.subscribe();

    let events = envelopes(1..=5);
    bus.publish(&events[0]);
    assert_eq!(expect_event(receive(&mut slow).await), events[0]);
    assert_eq!(expect_event(receive(&mut healthy).await), events[0]);

    // The healthy subscriber consumes in step; the slow one falls behind the
    // two-slot buffer without any wait.
    bus.publish(&events[1]);
    assert_eq!(expect_event(receive(&mut healthy).await), events[1]);
    bus.publish(&events[2]);
    assert_eq!(expect_event(receive(&mut healthy).await), events[2]);
    bus.publish(&events[3]);

    let lagged = receive(&mut slow).await;
    assert_eq!(
        lagged,
        LiveItem::Lagged {
            resume_from: Some(events[0].cursor())
        }
    );

    // The healthy subscriber keeps receiving after its peer lagged.
    assert_eq!(expect_event(receive(&mut healthy).await), events[3]);
    bus.publish(&events[4]);
    assert_eq!(expect_event(receive(&mut healthy).await), events[4]);

    let handle = tokio::spawn(async move { slow.next().await });
    let error = handle.await.expect_err("a lagged subscription is terminal");
    assert!(error.is_panic());
}

#[tokio::test]
async fn cold_start_lag_reports_no_resume_cursor() {
    let bus = LiveBus::new(1);
    let mut subscription = bus.subscribe();
    bus.publish(&envelope(1));
    bus.publish(&envelope(2));

    assert_eq!(
        receive(&mut subscription).await,
        LiveItem::Lagged { resume_from: None },
        "an undelivered subscription resumes from stream inception"
    );

    let handle = tokio::spawn(async move { subscription.next().await });
    let error = handle.await.expect_err("a lagged subscription is terminal");
    assert!(error.is_panic());
}

#[tokio::test]
async fn dropping_the_bus_closes_live_subscriptions() {
    let bus = LiveBus::new(2);
    let mut subscription = bus.subscribe();
    drop(bus);

    assert_eq!(receive(&mut subscription).await, LiveItem::BusClosed);

    let handle = tokio::spawn(async move { subscription.next().await });
    let error = handle.await.expect_err("a closed subscription is terminal");
    assert!(error.is_panic());
}

#[tokio::test]
async fn publishing_without_subscribers_is_a_no_op() {
    let bus = LiveBus::new(2);
    bus.publish(&envelope(1));

    let mut subscription = bus.subscribe();
    let event = envelope(2);
    bus.publish(&event);

    assert_eq!(expect_event(receive(&mut subscription).await), event);
}

#[tokio::test]
async fn resume_from_lag_reads_exactly_the_missed_journal_events() {
    let dir = TempDir::new().expect("temp runtime root");
    let journal = SqliteEventJournal::open(JournalConfig {
        path: dir.path().join("events.db"),
        busy_timeout_ms: 5_000,
    })
    .await
    .expect("journal opens");

    let stream = run_stream();
    let events = envelopes(1..=5);
    journal
        .append(&stream, 0, &events)
        .await
        .expect("journal appends the contiguous batch");

    let bus = LiveBus::new(1);
    let mut subscription = bus.subscribe();
    bus.publish(&events[0]);
    assert_eq!(expect_event(receive(&mut subscription).await), events[0]);

    for event in &events[1..] {
        bus.publish(event);
    }

    let LiveItem::Lagged {
        resume_from: Some(resume_from),
    } = receive(&mut subscription).await
    else {
        panic!("expected the slow subscription to lag with a delivered cursor");
    };
    assert_eq!(resume_from, events[0].cursor());

    let resumed = journal
        .read_stream(&stream, resume_from.sequence, 64)
        .await
        .expect("resume read succeeds");
    assert!(!resumed.retention_gap, "the inception journal has no gaps");
    assert_eq!(resumed.events, events[1..].to_vec());
    assert_eq!(
        resumed
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![2, 3, 4, 5]
    );
    assert!(
        resumed
            .events
            .iter()
            .all(|event| event.sequence > resume_from.sequence)
    );
    assert!(
        resumed
            .events
            .windows(2)
            .all(|pair| pair[0].event_id != pair[1].event_id),
        "resumed range contains no duplicate events"
    );
}

#[tokio::test]
async fn ephemeral_overflow_counts_drops_and_spares_the_durable_bus() {
    let ephemeral = EphemeralBus::new(2);
    assert!(ephemeral.try_publish(vec![0x01]));
    assert!(ephemeral.try_publish(vec![0x02]));
    assert_eq!(ephemeral.dropped(), 0);

    assert!(!ephemeral.try_publish(vec![0x03]));
    assert!(!ephemeral.try_publish(vec![0x04]));
    assert_eq!(ephemeral.dropped(), 2);

    let durable = LiveBus::new(2);
    let mut subscription = durable.subscribe();
    let event = envelope(1);
    durable.publish(&event);
    assert!(!ephemeral.try_publish(vec![0x05]));
    assert_eq!(ephemeral.dropped(), 3);

    assert_eq!(expect_event(receive(&mut subscription).await), event);
}
