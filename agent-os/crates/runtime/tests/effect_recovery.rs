//! `ResolveUnknownEffect` acceptance tests (EFF-004): capability gate,
//! approval compensation, expected-state fencing, and the never-auto-retry
//! guarantee for `Unknown` effects.

use std::str::FromStr;
use std::sync::Arc;

use command_coordinator::envelope::{CommandEnvelope, RequestDigest};
use command_coordinator::handler::{CommandRegistry, OutcomeCode};
use command_coordinator::{CommandCoordinator, FixedFence};
use domain::effect::EffectState;
use domain::generated::contract;
use domain::ids::{
    ActorId, AdapterId, ApprovalRequestId, CapabilityGrantId, CommandId, DaemonInstanceId,
    DecisionId, DelegationChainId, EffectId, EventCursor, EventStreamKey, IdempotencyKey,
    PrincipalId, RunId, TaskId,
};
use domain::run::{RecoveryDisposition, RunState};
use domain::security::ApprovalState;
use errors::codes::ErrorCode;
use identity::delegation::{Capability, CapabilityAction, CapabilityFamily, Hop, persist_hop};
use kernel_store::models::{NewApprovalRequest, NewCapabilityGrant, NewRun, NewTask};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use prost::Message;
use testkit::clock::TestClock;
use testkit::faults::ArmedFaults;
use testkit::ids::DeterministicIds;

const SEED_MS: i64 = 1_700_000_000_000;
const RESOLVE_UNKNOWN: Capability =
    Capability::new(CapabilityFamily::Effect, CapabilityAction::ResolveUnknown);
const OTHER_GRANT: Capability =
    Capability::new(CapabilityFamily::Workspace, CapabilityAction::Read);

struct Harness {
    _dir: tempfile::TempDir,
    store: Arc<SqliteKernelStore>,
    coordinator: CommandCoordinator,
    ids: Arc<DeterministicIds>,
    clock: Arc<TestClock>,
    principal: PrincipalId,
    actor: ActorId,
    epoch: u64,
}

impl Harness {
    async fn new() -> Self {
        let dir = tempfile::tempdir().expect("temp root");
        let store = Arc::new(
            SqliteKernelStore::open(StoreConfig {
                path: dir.path().join("kernel.db"),
                pool_max_connections: 8,
                busy_timeout_ms: 30_000,
            })
            .await
            .expect("store opens"),
        );
        let ids = Arc::new(DeterministicIds::new(SEED_MS));
        let fence = store
            .acquire_daemon_fence(DaemonInstanceId::new(ids.as_ref()))
            .await
            .expect("daemon fence acquired");
        let clock = Arc::new(TestClock::new(SEED_MS));
        let mut registry = CommandRegistry::new();
        runtime::register_handlers(
            &mut registry,
            runtime::RuntimeDeps {
                clock: clock.clone(),
                ids: ids.clone(),
            },
        )
        .expect("runtime handlers register");
        let coordinator = CommandCoordinator::new(
            store.clone(),
            Arc::new(registry),
            Arc::new(FixedFence(fence.epoch.0)),
            clock.clone(),
            Arc::new(ArmedFaults::new()),
        );
        let principal = PrincipalId::new(ids.as_ref());
        let actor = ActorId::new(ids.as_ref());
        Self {
            _dir: dir,
            store,
            coordinator,
            ids,
            clock,
            principal,
            actor,
            epoch: fence.epoch.0,
        }
    }

    fn context(&self) -> TxContext {
        TxContext {
            daemon_epoch: self.epoch,
            principal_id: self.principal,
            command_id: CommandId::new(self.ids.as_ref()),
            correlation_id: None,
        }
    }

