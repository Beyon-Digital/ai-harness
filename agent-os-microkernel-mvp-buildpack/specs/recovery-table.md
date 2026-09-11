> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Recovery Decision Table

At startup (lifecycle step 7) the daemon classifies every non-terminal run and every
non-terminal effect. Each `(run state, effect state)` pair maps to exactly one
`RecoveryDisposition` from the frozen enum in `contracts/domain/core.proto`: `NORMAL`,
`NEEDS_RECONCILIATION`, `RECOVERING`, `BLOCKED_UNKNOWN_EFFECT`, `BLOCKED_MISSING_RESOURCE`,
`REQUIRES_HUMAN_DECISION`. `RunPaused` and `RunResumed` no longer exist in the MVP catalog
(design D8), so no disposition references them.

Notation:

- Effect state `NONE` means no `effects` row references the run. The settled states
  `COMMITTED`, `FAILED`, and `CANCELLED` are equivalent to `NONE` for recovery: a settled
  effect adds no ambiguity, so the run-state row applies.
- `PREPARED`, `CLAIMED`, `DISPATCHED`, `ACKNOWLEDGED`, and `UNKNOWN` are the in-flight
  effect states.
- `lease` is `effects.lease_expires_unix_ms`; `epoch` is the daemon fencing epoch acquired at
  startup (lifecycle step 4), which is always newer than any epoch owned by a prior daemon.

## Matrix A — runs with no in-flight effect

| Run state | Condition | Disposition | Recovery action |
|---|---|---|---|
| `CREATED` | — | `NORMAL` | No loop turn can have run yet; the binder may proceed once dependencies allow. |
| `READY` | — | `NORMAL` | The claim worker may proceed after recovery validation. |
| `RUNNING` | — | `RECOVERING` | Reacquire the run claim and loop epoch under the current daemon epoch, validate frozen loop/adapter availability, then `NORMAL`. |
| `WAITING_TOOL` | no in-flight effect (timer-backed wait) | `NORMAL` | Claim an overdue `Scheduled` timer normally; there is no effect to reconcile. |
| `WAITING_CHILD` | all children terminal for the declared condition | `RECOVERING` | Re-derive parent advancement and issue the next loop turn under the current epoch. |
| `WAITING_CHILD` | some child still non-terminal | `NORMAL` | Children are recovered independently; the parent stays waiting. |
| `WAITING_HUMAN` | approval pending, not expired | `NORMAL` | The approval request remains pending; there is no recovery action. |
| `WAITING_HUMAN` | approval expired | `REQUIRES_HUMAN_DECISION` | Do not auto-resume; surface the run to an operator to renew, respond, or cancel. |
| `SUSPENDED` | — | `REQUIRES_HUMAN_DECISION` | No MVP command or transition resumes a suspended run (design D8 removed pause/resume); an operator decides to cancel or re-create. Recovery never invents a resume. |
| `CANCELLING` | — | `RECOVERING` | Continue cancellation under the current epoch: cancel reachable non-terminal descendants, then terminalize `CANCELLED`. |
| `COMPLETED`, `FAILED`, `CANCELLED` | — | `NORMAL` | Terminal run; no recovery action. |

## Matrix B — runs with an in-flight effect

