//! Immutable approval flow: canonical digest binding, exactly one terminal
//! response, and resolution from persisted records (R3.1-R3.6, P3).
//!
//! [`create_request`] persists an immutable request inside the caller's
//! transaction and stages the catalogued `ApprovalRequested` event;
//! [`respond`] validates the echoed request digest, expiry, resolution state,
//! and responder identity before persisting the single response row; and
//! [`is_satisfied`] answers a blocked command from the persisted request plus
//! response, never from a boolean alone.
#![forbid(unsafe_code)]

use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use command_coordinator::handler::{
    CommandContext, CommandHandler, CommandOutcome, CommandRegistry, OutcomeCode,
};
use domain::generated::contract;
use domain::ids::{
    ActorId, ApprovalRequestId, DeviceId, EventId, EventStreamKey, PrincipalId, RunId,
};
use domain::security::ApprovalState;
use domain::time::Clock;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::StreamKey;
use events::outbox::{DraftEvent, stage};
use events::{CatalogClassificationPolicy, ClassificationPolicy};
use identity::delegation::Capability;
use kernel_store::KernelTxn;
use kernel_store::models::{ApprovalRequestRow, NewApprovalRequest, NewApprovalResponse};
use prost::Message;
use sha2::{Digest, Sha256};

/// Fully-qualified command type of `CreateApprovalRequest`.
pub const CMD_CREATE_APPROVAL_REQUEST: &str = "agentos.spec.v1.CreateApprovalRequest";
/// Fully-qualified command type of `RespondApproval`.
pub const CMD_RESPOND_APPROVAL: &str = "agentos.spec.v1.RespondApproval";

/// Catalogue event staged when an approval request is persisted.
const EVENT_APPROVAL_REQUESTED: &str = "ApprovalRequested";

/// Dependencies shared by the approval command handlers.
#[derive(Clone)]
pub struct ApprovalDeps {
    /// Wall-clock source for persisted timestamps.
    pub clock: Arc<dyn Clock>,
}

/// SHA-256 digest binding an approval to its canonical content.
///
/// Rendered as 64 lowercase hexadecimal characters; parsing accepts only the
/// canonical lowercase form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ApprovalDigest([u8; 32]);

impl FromStr for ApprovalDigest {
    type Err = KernelError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() != 64 {
            return Err(malformed_digest());
        }
        let mut bytes = [0u8; 32];
        for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
            let high = hex_nibble(pair[0]).ok_or_else(malformed_digest)?;
            let low = hex_nibble(pair[1]).ok_or_else(malformed_digest)?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for ApprovalDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// The exact content an approval digest binds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DigestInput {
    /// Identifier of the request, once assigned.
    pub request_id: Option<ApprovalRequestId>,
    /// Principal on whose behalf the operation runs.
    pub principal_id: PrincipalId,
    /// Actor that requested the approval.
    pub actor_id: ActorId,
    /// Run context when the operation belongs to a run.
    pub run_id: Option<RunId>,
    /// Canonical `family.action` operation token.
    pub operation: String,
    /// Canonical target token; identifiers and destinations only.
    pub target: String,
    /// Exercised capabilities; canonicalized before hashing.
    pub capabilities: Vec<Capability>,
    /// Extension bundle digest when the operation is extension-bound.
    pub extension_digest: Option<String>,
    /// Configuration generation digest when the operation is config-bound.
    pub config_digest: Option<String>,
    /// Expiry of the requested approval.
    pub expiry_ms: i64,
    /// Nonce the request binds; `create_request` assigns the canonical value.
    pub nonce: String,
}

