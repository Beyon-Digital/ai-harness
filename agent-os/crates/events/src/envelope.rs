//! Validated event envelopes: builder, classification policy, and codec.
//!
//! The builder is the only sanctioned constructor for durable events: it
//! requires every field the journal and journal-first dispatcher depend on and
//! enforces the event type's classification floor from the embedded catalog
//! (R1.1-R1.5). The envelope mirrors `contracts/events/event.proto`; encoding
//! and decoding preserve every field exactly (R1.6).

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::{self, Display};
use std::str::FromStr;

use domain::generated::contract;
use domain::ids::{EffectId, EventId, RunId, SessionId, TaskId};
use domain::provider::SystemIdProvider;
use domain::security::{RetentionClass, SensitivityClass};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use serde::Deserialize;

use crate::cursor::{EventCursor, EventCursorExt};
use crate::stream::StreamKey;

/// Minimum sensitivity and default retention an event type carries.
pub trait ClassificationPolicy: Send + Sync {
    /// Returns the lowest sensitivity the event type may be assigned, or
    /// `None` when the type has no catalog entry (and therefore no floor).
    fn minimum(&self, event_type: &str) -> Option<SensitivityClass>;

    /// Returns the retention class the catalog declares for the event type,
    /// or `None` when the type has no catalog entry.
    fn default_retention(&self, event_type: &str) -> Option<RetentionClass>;
}

#[derive(Deserialize)]
struct CatalogDocument {
    events: Vec<CatalogEntry>,
}

#[derive(Deserialize)]
struct CatalogEntry {
    id: String,
    default_sensitivity: String,
    default_retention: String,
}

/// Classification floor and retention default parsed once from the embedded
/// event catalog.
pub struct CatalogClassificationPolicy {
    floors: HashMap<String, SensitivityClass>,
    retentions: HashMap<String, RetentionClass>,
}

impl CatalogClassificationPolicy {
    /// Parses `agent-os/proto/events/catalog.yaml`, embedded at compile time.
    pub fn embedded() -> errors::Result<Self> {
        Self::from_yaml(include_str!("../../../proto/events/catalog.yaml"))
    }

    fn from_yaml(yaml: &str) -> errors::Result<Self> {
        let document: CatalogDocument = serde_yaml::from_str(yaml).map_err(|error| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "embedded event catalog is not valid YAML",
            )
            .with_source(error)
        })?;
        let mut floors = HashMap::with_capacity(document.events.len());
        let mut retentions = HashMap::with_capacity(document.events.len());
        for entry in document.events {
            let sensitivity = catalog_sensitivity(&entry.default_sensitivity).ok_or_else(|| {
                KernelError::new(
                    ErrorCode::Internal,
                    RetryClass::Never,
                    "embedded event catalog declares an unknown sensitivity",
                )
            })?;
            let retention = catalog_retention(&entry.default_retention).ok_or_else(|| {
                KernelError::new(
                    ErrorCode::Internal,
                    RetryClass::Never,
                    "embedded event catalog declares an unknown retention",
                )
            })?;
            floors.insert(entry.id.clone(), sensitivity);
            retentions.insert(entry.id, retention);
        }
        Ok(Self { floors, retentions })
    }
}

impl ClassificationPolicy for CatalogClassificationPolicy {
    fn minimum(&self, event_type: &str) -> Option<SensitivityClass> {
        self.floors.get(event_type).copied()
    }

    fn default_retention(&self, event_type: &str) -> Option<RetentionClass> {
        self.retentions.get(event_type).copied()
    }
}

fn catalog_sensitivity(text: &str) -> Option<SensitivityClass> {
    match text {
        "public" => Some(SensitivityClass::Public),
        "internal" => Some(SensitivityClass::Internal),
        "confidential" => Some(SensitivityClass::Confidential),
        "secret" => Some(SensitivityClass::Secret),
        _ => None,
    }
}