    async fn write(&self) -> Box<dyn KernelTxn + '_> {
        self.store
            .begin_write(self.context())
            .await
            .expect("write transaction opens")
    }

    async fn seed_run(&self) -> RunId {
        let task_id = TaskId::new(self.ids.as_ref());
        let run_id = RunId::new(self.ids.as_ref());
        let cursor = EventCursor::new(
            EventStreamKey::new(format!("run/{run_id}")).expect("canonical stream key"),
            0,
        );
        let mut txn = self.write().await;
        txn.tasks()
            .insert(NewTask {
                task_id,
                session_id: None,
                created_by_actor_id: self.actor,
                task_kind: "resolve".to_owned(),
                payload: Vec::new(),
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed task");
        txn.runs()
            .insert(NewRun {
                run_id,
                task_id,
                session_id: None,
                parent_run_id: None,
                state: RunState::Running,
                recovery: RecoveryDisposition::Normal,
                loop_epoch: 0,
                step_sequence: 0,
                input_event_cursor: cursor,
                cancellation_epoch: 0,
                resolved_environment_id: None,
                created_at_ms: SEED_MS,
            })
            .await
            .expect("seed run");
        txn.commit().await.expect("seed run commits");
        run_id
    }

    /// Seeds a chain whose single hop holds exactly `capability`.
    async fn seed_chain(&self, capability: Capability) -> DelegationChainId {
        let chain_id = DelegationChainId::new(self.ids.as_ref());
        let grant_id = CapabilityGrantId::new(self.ids.as_ref());
        let mut txn = self.write().await;
        txn.security()
            .insert_grant(NewCapabilityGrant {
                grant_id,
                principal_id: self.principal,
                actor_id: self.actor,
                run_id: None,
                capability_id: capability.to_string(),
                scope: Vec::new(),
                delegated_from_grant_id: None,
                expires_at_ms: None,
                revoked_at_ms: None,
                created_at_ms: SEED_MS,
            })
            .await
            .expect("grant inserts");
        persist_hop(
            txn.as_mut(),
            chain_id,
            Hop {
                hop_index: 0,
                actor_id: self.actor,
                run_id: None,
                grant_ids: vec![grant_id],
                capabilities: vec![capability],
            },
        )
        .await
        .expect("hop persists");
        txn.commit().await.expect("chain commits");
        chain_id
    }

    fn env(&self) -> effects::EffectEnv<'_> {
        effects::EffectEnv {
            ids: self.ids.as_ref(),
            clock: self.clock.as_ref(),
            correlation_id: None,
            causation_id: None,
        }
    }

    /// Seeds an `Unknown` effect bound to `run_id`.
    async fn seed_unknown_effect(&self, run_id: RunId) -> EffectId {
        let effect_id = EffectId::new(self.ids.as_ref());
        let mut txn = self.write().await;
        effects::prepare_effect(
            txn.as_mut(),
            &self.env(),
            effects::PrepareRequest {
                effect_id,
                run_id,
                step_sequence: 1,
                decision_id: DecisionId::new(self.ids.as_ref()),
                operation: "http.post".to_owned(),
                request_payload: b"payload".to_vec(),
                adapter: effects::AdapterBinding {
                    adapter_id: AdapterId::new(self.ids.as_ref()),
                    adapter_version: "1.0.0".to_owned(),
                    adapter_digest: "digest".to_owned(),
                },
                contract: effects::EffectContract::unknown(),
            },
        )
        .await
        .expect("prepare");
        txn.commit().await.expect("prepare commits");
        let mut txn = self.write().await;
        let outcome = effects::claim(txn.as_mut(), &self.env(), effect_id, "executor")
            .await
            .expect("claim");
        let effects::ClaimOutcome::Claimed { fencing_token, .. } = outcome else {
            panic!("claim wins")
        };
        let executor = effects::ExecutorRef {
            executor_id: "executor",
            fencing_token,
        };
        effects::mark_dispatched(txn.as_mut(), &self.env(), effect_id, &executor, None)
            .await
            .expect("dispatch");
        effects::mark_unknown(txn.as_mut(), &self.env(), effect_id, Some(&executor))
            .await
            .expect("unknown");
        txn.commit().await.expect("unknown commits");
        effect_id
    }

    async fn seed_approval(
        &self,
        run_id: RunId,
        state: ApprovalState,
        action: &str,
        effect_id: EffectId,
    ) -> ApprovalRequestId {
        let request_id = ApprovalRequestId::new(self.ids.as_ref());
        let mut txn = self.write().await;
        txn.security()
            .insert_approval_request(NewApprovalRequest {
                request_id,
                request_digest: "cd".repeat(32),
                principal_id: self.principal,
                actor_id: self.actor,
                run_id: Some(run_id),
                operation: format!("effect.resolve_unknown:{action}"),
                target_resource: Some(effect_id.to_string()),
                capability_ids: Vec::new(),
                extension_bundle_digest: None,
                config_generation_digest: None,
                expires_at_ms: SEED_MS + 60_000,
                nonce: format!("nonce-{request_id}"),
                state,
                created_at_ms: SEED_MS,
                resolved_at_ms: (state != ApprovalState::Pending).then_some(SEED_MS),
            })
            .await
            .expect("approval seeds");
        txn.commit().await.expect("approval commits");
        request_id
    }

    fn envelope(
        &self,
        key: &str,
        chain: Option<DelegationChainId>,
        payload: contract::ResolveUnknownEffect,
    ) -> CommandEnvelope {
        CommandEnvelope {
            command_id: CommandId::new(self.ids.as_ref()),
            idempotency_key: IdempotencyKey::new(key).expect("valid idempotency key"),
            principal_id: self.principal,
            actor_id: self.actor,
            device_id: None,
            delegation_chain_id: chain,
            request_digest: RequestDigest::from_str(&"ab".repeat(32)).expect("valid digest"),
            correlation_id: Some("corr-resolve".to_owned()),
            causation_id: None,
            deadline_unix_ms: None,
            command_type: runtime::CMD_RESOLVE_UNKNOWN_EFFECT.to_owned(),
            payload: payload.encode_to_vec(),
        }
    }

    async fn effect(&self, effect_id: EffectId) -> kernel_store::models::EffectRow {
        let mut txn = self.write().await;
        let row = txn
            .effects()
            .get(effect_id)
            .await
            .expect("effect read")
            .expect("effect exists");
        txn.rollback().await.expect("rollback");
        row
    }
}