/// Computes the canonical digest over `input`.
///
/// Framing is unambiguous and big-endian: fields are appended in declaration
/// order; each optional field is a one-byte presence marker followed by its
/// length-prefixed text when present; each string is a `u32` byte length plus
/// UTF-8 bytes; the capability list is a `u32` count followed by each
/// `family.action` token; and `expiry_ms` is a signed 64-bit big-endian
/// integer. Capabilities are sorted by family then action and deduplicated
/// before hashing, so any permutation of the same set binds the same digest.
/// The result is SHA-256 rendered as 64 lowercase hexadecimal characters.
pub fn canonical_digest(input: &DigestInput) -> ApprovalDigest {
    let mut buffer = Vec::new();
    let request_id = input.request_id.map(|id| id.to_string());
    put_optional(&mut buffer, request_id.as_deref());
    put_str(&mut buffer, &input.principal_id.to_string());
    put_str(&mut buffer, &input.actor_id.to_string());
    let run_id = input.run_id.map(|id| id.to_string());
    put_optional(&mut buffer, run_id.as_deref());
    put_str(&mut buffer, &input.operation);
    put_str(&mut buffer, &input.target);
    let mut capabilities = input.capabilities.clone();
    capabilities.sort();
    capabilities.dedup();
    buffer.extend_from_slice(&(capabilities.len() as u32).to_be_bytes());
    for capability in &capabilities {
        put_str(&mut buffer, &capability.to_string());
    }
    put_optional(&mut buffer, input.extension_digest.as_deref());
    put_optional(&mut buffer, input.config_digest.as_deref());
    buffer.extend_from_slice(&input.expiry_ms.to_be_bytes());
    put_str(&mut buffer, &input.nonce);

    let hashed = Sha256::digest(&buffer);
    let mut bytes = [0u8; 32];
    bytes.copy_from_slice(&hashed);
    ApprovalDigest(bytes)
}

/// Appends a `u32` length-prefixed UTF-8 string.
fn put_str(buffer: &mut Vec<u8>, text: &str) {
    buffer.extend_from_slice(&(text.len() as u32).to_be_bytes());
    buffer.extend_from_slice(text.as_bytes());
}

/// Appends a presence marker and, when present, the length-prefixed string.
fn put_optional(buffer: &mut Vec<u8>, text: Option<&str>) {
    match text {
        Some(text) => {
            buffer.push(1);
            put_str(buffer, text);
        }
        None => buffer.push(0),
    }
}

