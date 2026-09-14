//! Secret material leaves the daemon only through an authorized, audited
//! broker that prefers scoped operations and never renders values
//! (R4.1-R4.7, P4, N1).
//!
//! [`SecretsBroker`] evaluates a [`SecretUseRequest`] with
//! [`permissions::evaluate`] before any [`SecretStore`] call. A `Deny` or
//! `RequireApproval` decision releases no material and produces an error that
//! carries no secret content; an `Allow` decision returns raw bytes only as a
//! zeroizing [`SecretValue`] whose `Debug` is redacted. Every outcome is
//! reported to an [`AuditSink`] as actor, run, uri, and outcome only;
//! [`SecretsBroker::new`] wires the production [`TracingAuditSink`], and
//! tests inject a capturing sink through [`SecretsBroker::with_audit`].
//!
//! The catalogued `SecretActionPerformed` event (`produced_by: sign_or_act`)
//! is **not** staged by this crate: the broker has no write transaction or
//! principal stream. Staging it belongs to the composition root
//! (`process-supervisor`), which owns the transaction and can stage the event
//! on the principal stream in the same transaction as the brokered action's
//! bookkeeping. Audit records emitted here are the observability substrate's
//! structured logs, not the catalogue event.
//!
//! The documented test backend is [`InMemorySecretStore`]; on macOS,
//! [`MacOsKeychainSecretStore`] wraps `security-framework` directly (no shell
//! interpolation of values). There is no fallback from a failing backend to
//! plaintext.
#![forbid(unsafe_code)]

pub mod broker;
pub mod keychain;

use std::fmt;

use async_trait::async_trait;
use domain::ids::{ActorId, PrincipalId, RunId};
use identity::delegation::DelegationChain;
use observability::{Classification, Classified};
use permissions::GrantScope;
use zeroize::Zeroizing;

pub use broker::{InMemorySecretStore, NullAuditSink, SecretsBroker, TracingAuditSink};
#[cfg(target_os = "macos")]
pub use keychain::MacOsKeychainSecretStore;

/// Metadata describing one secret without exposing its value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretMetadata {
    /// Resource URI identifying the secret.
    pub uri: String,
    /// Backend-defined kind (for example `api-key`).
    pub kind: String,
    /// Opaque scope labels attached to the secret.
    pub scopes: Vec<String>,
}

/// Raw secret bytes that zeroize on drop and never render.
///
/// The bytes are reachable only through [`SecretValue::expose`]; `Debug`
/// prints a redacted marker, and there is no `Display` or serialization path.
pub struct SecretValue(Zeroizing<Vec<u8>>);

impl SecretValue {
    /// Wraps raw bytes in a zeroizing container.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(Zeroizing::new(bytes.into()))
    }

    /// The only access path to the raw bytes.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// The scoped result of a sign-or-act operation: an opaque reference, never
/// the material that produced it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActResult {
    /// Backend reference for the produced signature or action.
    pub reference: String,
}

/// One backend that can hold secrets. Access is brokered; stores never see a
/// [`PermissionRequest`](permissions::PermissionRequest).
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Returns metadata for `uri` without touching the value.
    async fn metadata(&self, uri: &str) -> errors::Result<SecretMetadata>;

    /// Returns the raw value for `uri`.
    async fn get(&self, uri: &str) -> errors::Result<SecretValue>;

    /// Performs `action` over `payload` inside the backend, returning a scoped
    /// reference instead of material.
    async fn sign_or_act(
        &self,
        uri: &str,
        action: &str,
        payload: &[u8],
    ) -> errors::Result<ActResult>;
}

/// Everything the broker needs to authorize one secret operation.
///
/// `grant_scopes` carries the grant rows for the chain's grants, loaded by the
/// caller from `capability_grants` exactly as the permission engine expects.
/// `operation` is the provider action for sign-or-act (for example `sign`);
/// raw and metadata access derive their capability from the method called.
#[derive(Clone, Debug)]
pub struct SecretUseRequest {
    /// Principal on whose behalf the operation runs.
    pub principal_id: PrincipalId,
    /// Actor exercising the authority.
    pub actor_id: ActorId,
    /// Run context when the operation belongs to a run.
    pub run_id: Option<RunId>,
    /// Loaded delegation chain from root to tip.
    pub chain: DelegationChain,
    /// Grant rows loaded for the chain's grants.
    pub grant_scopes: Vec<GrantScope>,
    /// Secret resource URI the operation targets.
    pub uri: String,
    /// Optional destinations the material may flow to.
    pub egress: Option<Vec<String>>,
    /// Provider action for sign-or-act operations.
    pub operation: String,
}

/// The terminal outcome of one brokered secret operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretUseOutcome {
    /// Authorization allowed raw or metadata access.
    Allowed,
    /// Authorization allowed a scoped sign-or-act operation.
    Acted,
    /// Authorization denied the operation.
    Denied,
    /// Authorization requires an operator approval first.
    ApprovalRequired,
    /// The operation was authorized but the backend failed.
    Failed,
}

impl SecretUseOutcome {
    /// Returns the stable lowercase token for this outcome.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Allowed => "allowed",
            Self::Acted => "acted",
            Self::Denied => "denied",
            Self::ApprovalRequired => "approval_required",
            Self::Failed => "failed",
        }
    }
}

impl fmt::Display for SecretUseOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One audit record: actor, run, uri, and outcome only, never a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretAuditRecord {
    /// Actor that exercised the request.
    pub actor_id: ActorId,
    /// Run context when the operation belonged to a run.
    pub run_id: Option<RunId>,
    /// Secret resource URI the operation targeted.
    pub uri: String,
    /// Terminal outcome of the operation.
    pub outcome: SecretUseOutcome,
}

impl Classified for SecretAuditRecord {
    fn classification(&self) -> Classification {
        Classification::Internal
    }
}

/// Receives one audit record per brokered secret operation.
///
/// Production wiring sends these records to the observability substrate as
/// structured logs; tests capture them directly. The broker never places
/// secret material in a record.
pub trait AuditSink: Send + Sync {
    /// Records one completed operation outcome.
    fn record(&self, record: &SecretAuditRecord);
}