fn catalog_retention(text: &str) -> Option<RetentionClass> {
    match text {
        "ephemeral" => Some(RetentionClass::Ephemeral),
        "standard" => Some(RetentionClass::Standard),
        "audit" => Some(RetentionClass::Audit),
        _ => None,
    }
}

/// Validated durable event, mirroring `contracts/events/event.proto` exactly.
///
/// The manual [`fmt::Debug`] rendering elides the payload bytes and prints
/// `payload_len` instead, so logging an envelope cannot leak payload content
/// (N1).
#[derive(Clone, PartialEq, Eq)]
pub struct EventEnvelope {
    /// Stable identity of the event.
    pub event_id: EventId,
    /// Event type name from the contracts.
    pub event_type: String,
    /// Version of the event type schema.
    pub event_version: u32,
    /// Canonical stream that owns the event.
    pub stream_key: StreamKey,
    /// Per-stream journal position.
    pub sequence: u64,
    /// Event time as Unix milliseconds.
    pub occurred_unix_ms: i64,
    /// Run the event belongs to, when applicable.
    pub run_id: Option<RunId>,
    /// Task the event belongs to, when applicable.
    pub task_id: Option<TaskId>,
    /// Session the event belongs to, when applicable.
    pub session_id: Option<SessionId>,
    /// Effect record the event belongs to, when applicable.
    pub effect_id: Option<EffectId>,
    /// Event that directly caused this one, when known.
    pub causation_id: Option<String>,
    /// Correlation identifier linking related events, when known.
    pub correlation_id: Option<String>,
    /// Sensitivity classification of the payload.
    pub sensitivity: SensitivityClass,
    /// Retention class of the event.
    pub retention: RetentionClass,
    /// Serialized event payload; never rendered in errors or logs.
    pub payload: Vec<u8>,
}

impl fmt::Debug for EventEnvelope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EventEnvelope")
            .field("event_id", &self.event_id)
            .field("event_type", &self.event_type)
            .field("event_version", &self.event_version)
            .field("stream_key", &self.stream_key)
            .field("sequence", &self.sequence)
            .field("occurred_unix_ms", &self.occurred_unix_ms)
            .field("run_id", &self.run_id)
            .field("task_id", &self.task_id)
            .field("session_id", &self.session_id)
            .field("effect_id", &self.effect_id)
            .field("causation_id", &self.causation_id)
            .field("correlation_id", &self.correlation_id)
            .field("sensitivity", &self.sensitivity)
            .field("retention", &self.retention)
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

