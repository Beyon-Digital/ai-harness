//! API-003 event service coverage: durable `ReadStream`, replay→live
//! `Subscribe` handoff, and lag termination.

mod common;

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use control_api::event_service::generated::mvp_event_api_client::MvpEventApiClient;
use control_api::event_service::generated::mvp_event_api_server::MvpEventApi;
use control_api::{ControlSocket, EventApiService, UidPrincipalMap, serve};
use domain::generated::contract::{ReadEventStreamRequest, SubscribeEventsRequest};
use events::journal::EventJournalPort;
use testkit::ids::DeterministicIds;

fn envelope(
    ids: &DeterministicIds,
    stream: &events::stream::StreamKey,
    seq: u64,
    sensitivity: domain::security::SensitivityClass,
) -> events::envelope::EventEnvelope {
    events::envelope::EventEnvelope {
        event_id: domain::ids::EventId::new(ids),
        event_type: "agentos.test.Occurred".into(),
        event_version: 1,
        stream_key: stream.clone(),
        sequence: seq,
        occurred_unix_ms: NOW,
        run_id: None,
        task_id: None,
        session_id: None,
        effect_id: None,
        causation_id: None,
        correlation_id: None,
        sensitivity,
        retention: domain::security::RetentionClass::Standard,
        payload: b"payload".to_vec(),
    }
}

async fn event_client(path: &std::path::Path) -> MvpEventApiClient<tonic::transport::Channel> {
    let path = path.to_path_buf();
    let channel = tonic::transport::Endpoint::from_static("http://[::1]:0")
        .connect_with_connector(tower::service_fn(move |_| {
            let path = path.clone();
            async move {
                tokio::net::UnixStream::connect(path)
                    .await
                    .map(hyper_util::rt::TokioIo::new)
                    .map_err(std::io::Error::other)
            }
        }))
        .await
        .expect("connect");
    MvpEventApiClient::new(channel)
}

struct EventRig {
    journal: Arc<event_journal_sqlite::SqliteEventJournal>,
    bus: Arc<events::live_bus::LiveBus>,
    rig: Rig,
}

impl EventRig {
    async fn new() -> Self {
        let rig = Rig::new().await;
        let journal = Arc::new(
            event_journal_sqlite::SqliteEventJournal::open(event_journal_sqlite::JournalConfig {
                path: rig.runtime_dir.join("events.db"),
                busy_timeout_ms: 5_000,
            })
            .await
            .expect("journal"),
        );
        Self {
            journal,
            bus: Arc::new(events::live_bus::LiveBus::new(64)),
            rig,
        }
    }

