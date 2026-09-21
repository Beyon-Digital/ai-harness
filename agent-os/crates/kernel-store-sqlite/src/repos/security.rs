//! Security repository: capability grants, delegation hops, and approvals.
//!
//! Approval requests and their responses are append-only here; capability
//! grants and delegation hops are insert-then-read records. No update or
//! delete operation is exposed.

use async_trait::async_trait;
use domain::ids::{ApprovalRequestId, CapabilityGrantId, DelegationChainId, RunId};
use domain::run::UnknownStateValue;
use domain::security::ApprovalState;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{
    ApprovalRequestRow, ApprovalResponseRow, CapabilityGrantRow, DelegationHopRow,
    NewApprovalRequest, NewApprovalResponse, NewCapabilityGrant, NewDelegationHop,
};
use kernel_store::repositories::{SecurityRead, SecurityRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over the security tables.
pub(crate) struct SqliteSecurityRepo {
    conn: SharedConn,
}

impl SqliteSecurityRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

fn decode_grant(row: &SqliteRow) -> errors::Result<CapabilityGrantRow> {
    Ok(CapabilityGrantRow {
        grant_id: mapping::decode_id(
            "capability_grants.grant_id",
            &mapping::text(row, "grant_id")?,
        )?,
        principal_id: mapping::decode_id(
            "capability_grants.principal_id",
            &mapping::text(row, "principal_id")?,
        )?,
        actor_id: mapping::decode_id(
            "capability_grants.actor_id",
            &mapping::text(row, "actor_id")?,
        )?,
        run_id: mapping::decode_opt_id(
            "capability_grants.run_id",
            mapping::opt_text(row, "run_id")?,
        )?,
        capability_id: mapping::text(row, "capability_id")?,
        scope: mapping::blob(row, "scope")?,
        delegated_from_grant_id: mapping::decode_opt_id(
            "capability_grants.delegated_from_grant_id",
            mapping::opt_text(row, "delegated_from_grant_id")?,
        )?,
        expires_at_ms: mapping::opt_int(row, "expires_at_ms")?,
        revoked_at_ms: mapping::opt_int(row, "revoked_at_ms")?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
    })
}

fn decode_hop(row: &SqliteRow) -> errors::Result<DelegationHopRow> {
    let hop_index = mapping::int(row, "hop_index")?;
    Ok(DelegationHopRow {
        chain_id: mapping::decode_id("delegation_hops.chain_id", &mapping::text(row, "chain_id")?)?,
        hop_index: u32::try_from(hop_index).map_err(|_| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "delegation_hops.hop_index is outside the u32 range",
            )
        })?,
        principal_or_actor_id: mapping::text(row, "principal_or_actor_id")?,
        run_id: mapping::decode_opt_id(
            "delegation_hops.run_id",
            mapping::opt_text(row, "run_id")?,
        )?,
        capability_grant_ids: mapping::blob(row, "capability_grant_ids")?,
    })
}

fn decode_approval_request(row: &SqliteRow) -> errors::Result<ApprovalRequestRow> {
    Ok(ApprovalRequestRow {
        request_id: mapping::decode_id(
            "approval_requests.request_id",
            &mapping::text(row, "request_id")?,
        )?,
        request_digest: mapping::text(row, "request_digest")?,
        principal_id: mapping::decode_id(
            "approval_requests.principal_id",
            &mapping::text(row, "principal_id")?,
        )?,
        actor_id: mapping::decode_id(
            "approval_requests.actor_id",
            &mapping::text(row, "actor_id")?,
        )?,
        run_id: mapping::decode_opt_id(
            "approval_requests.run_id",
            mapping::opt_text(row, "run_id")?,
        )?,
        operation: mapping::text(row, "operation")?,
        target_resource: mapping::opt_text(row, "target_resource")?,
        capability_ids: mapping::blob(row, "capability_ids")?,
        extension_bundle_digest: mapping::opt_text(row, "extension_bundle_digest")?,
        config_generation_digest: mapping::opt_text(row, "config_generation_digest")?,
        expires_at_ms: mapping::int(row, "expires_at_ms")?,
        nonce: mapping::text(row, "nonce")?,
        state: mapping::decode_state(
            "approval_requests.state",
            &mapping::text(row, "state")?,
            ApprovalState::from_state_str,
        )?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
        resolved_at_ms: mapping::opt_int(row, "resolved_at_ms")?,
    })
}

/// Parses the exact `approval_responses.decision` CHECK literal set.
fn approval_decision_from_state(value: &str) -> Result<String, UnknownStateValue> {
    match value {
        "approve" | "deny" => Ok(value.to_owned()),
        _ => Err(UnknownStateValue {
            value: value.to_owned(),
            enum_name: "ApprovalDecision",
        }),
    }
}

fn decode_approval_response(row: &SqliteRow) -> errors::Result<ApprovalResponseRow> {
    Ok(ApprovalResponseRow {
        request_id: mapping::decode_id(
            "approval_responses.request_id",
            &mapping::text(row, "request_id")?,
        )?,
        request_digest: mapping::text(row, "request_digest")?,
        decision: mapping::decode_state(
            "approval_responses.decision",
            &mapping::text(row, "decision")?,
            approval_decision_from_state,
        )?,
        device_id: mapping::decode_id(
            "approval_responses.device_id",
            &mapping::text(row, "device_id")?,
        )?,
        responder_principal_id: mapping::decode_id(
            "approval_responses.responder_principal_id",
            &mapping::text(row, "responder_principal_id")?,
        )?,
        responded_at_ms: mapping::int(row, "responded_at_ms")?,
    })
}