fn resolve_payload(
    effect_id: EffectId,
    action: &str,
    approval_request_id: &str,
) -> contract::ResolveUnknownEffect {
    contract::ResolveUnknownEffect {
        effect_id: effect_id.to_string(),
        expected_effect_state: EffectState::Unknown.to_wire(),
        action: action.to_owned(),
        result_ref: String::new(),
        reason: "operator decision".to_owned(),
        approval_request_id: approval_request_id.to_owned(),
    }
}

#[tokio::test]
async fn capability_allows_resolution() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    let chain = h.seed_chain(RESOLVE_UNKNOWN).await;
    let effect_id = h.seed_unknown_effect(run_id).await;
    let outcome = h
        .coordinator
        .execute(h.envelope(
            "resolve-allow",
            Some(chain),
            resolve_payload(effect_id, "mark_failed", ""),
        ))
        .await
        .expect("resolution executes");
    assert_eq!(outcome.code, OutcomeCode::Ok);
    assert_eq!(h.effect(effect_id).await.state, EffectState::Failed);
}

#[tokio::test]
async fn missing_capability_denies_resolution() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    let chain = h.seed_chain(OTHER_GRANT).await;
    let effect_id = h.seed_unknown_effect(run_id).await;
    let err = h
        .coordinator
        .execute(h.envelope(
            "resolve-deny",
            Some(chain),
            resolve_payload(effect_id, "mark_failed", ""),
        ))
        .await
        .expect_err("unauthorized resolution fails");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    assert_eq!(h.effect(effect_id).await.state, EffectState::Unknown);
}

#[tokio::test]
async fn denied_policy_is_not_bypassed_by_approval() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    // A chain lacking `effect.resolve_unknown` produces Deny; an approval
    // covering the exact action+effect still cannot override it.
    let chain = h.seed_chain(OTHER_GRANT).await;
    let effect_id = h.seed_unknown_effect(run_id).await;
    let approval = h
        .seed_approval(run_id, ApprovalState::Approved, "mark_succeeded", effect_id)
        .await;
    let err = h
        .coordinator
        .execute(h.envelope(
            "resolve-approved-deny",
            Some(chain),
            resolve_payload(effect_id, "mark_succeeded", &approval.to_string()),
        ))
        .await
        .expect_err("approval cannot bypass an explicit denial");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    assert_eq!(h.effect(effect_id).await.state, EffectState::Unknown);
}