    fn event_service(&self) -> EventApiService {
        EventApiService::new(
            self.journal.clone(),
            self.bus.clone(),
            Arc::new(UidPrincipalMap::new([(self.rig.uid, self.rig.principal)]).expect("map")),
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_stream_reads_journal_history() {
    let rig = EventRig::new().await;
    let stream = events::stream::StreamKey::from_str("run/x").unwrap();
    let ids = DeterministicIds::new(SEED + 9);
    let batch = vec![
        envelope(
            &ids,
            &stream,
            1,
            domain::security::SensitivityClass::Internal,
        ),
        envelope(&ids, &stream, 2, domain::security::SensitivityClass::Secret),
    ];
    rig.journal
        .append(&stream, 0, &batch)
        .await
        .expect("append");

    let socket = ControlSocket::bind(&rig.rig.runtime_dir.join("svc"), &FakeLock).expect("bind");
    let path = socket.path().to_path_buf();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let service = rig.rig.service();
    let events = rig.event_service();
    let server = tokio::spawn(serve(socket, service, Some(events), async move {
        let _ = stop_rx.await;
    }));
    wait_ready(&path).await;
    let mut c = event_client(&path).await;

    let page = c
        .read_stream(ReadEventStreamRequest {
            stream_key: "run/x".into(),
            from_sequence: 0,
            limit: 10,
        })
        .await
        .expect("read")
        .into_inner();
    assert_eq!(page.events.len(), 2);
    assert_eq!(page.events[0].sequence, 1);
    // Secret payload never leaves the journal.
    assert!(page.events[1].payload.is_empty());
    assert!(!page.retention_gap);
    stop_tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribe_replays_then_lives_without_gap() {
    let rig = EventRig::new().await;
    let stream = events::stream::StreamKey::from_str("run/y").unwrap();
    let ids = DeterministicIds::new(SEED + 10);
    rig.journal
        .append(
            &stream,
            0,
            &[envelope(
                &ids,
                &stream,
                1,
                domain::security::SensitivityClass::Public,
            )],
        )
        .await
        .expect("append 1");

    let socket = ControlSocket::bind(&rig.rig.runtime_dir.join("svc"), &FakeLock).expect("bind");
    let path = socket.path().to_path_buf();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let events = rig.event_service();
    let server = tokio::spawn(serve(socket, rig.rig.service(), Some(events), async move {
        let _ = stop_rx.await;
    }));
    wait_ready(&path).await;
    let mut c = event_client(&path).await;

    // Subscribe from sequence 0 — replay starts with the durable event.
    let mut stream_resp = c
        .subscribe(SubscribeEventsRequest {
            stream_key: "run/y".into(),
            after_sequence: 0,
        })
        .await
        .expect("subscribe")
        .into_inner();

    // While replay runs, append + publish a live event.
    let live = envelope(&ids, &stream, 2, domain::security::SensitivityClass::Public);
    rig.journal
        .append(&stream, 1, std::slice::from_ref(&live))
        .await
        .expect("append 2");
    rig.bus.publish(&live);

    // Expect seq 1 then seq 2, exactly once each, no gap.
    let mut got = Vec::new();
    for _ in 0..2 {
        let frame = tokio::time::timeout(Duration::from_secs(5), stream_resp.message())
            .await
            .expect("frame timeout")
            .expect("stream open")
            .expect("frame ok");
        use domain::generated::contract::event_stream_frame::Frame;
        match frame.frame {
            Some(Frame::Event(e)) => got.push(e.sequence),
            other => panic!("unexpected frame {other:?}"),
        }
    }
    assert_eq!(got, vec![1, 2], "replay + live, ordered, no dupes");
    drop(stream_resp);
    stop_tx.send(()).unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribe_lag_terminates_with_resume_cursor() {
    // Deterministic lag: a replay backlog larger than the reply channel
    // buffer (256) parks the forwarder mid-replay until the client drains;
    // a live burst published during that window overflows the cap-1 bus,
    // so the first `recv` reports `Lagged`. Service-level (no transport).
    let rig = EventRig::new().await;
    let ids = DeterministicIds::new(SEED + 11);
    let stream = events::stream::StreamKey::from_str("run/z").unwrap();

    const BACKLOG: u64 = 300;
    let mut start = 0u64;
    while start < BACKLOG {
        let end = (start + 50).min(BACKLOG);
        let batch: Vec<_> = (start + 1..=end)
            .map(|seq| {
                envelope(
                    &ids,
                    &stream,
                    seq,
                    domain::security::SensitivityClass::Public,
                )
            })
            .collect();
        rig.journal
            .append(&stream, start, &batch)
            .await
            .expect("append");
        start = end;
    }

    let bus = Arc::new(events::live_bus::LiveBus::new(1));
    let svc = EventApiService::new(
        rig.journal.clone(),
        bus.clone(),
        Arc::new(UidPrincipalMap::new([(rig.rig.uid, rig.rig.principal)]).expect("map")),
    );
    let mut req = tonic::Request::new(SubscribeEventsRequest {
        stream_key: "run/z".into(),
        after_sequence: 0,
    });
    req.extensions_mut().insert(control_api::PeerCreds {
        uid: rig.rig.uid,
        pid: None,
    });
    let mut frames = svc.subscribe(req).await.expect("subscribe").into_inner();

    // Burst lands while the 300-event replay is still buffered.
    for seq in BACKLOG + 1..=BACKLOG + 3 {
        let ev = envelope(
            &ids,
            &stream,
            seq,
            domain::security::SensitivityClass::Public,
        );
        rig.journal
            .append(&stream, seq - 1, std::slice::from_ref(&ev))
            .await
            .expect("append");
        bus.publish(&ev);
    }

    // Drain until the terminal LagNotice; all frames must be ordered.
    use domain::generated::contract::event_stream_frame::Frame;
    use tokio_stream::StreamExt;
    let mut last_seq = 0u64;
    let lag = loop {
        match tokio::time::timeout(Duration::from_secs(10), frames.next())
            .await
            .expect("frame")
        {
            Some(Ok(f)) => match f.frame {
                Some(Frame::Event(e)) => {
                    assert!(e.sequence > last_seq, "ordered replay");
                    last_seq = e.sequence;
                }
                Some(Frame::Lag(l)) => break l,
                None => panic!("empty frame"),
            },
            other => panic!("stream ended early: {other:?}"),
        }
    };
    assert_eq!(lag.stream_key, "run/z");
    assert_eq!(
        lag.resume_sequence, last_seq,
        "resume cursor is the last delivered sequence"
    );
    assert!(last_seq >= BACKLOG, "all durable events replayed");
}
