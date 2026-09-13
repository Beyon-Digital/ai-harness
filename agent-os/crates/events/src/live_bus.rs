//! Bounded live delivery with explicit lag, plus a lossy ephemeral channel.
//!
//! [`LiveBus`] fans journal-backed events out to subscribers over a bounded
//! `tokio::sync::broadcast` channel. A subscriber that falls behind the
//! capacity receives exactly one [`LiveItem::Lagged`] carrying the last
//! journal-backed cursor it actually delivered (`None` when it delivered
//! none), and the subscription is then terminal: the client resumes through
//! the durable journal from that cursor, or from stream inception, and no
//! cursor is ever fabricated (R4.1-R4.3, R4.5). Dropping the bus yields
//! [`LiveItem::BusClosed`].
//!
//! [`EphemeralBus`] is a separate, lossy, counted channel for transient bytes.
//! Its overflow never touches durable cursors or durable delivery (R4.4, P3).

use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::broadcast;
use tokio::sync::mpsc;

use crate::cursor::EventCursor;
use crate::dispatcher::LiveSink;
use crate::envelope::EventEnvelope;

/// One item produced by a live subscription.
///
/// `Event` carries the envelope inline so normal delivery stays allocation
/// free; the rare terminal variants make the enum intentionally asymmetric.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveItem {
    /// A journal-backed event, delivered in publication order.
    Event(EventEnvelope),
    /// The subscription fell behind; resume the journal from this cursor.
    ///
    /// `resume_from` is the last cursor the subscription actually delivered,
    /// so a journal read strictly after it recovers every missed event with no
    /// gap and no duplicate. `None` means no event was ever delivered: the
    /// consumer resumes from stream inception.
    Lagged { resume_from: Option<EventCursor> },
    /// The bus was dropped while the subscription was live.
    ///
    /// The subscription ends without a resume cursor; no event is fabricated
    /// and no cursor is invented.
    BusClosed,
}

/// Bounded in-process fan-out of journal-backed events.
pub struct LiveBus {
    sender: broadcast::Sender<EventEnvelope>,
}

impl LiveBus {
    /// Creates a bus whose per-subscriber buffer holds `capacity` events.
    ///
    /// # Panics
    ///
    /// Panics when `capacity` is zero; a bus with no buffer could never
    /// deliver an event before lagging.
    pub fn new(capacity: usize) -> Self {
        let (sender, _receiver) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Broadcasts an already-journaled event to every live subscriber.
    ///
    /// Publishing with no subscribers is a no-op; the event remains durable in
    /// the journal, and nothing is fabricated for live delivery.
    pub fn publish(&self, event: &EventEnvelope) {
        if self.sender.receiver_count() == 0 {
            return;
        }
        let _ = self.sender.send(event.clone());
    }

    /// Registers a new subscription positioned at the current tail.
    ///
    /// The subscription tracks the last cursor it delivers, starting from none:
    /// only events received after this call can advance it.
    pub fn subscribe(&self) -> LiveSubscription {
        LiveSubscription {
            receiver: self.sender.subscribe(),
            last_delivered: None,
            ended: false,
        }
    }

    /// Number of subscriptions currently registered.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl LiveSink for LiveBus {
    fn publish(&self, event: &EventEnvelope) {
        LiveBus::publish(self, event);
    }
}

/// A live subscription over one [`LiveBus`].
pub struct LiveSubscription {
    receiver: broadcast::Receiver<EventEnvelope>,
    last_delivered: Option<EventCursor>,
    ended: bool,
}

impl LiveSubscription {
    /// Awaits the next live item.
    ///
    /// Delivery is ordered. When the bounded buffer overflows, the
    /// subscription reports [`LiveItem::Lagged`] once, carrying the last
    /// journal-backed cursor it delivered (`None` when it never delivered
    /// one), and becomes terminal. When the owning [`LiveBus`] is dropped it
    /// reports [`LiveItem::BusClosed`] and becomes terminal. No cursor is ever
    /// fabricated.
    ///
    /// # Panics
    ///
    /// Panics on any call after the subscription became terminal.
    pub async fn next(&mut self) -> LiveItem {
        assert!(
            !self.ended,
            "live subscription is terminal after lag or bus closure"
        );
        match self.receiver.recv().await {
            Ok(event) => {
                self.last_delivered = Some(event.cursor());
                LiveItem::Event(event)
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                self.ended = true;
                LiveItem::Lagged {
                    resume_from: self.last_delivered.take(),
                }
            }
            Err(broadcast::error::RecvError::Closed) => {
                self.ended = true;
                LiveItem::BusClosed
            }
        }
    }
}

/// Bounded, lossy channel for ephemeral bytes.
///
/// Dropped messages are counted; nothing published here can advance a durable
/// cursor or affect [`LiveBus`] delivery.
pub struct EphemeralBus {
    sender: mpsc::Sender<Vec<u8>>,
    _receiver: mpsc::Receiver<Vec<u8>>,
    dropped: AtomicU64,
}

impl EphemeralBus {
    /// Creates a bus whose buffer holds `capacity` ephemeral messages.
    ///
    /// # Panics
    ///
    /// Panics when `capacity` is zero.
    pub fn new(capacity: usize) -> Self {
        let (sender, receiver) = mpsc::channel(capacity);
        Self {
            sender,
            _receiver: receiver,
            dropped: AtomicU64::new(0),
        }
    }

    /// Attempts a non-blocking publish.
    ///
    /// Returns `true` when the message entered the buffer and `false` when it
    /// was dropped because the buffer was full or the channel was closed,
    /// incrementing [`Self::dropped`].
    pub fn try_publish(&self, bytes: Vec<u8>) -> bool {
        match self.sender.try_send(bytes) {
            Ok(()) => true,
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Number of messages dropped by [`Self::try_publish`].
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}