impl EventEnvelope {
    /// Encodes the envelope as protobuf wire bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        prost::Message::encode_to_vec(&self.to_contract())
    }

    /// Decodes protobuf wire bytes into a validated envelope.
    ///
    /// Failures never echo the input bytes.
    pub fn from_bytes(bytes: &[u8]) -> errors::Result<Self> {
        let raw = <contract::EventEnvelope as prost::Message>::decode(bytes).map_err(|error| {
            invalid_envelope("event envelope is not valid contract bytes").with_source(error)
        })?;

        let event_id = EventId::from_str(&raw.event_id)
            .map_err(|_| invalid_envelope("event envelope event id is invalid"))?;
        let stream_key = StreamKey::from_str(&raw.stream_key)
            .map_err(|_| invalid_envelope("event envelope stream key is invalid"))?;
        let sensitivity = SensitivityClass::from_wire(raw.sensitivity)
            .map_err(|_| invalid_envelope("event envelope sensitivity is unknown"))?;
        if sensitivity == SensitivityClass::Unspecified {
            return Err(invalid_envelope("event envelope sensitivity is absent"));
        }
        let retention = RetentionClass::from_wire(raw.retention)
            .map_err(|_| invalid_envelope("event envelope retention is unknown"))?;
        if retention == RetentionClass::Unspecified {
            return Err(invalid_envelope("event envelope retention is absent"));
        }

        Ok(Self {
            event_id,
            event_type: raw.event_type,
            event_version: raw.event_version,
            stream_key,
            sequence: raw.sequence,
            occurred_unix_ms: raw.occurred_unix_ms,
            run_id: optional_id(&raw.run_id, "run id")?,
            task_id: optional_id(&raw.task_id, "task id")?,
            session_id: optional_id(&raw.session_id, "session id")?,
            effect_id: optional_id(&raw.effect_id, "effect id")?,
            causation_id: optional_text(raw.causation_id),
            correlation_id: optional_text(raw.correlation_id),
            sensitivity,
            retention,
            payload: raw.payload,
        })
    }

    /// Returns the canonical cursor for this event's stream position.
    pub fn cursor(&self) -> EventCursor {
        EventCursor::for_event(&self.stream_key, self.sequence)
    }

    fn to_contract(&self) -> contract::EventEnvelope {
        contract::EventEnvelope {
            event_id: self.event_id.to_hyphenated(),
            event_type: self.event_type.clone(),
            event_version: self.event_version,
            stream_key: self.stream_key.as_str().to_owned(),
            sequence: self.sequence,
            occurred_unix_ms: self.occurred_unix_ms,
            run_id: text_of(&self.run_id),
            task_id: text_of(&self.task_id),
            session_id: text_of(&self.session_id),
            effect_id: text_of(&self.effect_id),
            causation_id: self.causation_id.clone().unwrap_or_default(),
            correlation_id: self.correlation_id.clone().unwrap_or_default(),
            sensitivity: self.sensitivity.to_wire(),
            retention: self.retention.to_wire(),
            payload: self.payload.clone(),
        }
    }
}

/// Builder for [`EventEnvelope`] with required-field and floor validation.
pub struct EventBuilder {
    event_id: EventId,
    event_type: String,
    event_version: u32,
    stream_key: StreamKey,
    sequence: Option<u64>,
    occurred_at_ms: Option<i64>,
    run_id: Option<RunId>,
    task_id: Option<TaskId>,
    session_id: Option<SessionId>,
    effect_id: Option<EffectId>,
    causation_id: Option<String>,
    correlation_id: Option<String>,
    sensitivity: Option<SensitivityClass>,
    retention: Option<RetentionClass>,
    payload: Option<Vec<u8>>,
}

impl EventBuilder {
    /// Starts a builder for `event_type`. A fresh event id is drawn from the
    /// system provider; use [`EventBuilder::event_id`] to supply one instead.
    pub fn new(event_type: impl Into<String>, event_version: u32, stream_key: StreamKey) -> Self {
        Self {
            event_id: EventId::new(&SystemIdProvider),
            event_type: event_type.into(),
            event_version,
            stream_key,
            sequence: None,
            occurred_at_ms: None,
            run_id: None,
            task_id: None,
            session_id: None,
            effect_id: None,
            causation_id: None,
            correlation_id: None,
            sensitivity: None,
            retention: None,
            payload: None,
        }
    }

    /// Overrides the generated event id.
    pub fn event_id(mut self, id: EventId) -> Self {
        self.event_id = id;
        self
    }

    /// Sets the per-stream journal position.
    pub fn sequence(mut self, sequence: u64) -> Self {
        self.sequence = Some(sequence);
        self
    }

    /// Sets the event time as Unix milliseconds.
    pub fn occurred_at_ms(mut self, ms: i64) -> Self {
        self.occurred_at_ms = Some(ms);
        self
    }

    /// Sets the correlation identifier.
    pub fn correlation_id(mut self, id: impl Into<String>) -> Self {
        self.correlation_id = Some(id.into());
        self
    }

    /// Sets the causing event identifier.
    pub fn causation_id(mut self, id: impl Into<String>) -> Self {
        self.causation_id = Some(id.into());
        self
    }

    /// Sets the run the event belongs to.
    pub fn run_id(mut self, id: RunId) -> Self {
        self.run_id = Some(id);
        self
    }

    /// Sets the task the event belongs to.
    pub fn task_id(mut self, id: TaskId) -> Self {
        self.task_id = Some(id);
        self
    }

