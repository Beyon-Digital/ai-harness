//! `MvpEventApi` (API-003): durable `ReadStream` + gap-free `Subscribe`.
//!
//! Subscribe attaches the live-bus subscription *before* replaying the
//! journal so events published mid-replay are buffered, then dedupes by
//! sequence at the handoff — the client never sees a gap or a duplicate
//! durable event. Lag terminates the stream with a `LagNotice` carrying
//! the last delivered sequence to resume from.
//!
//! Sensitivity projection: `Secret` envelopes ship with `payload`
//! stripped — the field is protected by event design, not caller honesty.
#![forbid(unsafe_code)]

use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;

use domain::generated::contract::{
    EventEnvelope, EventStreamFrame, LagNotice, ReadEventStreamRequest, ReadEventStreamResponse,
    SubscribeEventsRequest, event_stream_frame,
};
use domain::security::SensitivityClass;
use errors::codes::ErrorCode;
use events::journal::EventJournalPort;
use events::live_bus::{LiveBus, LiveItem};
use events::stream::StreamKey;
use tokio::sync::mpsc;
use tokio_stream::Stream;
use tonic::{Code, Request, Response, Status};

use crate::server::{PeerCreds, PeerPrincipalMap};

use generated::mvp_event_api_server::{MvpEventApi, MvpEventApiServer};

/// Generated stubs: control + event services share the `agentos.spec.v1`
/// package in one compiled file.
pub mod generated {
    tonic::include_proto!("agentos.spec.v1");
}

/// Client stub for tests and `agentctl`.
pub use generated::mvp_event_api_client::MvpEventApiClient;

/// Event service: journal reads + live-bus fan-out under peer auth.
pub struct EventApiService {
    journal: Arc<dyn EventJournalPort>,
    bus: Arc<LiveBus>,
    principals: Arc<dyn PeerPrincipalMap>,
}

impl EventApiService {
    /// Binds the journal, live bus, and peer map.
    pub fn new(
        journal: Arc<dyn EventJournalPort>,
        bus: Arc<LiveBus>,
        principals: Arc<dyn PeerPrincipalMap>,
    ) -> Self {
        Self {
            journal,
            bus,
            principals,
        }
    }

    /// Builds the tonic server wrapper.
    pub fn into_server(self) -> MvpEventApiServer<Self> {
        MvpEventApiServer::new(self)
    }

    fn authorize<T>(&self, request: &Request<T>) -> Result<(), Status> {
        let creds = request
            .extensions()
            .get::<PeerCreds>()
            .copied()
            .ok_or_else(|| Status::unauthenticated("missing peer credentials"))?;
        self.principals
            .principal_for_uid(creds.uid)
            .map(|_| ())
            .ok_or_else(|| Status::permission_denied("peer uid has no mapped principal"))
    }
}

/// Projects a durable envelope onto the wire: `Secret` payloads never
/// leave the journal (only metadata rides out).
fn project(event: &events::envelope::EventEnvelope) -> EventEnvelope {
    let payload = if event.sensitivity == SensitivityClass::Secret {
        Vec::new()
    } else {
        event.payload.clone()
    };
    EventEnvelope {
        event_id: event.event_id.to_string(),
        event_type: event.event_type.clone(),
        event_version: event.event_version,
        stream_key: event.stream_key.as_str().to_owned(),
        sequence: event.sequence,
        occurred_unix_ms: event.occurred_unix_ms,
        run_id: event.run_id.map(|id| id.to_string()).unwrap_or_default(),
        task_id: event.task_id.map(|id| id.to_string()).unwrap_or_default(),
        session_id: event
            .session_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        effect_id: event.effect_id.map(|id| id.to_string()).unwrap_or_default(),
        causation_id: event.causation_id.clone().unwrap_or_default(),
        correlation_id: event.correlation_id.clone().unwrap_or_default(),
        sensitivity: event.sensitivity.to_wire(),
        retention: event.retention.to_wire(),
        payload,
    }
}