| Run state | Effect state | Condition | Disposition | Recovery action |
|---|---|---|---|---|
| `RUNNING` / `WAITING_TOOL` | `PREPARED` | — | `NORMAL` | Safe to claim; the protocol persists `DISPATCHED` before any adapter call, so no external operation exists. |
| `CANCELLING` | `PREPARED` | — | `RECOVERING` | Cancel the prepared effect inside the cancellation transaction; never dispatch it. |
| `RUNNING` / `WAITING_TOOL` | `CLAIMED` | lease expired | `RECOVERING` | Reclaim with a higher fencing token; never dispatch under the stale token. |
| `RUNNING` / `WAITING_TOOL` | `CLAIMED` | lease live and daemon epoch changed | `RECOVERING` | The old executor is fenced by the epoch change; reclaim with a higher fencing token, then continue. |
| `RUNNING` / `WAITING_TOOL` | `CLAIMED` | lease live and epoch unchanged | `RECOVERING` | Do not double-claim; wait for expiry or reclaim only under configured policy. |
| `CANCELLING` | `CLAIMED` | — | `RECOVERING` | Fence the old executor and cancel the claim; never dispatch. |
| `RUNNING` / `WAITING_TOOL` / `CANCELLING` | `DISPATCHED` | reconciliation is `STATUS_LOOKUP`, `RESULT_LOOKUP`, or `DETERMINISTIC_INSPECTION` | `NEEDS_RECONCILIATION` | Query the provider with the same operation ID/provider ref; never a fresh operation ID. |
| `RUNNING` / `WAITING_TOOL` / `CANCELLING` | `DISPATCHED` | reconciliation is `IMPOSSIBLE` or `UNKNOWN` and idempotency is `NATURALLY_IDEMPOTENT` or `IDEMPOTENCY_KEY_SUPPORTED` | `NEEDS_RECONCILIATION` | Redispatch the same operation ID only under configured policy, then record the outcome. |
| `RUNNING` / `WAITING_TOOL` / `CANCELLING` | `DISPATCHED` | reconciliation is `IMPOSSIBLE` or `UNKNOWN` and idempotency is `NOT_IDEMPOTENT` or `UNKNOWN_IDEMPOTENCY` | `BLOCKED_UNKNOWN_EFFECT` | Mark the effect `UNKNOWN`, block the run, and require a recorded `ResolveUnknownEffect` decision (R7.4). |
| `RUNNING` / `WAITING_TOOL` / `CANCELLING` | `ACKNOWLEDGED` | owning run transition uncommitted | `RECOVERING` | Deterministic replay: commit effect `COMMITTED` and the uncommitted owning-run transition atomically under the current epoch. |
| `RUNNING` / `WAITING_TOOL` / `CANCELLING` | `ACKNOWLEDGED` | owning run transition already committed | `RECOVERING` | Complete effect `COMMITTED` from the durable acknowledgment; no external call. |
| `RUNNING` / `WAITING_TOOL` / `CANCELLING` | `UNKNOWN` | — | `BLOCKED_UNKNOWN_EFFECT` | Never dispatch again; the only exit is a recorded `ResolveUnknownEffect` decision (R7.4). |
| `SUSPENDED` | `PREPARED`, `CLAIMED`, or `ACKNOWLEDGED` | — | `REQUIRES_HUMAN_DECISION` | No resume path exists (design D8); complete only local effect cleanup that an operator directs. |
| `SUSPENDED` | `DISPATCHED` | — | `REQUIRES_HUMAN_DECISION` | Reconcile and record the external outcome first; an operator still decides cancel or re-create, and recovery never resumes automatically. |
| `SUSPENDED` | `UNKNOWN` | — | `BLOCKED_UNKNOWN_EFFECT` | Resolve the unknown effect with a recorded decision before anything else. |

## Matrix C — effect with a terminal owning run

Effects can outlive the run that owned them (for example, a crash during cancellation).
Terminal run states do not change here; the effect is cleaned up or reconciled.

| Run state | Effect state | Condition | Disposition | Recovery action |
|---|---|---|---|---|
| `COMPLETED` / `FAILED` / `CANCELLED` | `PREPARED` | orphaned effect, terminal owning run | `NORMAL` | Cancel the orphaned effect; the protocol guarantees it was never dispatched. |
| `COMPLETED` / `FAILED` / `CANCELLED` | `CLAIMED` | — | `NORMAL` | Fence the claim and cancel it; `DISPATCHED` was never persisted, so no external operation exists. |
| `COMPLETED` / `FAILED` / `CANCELLED` | `DISPATCHED` | — | `NEEDS_RECONCILIATION` | The external operation may exist; reconcile and record the effect outcome. The run state does not change. |
| `COMPLETED` / `FAILED` / `CANCELLED` | `ACKNOWLEDGED` | — | `RECOVERING` | Complete effect `COMMITTED` from the durable acknowledgment. The run state does not change. |
| `COMPLETED` / `FAILED` / `CANCELLED` | `UNKNOWN` | — | `BLOCKED_UNKNOWN_EFFECT` | A recorded `ResolveUnknownEffect` decision is required before cleanup. The run state does not change. |

## Matrix D — missing resources

A missing resource is a durable reference that cannot be resolved, not an ambiguous external
effect. It never selects a replacement binding (the frozen `ResolvedRunEnvironment` is
immutable).

| Run state | Condition | Disposition | Recovery action |
|---|---|---|---|
| any non-terminal run | the frozen `ResolvedRunEnvironment`, a binding's adapter id/version/digest, the workspace URI, or the referenced config generation cannot be loaded or validated | `BLOCKED_MISSING_RESOURCE` | Do not substitute a replacement; exit through `ResolveBlockedRun` (`resume` retries the same frozen references, `cancel` terminalizes the run). |
| any run | effect `PREPARED` or `CLAIMED` whose frozen adapter id/version/digest is unavailable | `BLOCKED_MISSING_RESOURCE` | Do not silently resolve a replacement adapter; exit through `ResolveBlockedRun`. |
| any run | the same conditions as above and the effect state is `UNKNOWN` | `BLOCKED_UNKNOWN_EFFECT` | `BLOCKED_UNKNOWN_EFFECT` takes precedence because external side effects may already exist; resolve it first. |

