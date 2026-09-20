# Task EFF-004 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** EFF-004 — Effect reconciliation + Unknown handling
- **Status:** DONE
- **Commits:** `b4c0b31` (reconcile.rs), `65e987a` (runtime handler) — `feat(runtime): ResolveUnknownEffect command + reconciliation apply [EFF-004]`
- **Branch:** `devin/1789940928-effects-wave`

## What was implemented

- `reconcile.rs` — `EffectExecutor` adapter interface (`execute`, `status`,
  `cancel`) keyed by the stable operation id minted at Prepare (retries reuse
  it); `ReconcilePlan::{Settle{observed}, Redispatch, Unknown}` — only
  Dispatched rows are reconciled; `NotFound` observation yields Redispatch only
  when `is_safely_redispatchable()`; Settle applies Acknowledged->Committed or
  Failed + stages `EffectReconciled`. Unknown is never auto-retried.
- `runtime/src/effect_recovery.rs` — `CMD_RESOLVE_UNKNOWN_EFFECT` handler:
  expected-state fencing, only-Unknown gate, `effect.resolve_unknown` capability
  evaluation against the delegation chain, and an approved `approval_request`
  bound to (operation, run_id, unexpired) as the compensating control when the
  chain lacks standing capability. Actions: `mark_succeeded` -> Committed,
  `mark_failed` -> Failed, `retry_accepting_duplicate_risk` -> Prepared.

## Evidence — task ACs

- `resolve_unknown_succeeds_with_capability` (Allow -> Committed + EffectReconciled).
- `resolve_unknown_denied_without_capability_or_approval` -> Deny.
- `resolve_unknown_with_approved_request` / `pending` / `wrong-run` approval.
- `resolve_unknown_rejects_wrong_expected_state` -> Conflict.
- `resolve_unknown_rejects_non_unknown_state` -> FailedPrecondition.

`cargo test -p runtime` — 6 effect_recovery tests green; workspace suite green.