fn parse_stream_key(field: &str, raw: &str) -> Result<StreamKey, Status> {
    StreamKey::from_str(raw)
        .map_err(|_| Status::invalid_argument(format!("{field} is not a canonical stream key")))
}

fn unavailable(e: errors::KernelError) -> Status {
    let code = match e.code() {
        ErrorCode::NotFound => Code::NotFound,
        ErrorCode::FailedPrecondition => Code::FailedPrecondition,
        ErrorCode::Conflict => Code::AlreadyExists,
        ErrorCode::InvalidArgument => Code::InvalidArgument,
        ErrorCode::ResourceExhausted => Code::ResourceExhausted,
        _ => Code::Unavailable,
    };
    Status::new(code, e.to_string())
}

#[async_trait::async_trait]
impl MvpEventApi for EventApiService {
    async fn read_stream(
        &self,
        request: Request<ReadEventStreamRequest>,
    ) -> Result<Response<ReadEventStreamResponse>, Status> {
        self.authorize(&request)?;
        let req = request.into_inner();
        let stream_key = parse_stream_key("stream_key", &req.stream_key)?;
        let page = self
            .journal
            .read_stream(&stream_key, req.from_sequence, req.limit)
            .await
            .map_err(unavailable)?;
        Ok(Response::new(ReadEventStreamResponse {
            events: page.events.iter().map(project).collect(),
            retention_gap: page.retention_gap,
        }))
    }

    type SubscribeStream = Pin<Box<dyn Stream<Item = Result<EventStreamFrame, Status>> + Send>>;

    async fn subscribe(
        &self,
        request: Request<SubscribeEventsRequest>,
    ) -> Result<Response<Self::SubscribeStream>, Status> {
        self.authorize(&request)?;
        let req = request.into_inner();
        let stream_key = parse_stream_key("stream_key", &req.stream_key)?;
        let stream_key_text = req.stream_key.clone();

        // Attach live first so events appended during the journal replay
        // are buffered by the broadcast channel; the handoff dedupes by
        // sequence so nothing is skipped or delivered twice.
        let mut sub = self.bus.subscribe();
        let journal = self.journal.clone();

        let (tx, rx) = mpsc::channel::<Result<EventStreamFrame, Status>>(256);
        tokio::spawn(async move {
            // Phase 1 — replay durable history after the requested cursor.
            let mut last_sequence = req.after_sequence;
            let mut from = req.after_sequence;
            loop {
                let page = match journal.read_stream(&stream_key, from, 256).await {
                    Ok(page) => page,
                    Err(e) => {
                        let _ = tx.send(Err(unavailable(e))).await;
                        return;
                    }
                };
                if page.events.is_empty() {
                    break;
                }
                for event in &page.events {
                    last_sequence = event.sequence;
                    if tx
                        .send(Ok(EventStreamFrame {
                            frame: Some(event_stream_frame::Frame::Event(project(event))),
                        }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                from = last_sequence;
            }

            // Phase 2 — drain buffered live items newer than the replay
            // tail, then follow the bus until lag or close.
            loop {
                match sub.next().await {
                    LiveItem::Event(event) => {
                        if event.sequence <= last_sequence {
                            continue;
                        }
                        last_sequence = event.sequence;
                        if tx
                            .send(Ok(EventStreamFrame {
                                frame: Some(event_stream_frame::Frame::Event(project(&event))),
                            }))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    LiveItem::Lagged { .. } => {
                        let _ = tx
                            .send(Ok(EventStreamFrame {
                                frame: Some(event_stream_frame::Frame::Lag(LagNotice {
                                    stream_key: stream_key_text.clone(),
                                    resume_sequence: last_sequence,
                                })),
                            }))
                            .await;
                        return;
                    }
                    LiveItem::BusClosed => return,
                }
            }
        });
        Ok(Response::new(Box::pin(
            tokio_stream::wrappers::ReceiverStream::new(rx),
        )))
    }
}