## Unmapped combinations fail closed

If startup encounters a `(run state, effect state)` pair that no matrix row assigns, the
daemon MUST fail closed (design D7): it selects no default disposition, marks itself
unhealthy, dispatches no effect, reports the offending `(run_id, run_state, effect_id,
effect_state)`, and exits. `REQUIRES_HUMAN_DECISION` is a disposition a row assigns, never a
fallback.

Combinations that no row assigns are unmapped, including:

- `CREATED` or `READY` with any in-flight effect: no loop turn can have run before binding;
- `WAITING_CHILD` or `WAITING_HUMAN` with any in-flight effect: effects settle before those
  transitions are committed (transaction recipes C and D), so the state is inconsistent;
- an effect whose `run_id` does not resolve to a run row;
- any pair containing an enum value this build does not know (proto unknown-value handling
  stays an error and is never a default).

## Blocked-run exit: `ResolveBlockedRun`

`BLOCKED_MISSING_RESOURCE` has exactly one catalogued exit: `ResolveBlockedRun { string
run_id = 1; string action = 2; string reason = 3; }` (design D9), where `action` is `resume`
or `cancel`.

- The command requires `runs.recovery_disposition = BLOCKED_MISSING_RESOURCE`; any other
  disposition is rejected as `FAILED_PRECONDITION`, so it cannot clear `BLOCKED_UNKNOWN_EFFECT`
  or `REQUIRES_HUMAN_DECISION`.
- `resume` re-validates the same frozen resource references (exact adapter id/version/digest,
  workspace URI, config generation) without substituting anything. If they resolve now, the
  run follows the normal recovery sequence (`RECOVERING` then `NORMAL`); if any still fails,
  the run stays blocked and the missing reference is returned.
- `cancel` transitions the run through the cancellation path and terminalizes it as
  `CANCELLED`.
- The command is idempotent per the common key/digest contract: a repeated
  `(principal_id, idempotency_key)` with the same digest returns the stored outcome.

## Unknown effects never redispatch silently

R7.4: an effect in state `UNKNOWN`, or any row that selects `BLOCKED_UNKNOWN_EFFECT`, is never
dispatched again by recovery. The recorded exit is `ResolveUnknownEffect` with
`expected_effect_state = Unknown` and an explicit `action` (`mark_succeeded`, `mark_failed`,
or `retry_accepting_duplicate_risk`); the duplicate-risk retry additionally requires an
approval request when policy demands one. `ResolveBlockedRun` does not apply here.

## Recovery actions outside the run-by-effect matrix

These records are recovered before a run's disposition returns to `NORMAL` (recovery
ordering). The disposition shown is the classification the recovery pass reports; only run
dispositions are persisted in `runs.recovery_disposition`.

| Persisted condition after restart | Disposition | Safe action |
|---|---|---|
| Command absent from the idempotency table | `NORMAL` | The caller may retry the command with the same request/idempotency key. |
| Command idempotency outcome present | `NORMAL` | Return the stored outcome; never execute the command again. |
| Outbox event not journal-published | `NORMAL` | Append the same event ID/sequence to the Event Journal. |
| Outbox event journaled but the publication mark is absent | `NORMAL` | Reappend; an exact duplicate is success; then mark published. |
| Timer `Scheduled` and overdue | `NORMAL` | Claim it normally. |
| Timer `Claimed` by a stale daemon/worker | `RECOVERING` | Reclaim under fencing/lease rules; never mark `Fired` without the command outcome. |
| Resource reserved with a terminal owning run | `NORMAL` | Release if the resource is purely logical; reconcile first if an external allocation may exist. |
| External allocation uncertain | `NEEDS_RECONCILIATION` | Mark the resource unknown; never assume release. |
| Workspace lease whose owner run is terminal | `NORMAL` | The coordinator may release the logical lease after verifying that no active effect or process requires it. |
| External process from a prior daemon instance | `RECOVERING` | Treat it as stale/untrusted; terminate or reconcile it; never accept messages under the old daemon epoch. |

## Recovery ordering

Recover authoritative records before issuing new loop turns. Effect, timer, and resource
ambiguity is resolved before a run's `RecoveryDisposition` returns to `NORMAL`.