#[tokio::test]
async fn retry_requires_approved_request_even_with_capability() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    let chain = h.seed_chain(RESOLVE_UNKNOWN).await;
    let effect_id = h.seed_unknown_effect(run_id).await;
    let err = h
        .coordinator
        .execute(h.envelope(
            "resolve-retry-no-approval",
            Some(chain),
            resolve_payload(effect_id, "retry_accepting_duplicate_risk", ""),
        ))
        .await
        .expect_err("duplicate-risk retry requires an approval");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    assert_eq!(h.effect(effect_id).await.state, EffectState::Unknown);
    let approval = h
        .seed_approval(
            run_id,
            ApprovalState::Approved,
            "retry_accepting_duplicate_risk",
            effect_id,
        )
        .await;
    h.coordinator
        .execute(h.envelope(
            "resolve-retry-approved",
            Some(chain),
            resolve_payload(
                effect_id,
                "retry_accepting_duplicate_risk",
                &approval.to_string(),
            ),
        ))
        .await
        .expect("approved retry executes");
    assert_eq!(h.effect(effect_id).await.state, EffectState::Prepared);
}

#[tokio::test]
async fn approval_must_cover_the_requested_action_and_effect() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    let chain = h.seed_chain(RESOLVE_UNKNOWN).await;
    let effect_id = h.seed_unknown_effect(run_id).await;
    // Approved for a different action on this same effect.
    let wrong_action = h
        .seed_approval(run_id, ApprovalState::Approved, "mark_failed", effect_id)
        .await;
    let err = h
        .coordinator
        .execute(h.envelope(
            "resolve-wrong-action",
            Some(chain),
            resolve_payload(
                effect_id,
                "retry_accepting_duplicate_risk",
                &wrong_action.to_string(),
            ),
        ))
        .await
        .expect_err("approval for another action is not a cover");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    // Approved for the action but targeting a different effect.
    let other_effect = h.seed_unknown_effect(run_id).await;
    let wrong_effect = h
        .seed_approval(
            run_id,
            ApprovalState::Approved,
            "retry_accepting_duplicate_risk",
            other_effect,
        )
        .await;
    let err = h
        .coordinator
        .execute(h.envelope(
            "resolve-wrong-effect",
            Some(chain),
            resolve_payload(
                effect_id,
                "retry_accepting_duplicate_risk",
                &wrong_effect.to_string(),
            ),
        ))
        .await
        .expect_err("approval for another effect is not a cover");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    assert_eq!(h.effect(effect_id).await.state, EffectState::Unknown);
}

#[tokio::test]
async fn foreign_or_pending_approval_does_not_cover() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    let other_run = h.seed_run().await;
    // Retry requires an approval even with standing capability, so this
    // chain reaches the approval check.
    let chain = h.seed_chain(RESOLVE_UNKNOWN).await;
    // An approval bound to a different run must never cover this effect.
    let effect_id = h.seed_unknown_effect(run_id).await;
    let wrong_run = h
        .seed_approval(
            other_run,
            ApprovalState::Approved,
            "retry_accepting_duplicate_risk",
            effect_id,
        )
        .await;
    let err = h
        .coordinator
        .execute(h.envelope(
            "resolve-wrong-run",
            Some(chain),
            resolve_payload(
                effect_id,
                "retry_accepting_duplicate_risk",
                &wrong_run.to_string(),
            ),
        ))
        .await
        .expect_err("foreign approval is not a cover");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    // A still-pending approval on the right run does not cover either.
    let effect_id = h.seed_unknown_effect(run_id).await;
    let pending = h
        .seed_approval(
            run_id,
            ApprovalState::Pending,
            "retry_accepting_duplicate_risk",
            effect_id,
        )
        .await;
    let err = h
        .coordinator
        .execute(h.envelope(
            "resolve-pending",
            Some(chain),
            resolve_payload(
                effect_id,
                "retry_accepting_duplicate_risk",
                &pending.to_string(),
            ),
        ))
        .await
        .expect_err("pending approval is not a cover");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    assert_eq!(h.effect(effect_id).await.state, EffectState::Unknown);
}

