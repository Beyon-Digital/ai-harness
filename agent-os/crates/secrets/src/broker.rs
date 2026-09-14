//! The broker plus the documented in-memory test backend.
//!
//! [`SecretsBroker`] evaluates authorization with [`permissions::evaluate`]
//! before any store call; a denial or approval requirement releases no
//! material. [`InMemorySecretStore`] carries the backend contract tests and
//! behaves exactly like the Keychain backend for metadata and raw access.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use identity::delegation::{Capability, CapabilityAction, CapabilityFamily};
use permissions::{Decision, PermissionRequest, ScopedTarget, evaluate};
use sha2::{Digest, Sha256};

use crate::{
    ActResult, AuditSink, SecretAuditRecord, SecretMetadata, SecretStore, SecretUseOutcome,
    SecretUseRequest, SecretValue,
};

/// The raw-access capability a brokered read or metadata lookup requires.
const SECRET_USE: Capability = Capability::new(CapabilityFamily::Secret, CapabilityAction::Use);

/// The scoped-operation capability a sign-or-act call requires.
const SECRET_SIGN_OR_ACT: Capability =
    Capability::new(CapabilityFamily::Secret, CapabilityAction::SignOrAct);

/// Authorized, audited front door for every secret operation.
#[derive(Clone)]
pub struct SecretsBroker {
    store: Arc<dyn SecretStore>,
    clock: Arc<dyn Clock>,
    audit: Arc<dyn AuditSink>,
}

impl SecretsBroker {
    /// Builds a broker over one backend and clock that audits through the
    /// production default [`TracingAuditSink`].
    ///
    /// Every brokered outcome is then emitted through
    /// `observability::audit_record`, which records actor, run, target, and
    /// outcome only. Tests that capture records inject a sink with
    /// [`SecretsBroker::with_audit`].
    pub fn new(store: Arc<dyn SecretStore>, clock: Arc<dyn Clock>) -> Self {
        Self::with_audit(store, clock, Arc::new(TracingAuditSink))
    }

    /// Builds a broker with an injected audit sink.
    ///
    /// Tests use this form to capture records; production wiring uses
    /// [`SecretsBroker::new`] unless it routes records elsewhere.
    pub fn with_audit(
        store: Arc<dyn SecretStore>,
        clock: Arc<dyn Clock>,
        audit: Arc<dyn AuditSink>,
    ) -> Self {
        Self {
            store,
            clock,
            audit,
        }
    }

    /// Authorizes and returns metadata for the request's secret; no value is
    /// read on any decision.
    pub async fn resolve_metadata(
        &self,
        request: &SecretUseRequest,
    ) -> errors::Result<SecretMetadata> {
        self.authorize(request, SECRET_USE, "metadata")?;
        match self.store.metadata(&request.uri).await {
            Ok(metadata) => {
                self.record(request, SecretUseOutcome::Allowed);
                Ok(metadata)
            }
            Err(error) => {
                self.record(request, SecretUseOutcome::Failed);
                Err(error)
            }
        }
    }

    /// Authorizes and returns the raw value wrapped in a zeroizing container.
    pub async fn use_secret(&self, request: &SecretUseRequest) -> errors::Result<SecretValue> {
        self.authorize(request, SECRET_USE, "use")?;
        match self.store.get(&request.uri).await {
            Ok(value) => {
                self.record(request, SecretUseOutcome::Allowed);
                Ok(value)
            }
            Err(error) => {
                self.record(request, SecretUseOutcome::Failed);
                Err(error)
            }
        }
    }

    /// Authorizes and requests a scoped signature or action instead of raw
    /// material.
    pub async fn sign_or_act(
        &self,
        request: &SecretUseRequest,
        payload: &[u8],
    ) -> errors::Result<ActResult> {
        if request.operation.is_empty() {
            return Err(KernelError::new(
                ErrorCode::InvalidArgument,
                RetryClass::Never,
                "sign-or-act requires an operation",
            ));
        }
        self.authorize(request, SECRET_SIGN_OR_ACT, "sign_or_act")?;
        match self
            .store
            .sign_or_act(&request.uri, &request.operation, payload)
            .await
        {
            Ok(result) => {
                self.record(request, SecretUseOutcome::Acted);
                Ok(result)
            }
            Err(error) => {
                self.record(request, SecretUseOutcome::Failed);
                Err(error)
            }
        }
    }

    /// Evaluates one capability for the request, auditing and failing closed
    /// on every decision but `Allow`. Denial text carries only the stable
    /// reason token, never target or secret content.
    fn authorize(
        &self,
        request: &SecretUseRequest,
        capability: Capability,
        operation: &'static str,
    ) -> errors::Result<()> {
        match self.permission_decision(request, capability) {
            Decision::Allow { .. } => Ok(()),
            Decision::Deny { reason } => {
                self.record(request, SecretUseOutcome::Denied);
                Err(KernelError::new(
                    ErrorCode::FailedPrecondition,
                    RetryClass::Never,
                    format!("secret {operation} denied: {reason}"),
                ))
            }
            Decision::RequireApproval { .. } => {
                self.record(request, SecretUseOutcome::ApprovalRequired);
                Err(KernelError::new(
                    ErrorCode::Conflict,
                    RetryClass::Never,
                    format!("secret {operation} requires approval"),
                ))
            }
        }
    }