fn malformed_digest() -> KernelError {
    KernelError::new(
        ErrorCode::InvalidArgument,
        RetryClass::Never,
        "approval_digest must be 64 lowercase hexadecimal characters",
    )
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Persists an immutable approval request and stages `ApprovalRequested`.
///
/// The request id defaults to the command identity, the nonce is assigned
/// from the request id, and capabilities are canonicalized before the digest
/// is computed. The caller's transaction owns atomicity: the request row and
/// the outbox event commit together or not at all.
pub async fn create_request(
    txn: &mut dyn KernelTxn,
    input: DigestInput,
    now_ms: i64,
) -> errors::Result<(ApprovalRequestId, ApprovalDigest)> {
    let request_id = input
        .request_id
        .unwrap_or_else(|| ApprovalRequestId::from_uuid_v7(*txn.context().command_id.as_uuid_v7()));
    if input.operation.trim().is_empty() {
        return Err(invalid("approval request operation must not be empty"));
    }
    for capability in &input.capabilities {
        if !capability.is_supported() {
            return Err(invalid(
                "approval request cites a capability outside the catalogue",
            ));
        }
    }
    let mut input = input;
    input.request_id = Some(request_id);
    input.capabilities.sort();
    input.capabilities.dedup();
    input.nonce = request_id.to_string();
    let digest = canonical_digest(&input);
    let target_resource = if input.target.is_empty() {
        None
    } else {
        Some(input.target.clone())
    };

    let event = contract::ApprovalRequest {
        request_id: request_id.to_string(),
        request_digest: digest.to_string(),
        actor_id: input.actor_id.to_string(),
        run_id: input.run_id.map(|id| id.to_string()).unwrap_or_default(),
        capability_ids: input.capabilities.iter().map(ToString::to_string).collect(),
        target_resource: input.target.clone(),
        operation: input.operation.clone(),
        extension_bundle_digest: input.extension_digest.clone().unwrap_or_default(),
        config_generation_digest: input.config_digest.clone().unwrap_or_default(),
        expires_unix_ms: input.expiry_ms,
        nonce: input.nonce.clone(),
    };

    txn.security()
        .insert_approval_request(NewApprovalRequest {
            request_id,
            request_digest: digest.to_string(),
            principal_id: input.principal_id,
            actor_id: input.actor_id,
            run_id: input.run_id,
            operation: input.operation,
            target_resource,
            capability_ids: encode_capabilities(&input.capabilities),
            extension_bundle_digest: input.extension_digest,
            config_generation_digest: input.config_digest,
            expires_at_ms: input.expiry_ms,
            nonce: input.nonce,
            state: ApprovalState::Pending,
            created_at_ms: now_ms,
            resolved_at_ms: None,
        })
        .await?;
    stage_approval_requested(txn, request_id, input.principal_id, &event).await?;
    Ok((request_id, digest))
}

/// Stages the catalogued `ApprovalRequested` event on the principal stream.
async fn stage_approval_requested(
    txn: &mut dyn KernelTxn,
    request_id: ApprovalRequestId,
    principal_id: PrincipalId,
    event: &contract::ApprovalRequest,
) -> errors::Result<()> {
    let policy = CatalogClassificationPolicy::embedded()?;
    let sensitivity = policy
        .minimum(EVENT_APPROVAL_REQUESTED)
        .ok_or_else(uncatalogued)?;
    let retention = policy
        .default_retention(EVENT_APPROVAL_REQUESTED)
        .ok_or_else(uncatalogued)?;
    let stream_key = EventStreamKey::new(StreamKey::principal(principal_id).as_str().to_owned())
        .map_err(|_| internal("catalogued stream key is not canonical"))?;
    let correlation_id = txn.context().correlation_id.clone();
    stage(
        txn,
        DraftEvent {
            event_id: EventId::from_uuid_v7(*request_id.as_uuid_v7()),
            stream_key,
            event_type: EVENT_APPROVAL_REQUESTED.to_owned(),
            payload: event.encode_to_vec(),
            sensitivity,
            retention,
            correlation_id,
            causation_id: None,
        },
    )
    .await?;
    Ok(())
}

/// Persists the single terminal response for `request_id`.
///
/// The echoed digest must match the persisted request, the request must be
/// unexpired and unresolved, and the responder must be the approval principal
/// with its device identity present. Every rejection persists nothing.
pub async fn respond(
    txn: &mut dyn KernelTxn,
    request_id: ApprovalRequestId,
    digest: &ApprovalDigest,
    decision: ApprovalDecision,
    responder: Responder,
    now_ms: i64,
) -> errors::Result<()> {
    let row = txn
        .security()
        .get_approval_request(request_id)
        .await?
        .ok_or_else(|| not_found("approval request does not exist"))?;
    let persisted = recompute_digest(&row)?;
    if persisted.to_string() != row.request_digest {
        return Err(internal(
            "approval request digest does not match its persisted fields",
        ));
    }
    if digest != &persisted {
        return Err(conflict(
            "approval request digest does not match the persisted request",
        ));
    }
    if now_ms >= row.expires_at_ms {
        return Err(precondition("approval request has expired"));
    }
    if !txn
        .security()
        .list_approval_responses(request_id)
        .await?
        .is_empty()
    {
        return Err(conflict("approval request already has a terminal response"));
    }
    if responder.principal_id != row.principal_id {
        return Err(precondition("responder is not the approval principal"));
    }
    let device_id = responder
        .device_id
        .ok_or_else(|| invalid("responder device identity is required"))?;
    txn.security()
        .insert_approval_response(NewApprovalResponse {
            request_id,
            request_digest: persisted.to_string(),
            decision: decision.as_str().to_owned(),
            device_id,
            responder_principal_id: responder.principal_id,
            responded_at_ms: now_ms,
        })
        .await?;
    Ok(())
}

/// One terminal decision for an approval request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// The request is approved.
    Approve,
    /// The request is denied.
    Deny,
}

impl ApprovalDecision {
    /// Returns the stable persisted token.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Deny => "deny",
        }
    }

    /// Decodes a persisted decision token, failing closed on unknown values.
    pub fn from_token(token: &str) -> errors::Result<Self> {
        match token {
            "approve" => Ok(Self::Approve),
            "deny" => Ok(Self::Deny),
            _ => Err(invalid("approval decision is not recognized")),
        }
    }
}

/// Identity that responded to an approval request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Responder {
    /// Principal that responded; must be the approval principal.
    pub principal_id: PrincipalId,
    /// Device the response came from; the persisted record requires it.
    pub device_id: Option<DeviceId>,
}