    /// Sets the session the event belongs to.
    pub fn session_id(mut self, id: SessionId) -> Self {
        self.session_id = Some(id);
        self
    }

    /// Sets the effect record the event belongs to.
    pub fn effect_id(mut self, id: EffectId) -> Self {
        self.effect_id = Some(id);
        self
    }

    /// Sets the sensitivity classification.
    pub fn sensitivity(mut self, class: SensitivityClass) -> Self {
        self.sensitivity = Some(class);
        self
    }

    /// Overrides the catalog's default retention class.
    pub fn retention(mut self, class: RetentionClass) -> Self {
        self.retention = Some(class);
        self
    }

    /// Sets the serialized payload bytes.
    pub fn payload(mut self, bytes: Vec<u8>) -> Self {
        self.payload = Some(bytes);
        self
    }

    /// Validates required fields and the classification floor from `policy`.
    pub fn build(self, policy: &dyn ClassificationPolicy) -> errors::Result<EventEnvelope> {
        if self.event_type.trim().is_empty() {
            return Err(invalid_envelope("event type is required"));
        }
        if self.event_version == 0 {
            return Err(invalid_envelope("event version is required"));
        }
        let sequence = self
            .sequence
            .ok_or_else(|| invalid_envelope("event sequence is required"))?;
        let occurred_unix_ms = self
            .occurred_at_ms
            .ok_or_else(|| invalid_envelope("event occurred time is required"))?;
        let sensitivity = self
            .sensitivity
            .ok_or_else(|| invalid_envelope("event classification is required"))?;
        if sensitivity == SensitivityClass::Unspecified {
            return Err(invalid_envelope("event classification is required"));
        }
        let payload = self
            .payload
            .ok_or_else(|| invalid_envelope("event payload is required"))?;
        let retention = match self.retention {
            Some(class) => class,
            None => policy
                .default_retention(&self.event_type)
                .ok_or_else(|| invalid_envelope("event retention is required"))?,
        };
        if retention == RetentionClass::Unspecified {
            return Err(invalid_envelope("event retention is required"));
        }
        if let Some(minimum) = policy.minimum(&self.event_type)
            && sensitivity.to_wire() < minimum.to_wire()
        {
            return Err(KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                format!(
                    "event {} requires a classification at or above {}",
                    self.event_type,
                    sensitivity_token(minimum)
                ),
            ));
        }

        Ok(EventEnvelope {
            event_id: self.event_id,
            event_type: self.event_type,
            event_version: self.event_version,
            stream_key: self.stream_key,
            sequence,
            occurred_unix_ms,
            run_id: self.run_id,
            task_id: self.task_id,
            session_id: self.session_id,
            effect_id: self.effect_id,
            causation_id: self.causation_id,
            correlation_id: self.correlation_id,
            sensitivity,
            retention,
            payload,
        })
    }
}

fn invalid_envelope(message: impl Into<Cow<'static, str>>) -> KernelError {
    KernelError::new(ErrorCode::InvalidArgument, RetryClass::Never, message)
}

fn optional_id<T: FromStr>(text: &str, field: &'static str) -> errors::Result<Option<T>> {
    if text.is_empty() {
        return Ok(None);
    }
    T::from_str(text)
        .map(Some)
        .map_err(|_| invalid_envelope(format!("event envelope {field} is invalid")))
}

fn optional_text(text: String) -> Option<String> {
    if text.is_empty() { None } else { Some(text) }
}

fn text_of<T: Display>(value: &Option<T>) -> String {
    value.as_ref().map(ToString::to_string).unwrap_or_default()
}

fn sensitivity_token(class: SensitivityClass) -> &'static str {
    match class {
        SensitivityClass::Unspecified => "unspecified",
        SensitivityClass::Public => "public",
        SensitivityClass::Internal => "internal",
        SensitivityClass::Confidential => "confidential",
        SensitivityClass::Secret => "secret",
    }
}