#[tokio::test]
async fn expected_state_mismatch_is_a_conflict() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    let chain = h.seed_chain(RESOLVE_UNKNOWN).await;
    let effect_id = h.seed_unknown_effect(run_id).await;
    let mut payload = resolve_payload(effect_id, "mark_failed", "");
    payload.expected_effect_state = EffectState::Dispatched.to_wire();
    let err = h
        .coordinator
        .execute(h.envelope("resolve-stale", Some(chain), payload))
        .await
        .expect_err("stale expectation conflicts");
    assert_eq!(err.code(), ErrorCode::Conflict);
    assert_eq!(h.effect(effect_id).await.state, EffectState::Unknown);
}

#[tokio::test]
async fn non_unknown_effect_cannot_be_resolved() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    let chain = h.seed_chain(RESOLVE_UNKNOWN).await;
    // Seed a merely-Prepared effect (never dispatched).
    let effect_id = EffectId::new(h.ids.as_ref());
    let mut txn = h.write().await;
    effects::prepare_effect(
        txn.as_mut(),
        &h.env(),
        effects::PrepareRequest {
            effect_id,
            run_id,
            step_sequence: 1,
            decision_id: DecisionId::new(h.ids.as_ref()),
            operation: "fs.write".to_owned(),
            request_payload: Vec::new(),
            adapter: effects::AdapterBinding {
                adapter_id: AdapterId::new(h.ids.as_ref()),
                adapter_version: "1".to_owned(),
                adapter_digest: "d".to_owned(),
            },
            contract: effects::EffectContract::unknown(),
        },
    )
    .await
    .expect("prepare");
    txn.commit().await.expect("commit");
    let mut payload = resolve_payload(effect_id, "mark_failed", "");
    payload.expected_effect_state = EffectState::Prepared.to_wire();
    let err = h
        .coordinator
        .execute(h.envelope("resolve-prepared", Some(chain), payload))
        .await
        .expect_err("prepared effects are not resolvable");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
}

#[tokio::test]
async fn resolving_last_unknown_effect_unblocks_the_run() {
    let h = Harness::new().await;
    let run_id = h.seed_run().await;
    // Seed the run as blocked on this unknown effect (the state recovery
    // would have left after a crash with an unsafe dispatch).
    {
        let mut txn = h.write().await;
        let run = txn
            .runs()
            .get(run_id)
            .await
            .expect("run read")
            .expect("run");
        let updated = txn
            .runs()
            .cas_update(
                run_id,
                kernel_store::models::RunCas {
                    run_revision: run.run_revision,
                    state: Some(run.state),
                    cancellation_epoch: None,
                },
                kernel_store::models::RunPatch {
                    recovery: Some(domain::run::RecoveryDisposition::BlockedUnknownEffect),
                    bump_revision: true,
                    ..kernel_store::models::RunPatch::default()
                },
            )
            .await
            .expect("block run");
        assert!(updated);
        txn.commit().await.expect("commit");
    }
    let chain = h.seed_chain(RESOLVE_UNKNOWN).await;
    let effect_id = h.seed_unknown_effect(run_id).await;
    h.coordinator
        .execute(h.envelope(
            "resolve-unblocks-run",
            Some(chain),
            resolve_payload(effect_id, "mark_failed", ""),
        ))
        .await
        .expect("resolution executes");
    let mut txn = h.write().await;
    let run = txn
        .runs()
        .get(run_id)
        .await
        .expect("run read")
        .expect("run");
    txn.rollback().await.expect("rollback");
    assert_ne!(
        run.recovery,
        domain::run::RecoveryDisposition::BlockedUnknownEffect
    );
}