/// Resolution of an approval request derived from its persisted records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalOutcome {
    /// A terminal `approve` response exists for the expected digest.
    Approved,
    /// A terminal `deny` response exists for the expected digest.
    Denied,
    /// No response exists and the request has not expired.
    Pending,
    /// No response exists and the request has expired.
    Expired,
    /// The request exists but binds a digest other than `expected`, so a prior
    /// approval no longer covers the content being checked.
    Invalidated,
    /// No request with this identifier was persisted.
    Unknown,
}

/// Resolves `request_id` from its request and response rows.
///
/// The answer is `Unknown` without a request row and `Invalidated` when the
/// persisted digest differs from `expected` (for example after an extension or
/// config digest change). A terminal response wins over expiry because the
/// response was validated while the request was live.
pub async fn is_satisfied(
    txn: &mut dyn KernelTxn,
    request_id: ApprovalRequestId,
    expected: &ApprovalDigest,
    now_ms: i64,
) -> errors::Result<ApprovalOutcome> {
    let Some(row) = txn.security().get_approval_request(request_id).await? else {
        return Ok(ApprovalOutcome::Unknown);
    };
    let persisted = recompute_digest(&row)?;
    if persisted.to_string() != row.request_digest {
        return Err(internal(
            "approval request digest does not match its persisted fields",
        ));
    }
    let responses = txn.security().list_approval_responses(request_id).await?;
    if expected != &persisted {
        return Ok(ApprovalOutcome::Invalidated);
    }
    if let Some(response) = responses.first() {
        if response.request_digest != persisted.to_string() {
            return Ok(ApprovalOutcome::Invalidated);
        }
        return match response.decision.as_str() {
            "approve" => Ok(ApprovalOutcome::Approved),
            "deny" => Ok(ApprovalOutcome::Denied),
            _ => Err(internal("approval response decision is not recognized")),
        };
    }
    if now_ms >= row.expires_at_ms {
        return Ok(ApprovalOutcome::Expired);
    }
    Ok(ApprovalOutcome::Pending)
}

/// Recomputes the canonical digest from a persisted request row.
fn recompute_digest(row: &ApprovalRequestRow) -> errors::Result<ApprovalDigest> {
    Ok(canonical_digest(&DigestInput {
        request_id: Some(row.request_id),
        principal_id: row.principal_id,
        actor_id: row.actor_id,
        run_id: row.run_id,
        operation: row.operation.clone(),
        target: row.target_resource.clone().unwrap_or_default(),
        capabilities: decode_capabilities(&row.capability_ids)?,
        extension_digest: row.extension_bundle_digest.clone(),
        config_digest: row.config_generation_digest.clone(),
        expiry_ms: row.expires_at_ms,
        nonce: row.nonce.clone(),
    }))
}

/// Encodes canonical capability tokens as newline-joined UTF-8 bytes.
fn encode_capabilities(capabilities: &[Capability]) -> Vec<u8> {
    let tokens: Vec<String> = capabilities.iter().map(ToString::to_string).collect();
    tokens.join("\n").into_bytes()
}

/// Decodes the persisted capability blob, failing closed on tampering.
fn decode_capabilities(blob: &[u8]) -> errors::Result<Vec<Capability>> {
    if blob.is_empty() {
        return Ok(Vec::new());
    }
    let text = std::str::from_utf8(blob)
        .map_err(|_| internal("approval request capability list is not canonical text"))?;
    text.split('\n')
        .map(|token| {
            Capability::from_str(token)
                .map_err(|_| internal("approval request capability list is not canonical"))
        })
        .collect()
}

/// Command handler for `agentos.spec.v1.CreateApprovalRequest`.
pub struct CreateApprovalRequestHandler {
    deps: ApprovalDeps,
}

impl CreateApprovalRequestHandler {
    /// Creates the handler with its dependencies.
    pub fn new(deps: ApprovalDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl CommandHandler for CreateApprovalRequestHandler {
    async fn handle(
        &self,
        _ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw =
            decode_contract::<contract::CreateApprovalRequest>("CreateApprovalRequest", &payload)?;
        let input = DigestInput {
            request_id: optional_id::<ApprovalRequestId>("request_id", &raw.request_id)?,
            principal_id: required_id::<PrincipalId>("principal_id", &raw.principal_id)?,
            actor_id: required_id::<ActorId>("actor_id", &raw.actor_id)?,
            run_id: optional_id::<RunId>("run_id", &raw.run_id)?,
            operation: raw.operation,
            target: raw.target_resource,
            capabilities: parse_capabilities(&raw.capability_ids)?,
            extension_digest: optional_text(raw.extension_bundle_digest),
            config_digest: optional_text(raw.config_generation_digest),
            expiry_ms: raw.expires_at_ms,
            nonce: raw.nonce,
        };
        let (request_id, _digest) =
            create_request(txn, input, self.deps.clock.now_unix_ms()).await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: request_id.to_string().into_bytes(),
        })
    }
}

