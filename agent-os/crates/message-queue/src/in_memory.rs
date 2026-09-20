//! In-memory `MessageQueuePort` adapter: generation-global, non-durable.
//!
//! Semantics: a single FIFO pending queue. `consume(subscription)` moves the
//! next pending messages into that subscription's in-flight set under unique
//! `delivery_id`s; `ack` removes permanently, `nack` returns to the front of
//! pending. Bounded by [`QUEUE_CAPACITY_MESSAGES`] counting pending plus
//! in-flight messages; overflow fails `ResourceExhausted` — the publisher
//! applies backpressure, nothing is silently dropped.
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use domain::generated::contract::{
    GenericResult, QueueAckRequest, QueueConsumeRequest, QueueDelivery, QueueNackRequest,
    QueuePublishRequest, QueuePublishResult,
};
use errors::codes::{ErrorCode, RetryClass};
use errors::{KernelError, Result};

use crate::{MessageQueuePort, QUEUE_CAPACITY_MESSAGES, QueueCapabilities};

struct Stored {
    stream: String,
    payload: Vec<u8>,
}

struct InFlight {
    stream: String,
    message_id: String,
    payload: Vec<u8>,
}

struct State {
    /// Payload identity dedupe: `(stream, message_id)` -> stored payload hash.
    live_ids: HashMap<(String, String), Vec<u8>>,
    /// FIFO of `(stream, message_id)` awaiting a consumer.
    pending: VecDeque<(String, String)>,
    /// Payloads keyed by `(stream, message_id)`.
    messages: HashMap<(String, String), Stored>,
    /// `delivery_id` -> in-flight delivery.
    in_flight: HashMap<String, InFlight>,
    next_delivery: u64,
}

impl State {
    fn occupied(&self) -> usize {
        self.pending.len() + self.in_flight.len()
    }
}

/// Non-durable in-memory adapter. Truthfully reports `durable=false`,
/// `replay=false`, `cross_restart=false`.
pub struct InMemoryQueue {
    state: Mutex<State>,
    capacity: usize,
}

impl Default for InMemoryQueue {
    fn default() -> Self {
        Self::new(QUEUE_CAPACITY_MESSAGES)
    }
}

impl InMemoryQueue {
    /// `capacity` bounds total live messages (pending + in-flight).
    pub fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(State {
                live_ids: HashMap::new(),
                pending: VecDeque::new(),
                messages: HashMap::new(),
                in_flight: HashMap::new(),
                next_delivery: 1,
            }),
            capacity,
        }
    }

    fn parse_max(options_json: &[u8]) -> usize {
        // `{"max": <n>}`; absent/unparseable -> 1. Deterministic, no panic.
        let text = std::str::from_utf8(options_json).unwrap_or("");
        let marker = "\"max\"";
        let Some(pos) = text.find(marker) else {
            return 1;
        };
        text[pos + marker.len()..]
            .trim_start_matches([':', ' ', '\t'])
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(1)
    }
}

#[async_trait::async_trait]
impl MessageQueuePort for InMemoryQueue {
    fn capabilities(&self) -> QueueCapabilities {
        QueueCapabilities {
            durable: false,
            replay: false,
            cross_restart: false,
        }
    }

    async fn publish(&self, request: QueuePublishRequest) -> Result<QueuePublishResult> {
        let key = (request.stream.clone(), request.message_id.clone());
        let mut state = self.state.lock().expect("queue mutex");
        if let Some(existing) = state.live_ids.get(&key) {
            if *existing == request.payload {
                return Ok(QueuePublishResult {
                    message_id: request.message_id,
                });
            }
            return Err(KernelError::new(
                ErrorCode::Conflict,
                RetryClass::Never,
                "message_id already published with different payload",
            ));
        }
        if state.occupied() >= self.capacity {
            return Err(KernelError::new(
                ErrorCode::ResourceExhausted,
                RetryClass::Safe,
                "queue capacity reached",
            ));
        }
        state.live_ids.insert(key.clone(), request.payload.clone());
        state.pending.push_back(key.clone());
        state.messages.insert(
            key,
            Stored {
                stream: request.stream,
                payload: request.payload,
            },
        );
        Ok(QueuePublishResult {
            message_id: request.message_id,
        })
    }

    async fn consume(&self, request: &QueueConsumeRequest) -> Result<Vec<QueueDelivery>> {
        let max = Self::parse_max(&request.options_json).max(1);
        let mut state = self.state.lock().expect("queue mutex");
        let mut out = Vec::new();
        for _ in 0..max {
            let Some(key) = state.pending.pop_front() else {
                break;
            };
            // The payload migrates into the in-flight entry; `live_ids` keeps
            // the dedupe key so the slot stays occupied until `ack`.
            let Some(stored) = state.messages.remove(&key) else {
                continue;
            };
            let delivery_id = format!("d-{}", state.next_delivery);
            state.next_delivery += 1;
            out.push(QueueDelivery {
                delivery_id: delivery_id.clone(),
                message_id: key.1.clone(),
                payload: stored.payload.clone(),
            });
            state.in_flight.insert(
                delivery_id,
                InFlight {
                    stream: stored.stream.clone(),
                    message_id: key.1,
                    payload: stored.payload,
                },
            );
        }
        Ok(out)
    }

    async fn ack(&self, request: &QueueAckRequest) -> Result<GenericResult> {
        let mut state = self.state.lock().expect("queue mutex");
        let Some(flight) = state.in_flight.remove(&request.delivery_id) else {
            return Err(KernelError::new(
                ErrorCode::NotFound,
                RetryClass::Never,
                "unknown delivery_id",
            ));
        };
        state
            .messages
            .remove(&(flight.stream.clone(), flight.message_id.clone()));
        state.live_ids.remove(&(flight.stream, flight.message_id));
        Ok(GenericResult::default())
    }

    async fn nack(&self, request: &QueueNackRequest) -> Result<GenericResult> {
        let mut state = self.state.lock().expect("queue mutex");
        let Some(flight) = state.in_flight.remove(&request.delivery_id) else {
            return Err(KernelError::new(
                ErrorCode::NotFound,
                RetryClass::Never,
                "unknown delivery_id",
            ));
        };
        let key = (flight.stream, flight.message_id);
        state.messages.insert(
            key.clone(),
            Stored {
                stream: key.0.clone(),
                payload: flight.payload,
            },
        );
        state.pending.push_front(key);
        Ok(GenericResult::default())
    }
}
