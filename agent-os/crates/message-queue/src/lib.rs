//! Message queue port (`message-queue@1`) and the non-durable in-memory
//! generation-global adapter used by fixtures and internal wiring.
//!
//! The queue is a delivery convenience, never a source of truth: runtime
//! correctness derives from `KernelStore`, and workers must recover state
//! from the store rather than queue contents.
#![forbid(unsafe_code)]

pub mod in_memory;

use domain::generated::contract::{
    GenericResult, QueueAckRequest, QueueConsumeRequest, QueueDelivery, QueueNackRequest,
    QueuePublishRequest, QueuePublishResult,
};
use errors::Result;

pub use in_memory::InMemoryQueue;

/// `specs/limits.yaml` `queue.capacity_messages`.
pub const QUEUE_CAPACITY_MESSAGES: usize = 1024;

/// What an adapter actually guarantees — must be reported truthfully.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueCapabilities {
    /// Messages survive process restart.
    pub durable: bool,
    /// Delivered/acked messages can be re-read later.
    pub replay: bool,
    /// In-flight deliveries survive a daemon restart.
    pub cross_restart: bool,
}

/// `MessageQueuePort` (see `proto/ports/message_queue.proto`). Consume returns
/// a snapshot batch of deliveries rather than a live stream; adapters that
/// support streaming wrap this.
#[async_trait::async_trait]
pub trait MessageQueuePort: Send + Sync {
    /// Truthful capability report for this adapter.
    fn capabilities(&self) -> QueueCapabilities;
    /// Publish a message on `stream`. `message_id` is caller-assigned payload
    /// identity: re-publishing the same `(stream, message_id)` with identical
    /// payload is idempotent; a different payload conflicts.
    async fn publish(&self, request: QueuePublishRequest) -> Result<QueuePublishResult>;
    /// Take up to `max` pending deliveries into `subscription`, marking them
    /// in-flight until [`Self::ack`] or [`Self::nack`]. `options_json` may carry
    /// `{"max": n}`; default is 1.
    async fn consume(&self, request: &QueueConsumeRequest) -> Result<Vec<QueueDelivery>>;
    /// Permanently remove the in-flight delivery.
    async fn ack(&self, request: &QueueAckRequest) -> Result<GenericResult>;
    /// Return the in-flight delivery to pending for redelivery.
    async fn nack(&self, request: &QueueNackRequest) -> Result<GenericResult>;
}