/// Command handler for `agentos.spec.v1.RespondApproval`.
pub struct RespondApprovalHandler {
    deps: ApprovalDeps,
}

impl RespondApprovalHandler {
    /// Creates the handler with its dependencies.
    pub fn new(deps: ApprovalDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl CommandHandler for RespondApprovalHandler {
    async fn handle(
        &self,
        _ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw = decode_contract::<contract::RespondApproval>("RespondApproval", &payload)?;
        let request_id = required_id::<ApprovalRequestId>("request_id", &raw.request_id)?;
        let digest = ApprovalDigest::from_str(&raw.request_digest)?;
        let decision = ApprovalDecision::from_token(&raw.decision)?;
        let responder = Responder {
            principal_id: required_id::<PrincipalId>(
                "responder_principal_id",
                &raw.responder_principal_id,
            )?,
            device_id: optional_id::<DeviceId>("device_id", &raw.device_id)?,
        };
        respond(
            txn,
            request_id,
            &digest,
            decision,
            responder,
            self.deps.clock.now_unix_ms(),
        )
        .await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: Vec::new(),
        })
    }
}

/// Registers the approval handlers with their canonical command types.
pub fn register_handlers(registry: &mut CommandRegistry, deps: ApprovalDeps) -> errors::Result<()> {
    registry.register(
        CMD_CREATE_APPROVAL_REQUEST,
        Arc::new(CreateApprovalRequestHandler::new(deps.clone())),
    )?;
    registry.register(
        CMD_RESPOND_APPROVAL,
        Arc::new(RespondApprovalHandler::new(deps)),
    )?;
    Ok(())
}

/// Decodes a prost contract message without echoing the payload bytes.
fn decode_contract<M>(name: &'static str, payload: &[u8]) -> errors::Result<M>
where
    M: prost::Message + Default,
{
    M::decode(payload).map_err(|source| {
        KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            format!("{name} payload is not valid contract bytes"),
        )
        .with_source(source)
    })
}

/// Parses a required identifier field without echoing its value.
fn required_id<T>(field: &'static str, text: &str) -> errors::Result<T>
where
    T: FromStr,
{
    text.parse()
        .map_err(|_| invalid_field(field, "is not a valid identifier"))
}

/// Parses an optional identifier field; an empty string means absent.
fn optional_id<T>(field: &'static str, text: &str) -> errors::Result<Option<T>>
where
    T: FromStr,
{
    if text.is_empty() {
        return Ok(None);
    }
    required_id(field, text).map(Some)
}

/// Treats an empty contract string as an absent optional value.
fn optional_text(text: String) -> Option<String> {
    if text.is_empty() { None } else { Some(text) }
}

/// Parses every requested capability token without echoing its value.
fn parse_capabilities(tokens: &[String]) -> errors::Result<Vec<Capability>> {
    tokens
        .iter()
        .map(|token| {
            Capability::from_str(token)
                .map_err(|_| invalid("approval request cites a capability outside the catalogue"))
        })
        .collect()
}

fn invalid_field(field: &'static str, detail: &'static str) -> KernelError {
    KernelError::new(
        ErrorCode::InvalidArgument,
        RetryClass::Never,
        format!("{field} {detail}"),
    )
}

fn uncatalogued() -> KernelError {
    KernelError::new(
        ErrorCode::Internal,
        RetryClass::Never,
        "ApprovalRequested has no catalog entry",
    )
}

fn invalid(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::InvalidArgument, RetryClass::Never, message)
}

fn not_found(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::NotFound, RetryClass::Never, message)
}

fn conflict(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::Conflict, RetryClass::Never, message)
}

fn precondition(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::FailedPrecondition, RetryClass::Never, message)
}

fn internal(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::Internal, RetryClass::Never, message)
}
