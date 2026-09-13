//! Session creation: the transaction-scoped service and its command handler.

use async_trait::async_trait;
use command_coordinator::handler::{CommandContext, CommandHandler, CommandOutcome, OutcomeCode};
use domain::generated::contract;
use domain::ids::{EventId, PrincipalId, SessionId};
use events::StreamKey;
use kernel_store::KernelTxn;
use kernel_store::models::NewSession;
use prost::Message;

use crate::{RuntimeDeps, decode_contract, optional_id, stage_catalogued};

/// Inserts a session bound to `principal` and returns its identifier.
///
/// A `None` session id is derived from the command identity, so a caller that
/// does not choose one still gets a unique, replay-stable identifier.
pub async fn create_session(
    txn: &mut dyn KernelTxn,
    session_id: Option<SessionId>,
    principal: PrincipalId,
    now_ms: i64,
) -> errors::Result<SessionId> {
    let session_id = session_id
        .unwrap_or_else(|| SessionId::from_uuid_v7(*txn.context().command_id.as_uuid_v7()));
    txn.sessions()
        .insert(NewSession {
            session_id,
            principal_id: principal,
            created_at_ms: now_ms,
            metadata: None,
        })
        .await?;
    Ok(session_id)
}

/// Handler for `agentos.spec.v1.CreateSession`.
pub struct CreateSessionHandler {
    deps: RuntimeDeps,
}

impl CreateSessionHandler {
    /// Creates the handler with its runtime dependencies.
    pub fn new(deps: RuntimeDeps) -> Self {
        Self { deps }
    }
}

#[async_trait]
impl CommandHandler for CreateSessionHandler {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn KernelTxn,
        payload: Vec<u8>,
    ) -> errors::Result<CommandOutcome> {
        let raw = decode_contract::<contract::CreateSession>("CreateSession", &payload)?;
        let session_id = optional_id::<SessionId>("session_id", &raw.session_id)?;
        let now_ms = self.deps.now_unix_ms();
        let session_id = create_session(txn, session_id, ctx.principal_id, now_ms).await?;
        let event = contract::Session {
            session_id: session_id.to_string(),
            principal_id: ctx.principal_id.to_string(),
            metadata: raw.metadata,
            created_unix_ms: now_ms,
        };
        stage_catalogued(
            txn,
            EventId::new(self.deps.ids.as_ref()),
            "SessionCreated",
            StreamKey::session(session_id),
            event.encode_to_vec(),
            ctx.correlation_id.clone(),
            None,
        )
        .await?;
        Ok(CommandOutcome {
            code: OutcomeCode::Ok,
            payload: session_id.to_string().into_bytes(),
        })
    }
}