#[async_trait]
impl SecurityRead for SqliteSecurityRepo {
    async fn get_grant(
        &mut self,
        id: CapabilityGrantId,
    ) -> errors::Result<Option<CapabilityGrantRow>> {
        let mut guard = self.conn.lock().await;
        let row = sqlx::query(
            "SELECT grant_id, principal_id, actor_id, run_id, capability_id, scope, \
             delegated_from_grant_id, expires_at_ms, revoked_at_ms, created_at_ms \
             FROM capability_grants WHERE grant_id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_grant).transpose()
    }

    async fn list_delegation_hops(
        &mut self,
        chain: DelegationChainId,
    ) -> errors::Result<Vec<DelegationHopRow>> {
        let mut guard = self.conn.lock().await;
        let rows = sqlx::query(
            "SELECT chain_id, hop_index, principal_or_actor_id, run_id, capability_grant_ids \
             FROM delegation_hops WHERE chain_id = ?1 ORDER BY hop_index",
        )
        .bind(chain.to_string())
        .fetch_all(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_hop).collect()
    }

    async fn get_approval_request(
        &mut self,
        id: ApprovalRequestId,
    ) -> errors::Result<Option<ApprovalRequestRow>> {
        let mut guard = self.conn.lock().await;
        let row = sqlx::query(
            "SELECT request_id, request_digest, principal_id, actor_id, run_id, operation, \
             target_resource, capability_ids, extension_bundle_digest, config_generation_digest, \
             expires_at_ms, nonce, state, created_at_ms, resolved_at_ms \
             FROM approval_requests WHERE request_id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_approval_request).transpose()
    }

    async fn list_approvals_by_run(
        &mut self,
        run_id: RunId,
    ) -> errors::Result<Vec<ApprovalRequestRow>> {
        let mut guard = self.conn.lock().await;
        let rows = sqlx::query(
            "SELECT request_id, request_digest, principal_id, actor_id, run_id, operation, \
             target_resource, capability_ids, extension_bundle_digest, config_generation_digest, \
             expires_at_ms, nonce, state, created_at_ms, resolved_at_ms \
             FROM approval_requests WHERE run_id = ?1 ORDER BY created_at_ms, request_id",
        )
        .bind(run_id.to_string())
        .fetch_all(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_approval_request).collect()
    }

    async fn list_approvals(&mut self) -> errors::Result<Vec<ApprovalRequestRow>> {
        let mut guard = self.conn.lock().await;
        let rows = sqlx::query(
            "SELECT request_id, request_digest, principal_id, actor_id, run_id, operation, \
             target_resource, capability_ids, extension_bundle_digest, config_generation_digest, \
             expires_at_ms, nonce, state, created_at_ms, resolved_at_ms \
             FROM approval_requests ORDER BY created_at_ms, request_id",
        )
        .fetch_all(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_approval_request).collect()
    }

    async fn list_approval_responses(
        &mut self,
        request: ApprovalRequestId,
    ) -> errors::Result<Vec<ApprovalResponseRow>> {
        let mut guard = self.conn.lock().await;
        let rows = sqlx::query(
            "SELECT request_id, request_digest, decision, device_id, responder_principal_id, \
             responded_at_ms FROM approval_responses WHERE request_id = ?1 \
             ORDER BY responded_at_ms",
        )
        .bind(request.to_string())
        .fetch_all(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        rows.iter().map(decode_approval_response).collect()
    }
}

#[async_trait]
impl SecurityRepo for SqliteSecurityRepo {
    async fn insert_grant(&mut self, grant: NewCapabilityGrant) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO capability_grants (grant_id, principal_id, actor_id, run_id, \
             capability_id, scope, delegated_from_grant_id, expires_at_ms, revoked_at_ms, \
             created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )
        .bind(grant.grant_id.to_string())
        .bind(grant.principal_id.to_string())
        .bind(grant.actor_id.to_string())
        .bind(grant.run_id.map(|id| id.to_string()))
        .bind(grant.capability_id)
        .bind(grant.scope)
        .bind(grant.delegated_from_grant_id.map(|id| id.to_string()))
        .bind(grant.expires_at_ms)
        .bind(grant.revoked_at_ms)
        .bind(grant.created_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn insert_delegation_hop(&mut self, hop: NewDelegationHop) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO delegation_hops (chain_id, hop_index, principal_or_actor_id, run_id, \
             capability_grant_ids) VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(hop.chain_id.to_string())
        .bind(i64::from(hop.hop_index))
        .bind(hop.principal_or_actor_id)
        .bind(hop.run_id.map(|id| id.to_string()))
        .bind(hop.capability_grant_ids)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn insert_approval_request(&mut self, request: NewApprovalRequest) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO approval_requests (request_id, request_digest, principal_id, actor_id, \
             run_id, operation, target_resource, capability_ids, extension_bundle_digest, \
             config_generation_digest, expires_at_ms, nonce, state, created_at_ms, resolved_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        )
        .bind(request.request_id.to_string())
        .bind(request.request_digest)
        .bind(request.principal_id.to_string())
        .bind(request.actor_id.to_string())
        .bind(request.run_id.map(|id| id.to_string()))
        .bind(request.operation)
        .bind(request.target_resource)
        .bind(request.capability_ids)
        .bind(request.extension_bundle_digest)
        .bind(request.config_generation_digest)
        .bind(request.expires_at_ms)
        .bind(request.nonce)
        .bind(request.state.as_str())
        .bind(request.created_at_ms)
        .bind(request.resolved_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn insert_approval_response(
        &mut self,
        response: NewApprovalResponse,
    ) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO approval_responses (request_id, request_digest, decision, device_id, \
             responder_principal_id, responded_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(response.request_id.to_string())
        .bind(response.request_digest)
        .bind(response.decision)
        .bind(response.device_id.to_string())
        .bind(response.responder_principal_id.to_string())
        .bind(response.responded_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }
}