    /// Builds and evaluates the pure permission request for one capability.
    fn permission_decision(&self, request: &SecretUseRequest, capability: Capability) -> Decision {
        evaluate(&PermissionRequest {
            principal_id: request.principal_id,
            actor_id: request.actor_id,
            run_id: request.run_id,
            chain: request.chain.clone(),
            grant_scopes: request.grant_scopes.clone(),
            tool_capabilities: None,
            capability,
            target: ScopedTarget::Secret {
                uri: request.uri.clone(),
                egress: request.egress.clone(),
            },
            extension_digest: None,
            config_digest: None,
            now_ms: self.clock.now_unix_ms(),
        })
    }

    /// Reports one terminal outcome with actor, run, uri, and outcome only.
    fn record(&self, request: &SecretUseRequest, outcome: SecretUseOutcome) {
        self.audit.record(&SecretAuditRecord {
            actor_id: request.actor_id,
            run_id: request.run_id,
            uri: request.uri.clone(),
            outcome,
        });
    }
}

/// Production audit sink: emits each record through
/// [`observability::audit_record`] as a structured `audit.record` tracing
/// event.
///
/// The sink records actor, run, target URI, and outcome only; it never
/// receives the secret value, and the record is rendered through the
/// `observability` substrate.
#[derive(Clone, Copy, Debug, Default)]
pub struct TracingAuditSink;

impl AuditSink for TracingAuditSink {
    fn record(&self, record: &SecretAuditRecord) {
        let actor_id = record.actor_id.to_string();
        let run_id = record.run_id.map(|run| run.to_string());
        observability::audit_record(
            "secret",
            &actor_id,
            run_id.as_deref(),
            &record.uri,
            record.outcome.as_str(),
        );
    }
}

/// Audit sink that discards every record; used by tests and by callers that
/// wire their own sink explicitly.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullAuditSink;

impl AuditSink for NullAuditSink {
    fn record(&self, _record: &SecretAuditRecord) {}
}

/// One stored secret: metadata plus its zeroizing value.
struct Entry {
    metadata: SecretMetadata,
    value: SecretValue,
}

/// Documented in-memory backend carrying the [`SecretStore`] contract tests.
///
/// Values zeroize when the store drops. `sign_or_act` returns a deterministic
/// scoped reference derived from the stored value, so callers never receive
/// material.
#[derive(Default)]
pub struct InMemorySecretStore {
    entries: Mutex<BTreeMap<String, Entry>>,
}

impl InMemorySecretStore {
    /// Creates an empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or replaces the entry for `metadata.uri`.
    pub fn insert(&self, metadata: SecretMetadata, value: impl Into<Vec<u8>>) {
        let mut entries = self.lock();
        entries.insert(
            metadata.uri.clone(),
            Entry {
                metadata,
                value: SecretValue::new(value),
            },
        );
    }

    fn lock(&self) -> MutexGuard<'_, BTreeMap<String, Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[async_trait]
impl SecretStore for InMemorySecretStore {
    async fn metadata(&self, uri: &str) -> errors::Result<SecretMetadata> {
        self.lock()
            .get(uri)
            .map(|entry| entry.metadata.clone())
            .ok_or_else(metadata_missing)
    }

    async fn get(&self, uri: &str) -> errors::Result<SecretValue> {
        self.lock()
            .get(uri)
            .map(|entry| SecretValue::new(entry.value.expose().to_vec()))
            .ok_or_else(value_missing)
    }

    async fn sign_or_act(
        &self,
        uri: &str,
        action: &str,
        payload: &[u8],
    ) -> errors::Result<ActResult> {
        let reference = {
            let entries = self.lock();
            let entry = entries.get(uri).ok_or_else(value_missing)?;
            scoped_reference(uri, action, payload, entry.value.expose())
        };
        Ok(ActResult { reference })
    }
}

/// Builds the stable missing-value error without echoing the uri.
fn value_missing() -> KernelError {
    KernelError::new(ErrorCode::NotFound, RetryClass::Never, "secret not found")
}

/// Builds the stable missing-metadata error without echoing the uri.
fn metadata_missing() -> KernelError {
    KernelError::new(
        ErrorCode::NotFound,
        RetryClass::Never,
        "secret metadata not found",
    )
}

/// Derives a deterministic scoped reference over the operation and the stored
/// value; the value never leaves the backend.
fn scoped_reference(uri: &str, action: &str, payload: &[u8], secret: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"agentd-secrets-sign-or-act-v1");
    hasher.update(uri.as_bytes());
    hasher.update(b"\0");
    hasher.update(action.as_bytes());
    hasher.update(b"\0");
    hasher.update((payload.len() as u64).to_be_bytes());
    hasher.update(payload);
    hasher.update(secret);
    format!("scoped:v1:{}", hex_lower(&hasher.finalize()))
}

/// Renders bytes as lowercase hexadecimal.
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 0x0f) as usize] as char);
    }
    text
}
