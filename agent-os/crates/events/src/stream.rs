//! Canonical durable stream keys.
//!
//! The canonical forms are fixed by the event pipeline spec:
//!
//! ```text
//! run/<run-id>
//! task/<task-id>
//! session/<session-id>
//! effect/<effect-id>
//! config/global
//! adapter/<adapter-id>/<version>/<digest>
//! security/principal/<principal-id>
//! ```
//!
//! Construction and parsing are strict, so every [`StreamKey`] round-trips
//! through [`Display`](std::fmt::Display) and [`FromStr`].

use std::fmt;
use std::str::FromStr;

use domain::ids::{AdapterId, EffectId, EventStreamKey, PrincipalId, RunId, SessionId, TaskId};
use errors::codes::{ErrorCode, RetryClass};

/// Category of a canonical event stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum StreamKind {
    /// `run/<run-id>`
    Run,
    /// `task/<task-id>`
    Task,
    /// `session/<session-id>`
    Session,
    /// `effect/<effect-id>`
    Effect,
    /// `config/global`
    ConfigGlobal,
    /// `adapter/<adapter-id>/<version>/<digest>`
    Adapter,
    /// `security/principal/<principal-id>`
    Principal,
}

/// Validated canonical stream key.
///
/// The wrapped [`EventStreamKey`] enforces the canonical alphabet; the cached
/// [`StreamKind`] is derived at construction or parse time, so [`StreamKey::kind`]
/// is total.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamKey(EventStreamKey, StreamKind);

impl StreamKey {
    /// Canonical `run/<run-id>` key.
    pub fn run(id: RunId) -> Self {
        Self::canonical(&["run", &id.to_hyphenated()])
    }

    /// Canonical `task/<task-id>` key.
    pub fn task(id: TaskId) -> Self {
        Self::canonical(&["task", &id.to_hyphenated()])
    }

    /// Canonical `session/<session-id>` key.
    pub fn session(id: SessionId) -> Self {
        Self::canonical(&["session", &id.to_hyphenated()])
    }

    /// Canonical `effect/<effect-id>` key.
    pub fn effect(id: EffectId) -> Self {
        Self::canonical(&["effect", &id.to_hyphenated()])
    }

    /// The single `config/global` key.
    pub fn config_global() -> Self {
        Self::canonical(&["config", "global"])
    }

    /// Canonical `adapter/<adapter-id>/<version>/<digest>` key.
    ///
    /// Fails with `InvalidArgument`/`Never` when `version` or `digest` is
    /// empty or contains bytes outside the canonical stream-key alphabet.
    pub fn adapter(id: AdapterId, version: &str, digest: &str) -> errors::Result<Self> {
        Self::try_assemble(&["adapter", &id.to_hyphenated(), version, digest])
    }

    /// Canonical `security/principal/<principal-id>` key.
    pub fn principal(id: PrincipalId) -> Self {
        Self::canonical(&["security", "principal", &id.to_hyphenated()])
    }

    /// Returns the stream category.
    pub fn kind(&self) -> StreamKind {
        self.1
    }

    /// Borrows the canonical text.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Borrows the underlying domain key.
    pub(crate) fn event_stream_key(&self) -> &EventStreamKey {
        &self.0
    }

    fn canonical(segments: &[&str]) -> Self {
        match Self::try_assemble(segments) {
            Ok(key) => key,
            Err(_) => unreachable!("typed identifiers always form canonical stream keys"),
        }
    }

    fn try_assemble(segments: &[&str]) -> errors::Result<Self> {
        let text = segments.join("/");
        let kind = classify(&text).ok_or_else(invalid_stream_key)?;
        let key = EventStreamKey::new(text).map_err(|_| invalid_stream_key())?;
        Ok(Self(key, kind))
    }
}

impl FromStr for StreamKey {
    type Err = errors::KernelError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::try_assemble(&[text])
    }
}

impl fmt::Display for StreamKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0.as_str())
    }
}

fn classify(text: &str) -> Option<StreamKind> {
    let segments: Vec<&str> = text.split('/').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    match segments.as_slice() {
        ["run", _] => Some(StreamKind::Run),
        ["task", _] => Some(StreamKind::Task),
        ["session", _] => Some(StreamKind::Session),
        ["effect", _] => Some(StreamKind::Effect),
        ["config", "global"] => Some(StreamKind::ConfigGlobal),
        ["adapter", _, _, _] => Some(StreamKind::Adapter),
        ["security", "principal", _] => Some(StreamKind::Principal),
        _ => None,
    }
}

fn invalid_stream_key() -> errors::KernelError {
    errors::KernelError::new(
        ErrorCode::InvalidArgument,
        RetryClass::Never,
        "stream key is not in a canonical form",
    )
}
