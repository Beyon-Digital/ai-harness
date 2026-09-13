//! Immutable agent spec revisions: the storage service and its command handler.

use async_trait::async_trait;
use command_coordinator::handler::{CommandContext, CommandHandler, CommandOutcome, OutcomeCode};
use domain::generated::contract;
use domain::ids::{AgentSpecId, EventId};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use events::StreamKey;
use kernel_store::KernelTxn;
use kernel_store::models::NewAgentSpec;
use prost::Message;

use crate::{RuntimeDeps, decode_contract, optional_id, stage_catalogued};

/// Stores an immutable `(id, version)` revision.
///
/// Re-insertion of the identical digest and body is idempotent; a differing
/// body or digest conflicts and leaves the stored revision untouched.
pub async fn put_agent_spec_revision(
    txn: &mut dyn KernelTxn,
    spec_id: Option<AgentSpecId>,
    version: String,
    body: Vec<u8>,
    body_digest: String,
    now_ms: i64,
) -> errors::Result<()> {
    let spec_id = spec_id
        .unwrap_or_else(|| AgentSpecId::from_uuid_v7(*txn.context().command_id.as_uuid_v7()));
    if version.trim().is_empty() {
        return Err(KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            "agent spec version is required",
        ));
    }
    txn.agent_specs()
        .insert(NewAgentSpec {
            agent_spec_id: spec_id,
            version,
            digest: body_digest,
            body,
            created_at_ms: now_ms,
        })
        .await
}

/// Handler for `agentos.spec.v1.PutAgentSpecRevision`.
pub struct PutAgentSpecRevisionHandler {
    deps: RuntimeDeps,
}

impl PutAgentSpecRevisionHandler {
    /// Creates the handler with its runtime dependencies.
    pub fn new(deps: RuntimeDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl CommandHandler for PutAgentSpecRevisionHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw =
            decode_contract::<contract::PutAgentSpecRevision>("PutAgentSpecRevision", &payload)?;
        let spec_id = optional_id::<AgentSpecId>("agent_spec_id", &raw.agent_spec_id)?;
        let spec_id =
            spec_id.unwrap_or_else(|| AgentSpecId::from_uuid_v7(*ctx.command_id.as_uuid_v7()));
        let now_ms = self.deps.now_unix_ms();
        let already_stored = match txn.agent_specs().get(spec_id, &raw.version).await? {
            Some(stored) => stored.digest == raw.body_digest && stored.body == raw.body_bytes,
            None => false,
        };
        if !already_stored {
            put_agent_spec_revision(
                txn,
                Some(spec_id),
                raw.version.clone(),
                raw.body_bytes.clone(),
                raw.body_digest.clone(),
                now_ms,
            )
            .await?;
            let event = contract::AgentSpecRevision {
                agent_spec_id: spec_id.to_string(),
                version: raw.version,
                digest: raw.body_digest,
                body: raw.body_bytes,
                created_unix_ms: now_ms,
            };
            stage_catalogued(
                txn,
                EventId::new(self.deps.ids.as_ref()),
                "AgentSpecRevisionStored",
                StreamKey::config_global(),
                event.encode_to_vec(),
                ctx.correlation_id.clone(),
                None,
            )
            .await?;
        }
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: spec_id.to_string().into_bytes(),
        })
    }
}
