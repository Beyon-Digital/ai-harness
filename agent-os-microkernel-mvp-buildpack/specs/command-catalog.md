> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)

# Kernel Command Catalog

This catalog is the single authoritative list of the 18 kernel commands. All commands use the
common `CommandEnvelope` and are executed by the Command Coordinator. The `command_type`
envelope field is the fully-qualified protobuf message name of the command payload
(`agentos.spec.v1.` joined with the command name; design D2). Every payload message is defined
in `contracts/control-api/commands.proto`; each section below names its payload message, its
idempotency semantics, and its emitted events (R2.4, R8.4). Payload field lists are logical
protobuf shapes.

### Command set rules

- `TransitionRun` is **not** a command. Run state transitions happen inside the command that
  causes them (`BindRun`, `ClaimReadyRun`, `SubmitLoopDecision`, `CancelRun`,
  `ResolveBlockedRun`) and are mirrored by `RunStateChanged`.
- `SpawnChildRun` is **not** a command. It is superseded by `CreateTaskRun` with
  `parent_run_id`, which is the one authoritative child-creation path (R8.3). Loop
  `SpawnAgent` decisions route through `CreateTaskRun`; they never create a child through a
  second path. No other command may create a task run.
- `command-coordinator.md` mirrors this command set; neither list may diverge from the other.
- Worker transitions that are not catalogued commands (effect claim/dispatch/acknowledge/
  commit/fail/cancel/unknown, timer claim/fire, resource reserve/allocate/release/unknown,
  workspace lease grant/transfer/fork/merge, permission evaluation, adapter instance
  lifecycle, loop turn issue, recovery reconciliation, daemon fence acquire/release) execute
  inside Command Coordinator transaction semantics and are named as `produced_by` in
  `contracts/events/catalog.yaml`. They add no Control API surface.
- Idempotency for every command follows the common contract: same
  `(principal_id, idempotency_key)` and request digest returns the stored outcome; a different
  request digest for the same key is `IDEMPOTENCY_CONFLICT`. Command-specific rules below add
  CAS, uniqueness, or duplicate-detection requirements.

### Control API mapping

Every state-changing `MvpControlApi` method maps to exactly one catalogued command (R8.1).
Read-only methods map to no command.

| `MvpControlApi` method | State-changing | Catalogued command |
|---|---|---|
| `SubmitCommand` | yes | exactly one command selected by `command_type` (the fully-qualified payload message name) |
| `RespondApproval` | yes | `RespondApproval` |
| `GetRun` | no | none (read-only) |
| `GetTask` | no | none (read-only) |
| `GetEffect` | no | none (read-only) |
| `GetRunGraph` | no | none (read-only) |
| `GetActiveConfig` | no | none (read-only) |
| `ListAdapters` | no | none (read-only) |
| `Health` | no | none (read-only) |
| `MvpEventApi.ReadStream` / `MvpEventApi.Subscribe` | no | none (read-only) |

## `CreateSession`

- Payload message: `agentos.spec.v1.CreateSession`
- Idempotency: retry-safe; a caller-supplied `session_id` lets an identical replay return the stored outcome, and the common key/digest rule applies.
- Emits: `SessionCreated`

```text
session_id?          # caller-supplied UUIDv7 allowed for retry-safe creation; otherwise created before digesting request
metadata
```

Creates one session owned by the authenticated principal.

## `PutAgentSpecRevision`

- Payload message: `agentos.spec.v1.PutAgentSpecRevision`
- Idempotency: insert-only; an existing `(agent_spec_id, version)` with identical digest/body is idempotent, different bytes are a conflict.
- Emits: `AgentSpecRevisionStored`

```text
agent_spec_id
version
body_bytes
body_digest
```

## `CreateTaskRun`

- Payload message: `agentos.spec.v1.CreateTaskRun`
- Idempotency: retry-safe with caller-supplied `task_id`/`run_id`; an identical replay returns the stored outcome and a different payload for the same IDs is a conflict. Child creation additionally validates `observed_parent_cancellation_epoch`.
- Emits: `TaskCreated`, `RunCreated`, `ChildRunCreated`, `RunStateChanged`
- This is the single authoritative child-creation path (R8.3): a child is a run created with `parent_run_id`; loop `SpawnAgent` decisions route through this command.

```text
task_id?
run_id?
session_id?
task_kind
task_payload
agent_spec_ref
parent_run_id?
observed_parent_cancellation_epoch?
requested_profile
workspace_uri?             # pre-provisioned root workspace optional
requested_capabilities
requested_budget
```

Creates immutable Task if new plus Run in `Created`. For child creation, validates parent authority/epoch and persists parent linkage/grants/budget references atomically. The `run_graph_heads` row is inserted for a new task in this same transaction.

## `BindRun`

- Payload message: `agentos.spec.v1.BindRun`
- Idempotency: internal, once-only; an identical replay returns the stored outcome, a different `resolved_environment` or a stale `expected_run_revision` is rejected. The persisted resolved environment and bindings are immutable.
- Emits: `RunReady`, `RunBound`, `RunStateChanged`

```text
run_id
expected_run_revision
config_generation_id?      # normally current active generation, exact ID captured by resolver
resolved_environment
```

Internal command produced by Config/Runtime binder. Requires run `Created`. Inserts immutable ResolvedRunEnvironment/bindings, links run, increments revision, transitions to `Ready`, emits events. `RunBound` is the bindings audit event.

## `ClaimReadyRun`

- Payload message: `agentos.spec.v1.ClaimReadyRun`
- Idempotency: claim is CAS; exactly one unexpired claim wins. Retries of the winning request return the stored outcome; a run that is not `Ready` or an unexpired existing claim fails `FAILED_PRECONDITION`.
- Emits: `RunClaimed`, `RunStarted`, `RunStateChanged`

```text
run_id
claim_owner
claim_ttl_ms
```

Internal worker command. Checks dependencies/recovery/cancellation and creates claim atomically with `Ready -> Running`.

## `AddRunDependency`

- Payload message: `agentos.spec.v1.AddRunDependency`
- Idempotency: common key/digest rule; a duplicate identical edge at the same expected graph revision is a no-op. A stale `expected_graph_revision` or a cycle is rejected.
- Emits: `DependencyAdded`

```text
source_run_id
target_run_id
condition
expected_graph_revision
```

Target must be `Created` or unclaimed `Ready`; cycle check and insert occur in same transaction.

## `CancelRun`

- Payload message: `agentos.spec.v1.CancelRun`
- Idempotency: cancellation is monotonic; once the cancellation epoch has advanced, a replay returns the stored outcome and a terminal run returns its terminal outcome. A stale expected revision is rejected.
- Emits: `CancellationEpochAdvanced`, `RunCancelled`, `RunStateChanged`

```text
run_id
expected_run_revision?
reason
```

Advances cancellation epoch and transitions/cancels reachable nonterminal descendants according to state.

## `SubmitLoopDecision`

- Payload message: `agentos.spec.v1.SubmitLoopDecision`
- Idempotency: duplicate detection by `decision_id`; the same `decision_id` with the same bytes returns the stored acceptance, a different payload for the same `decision_id` is a conflict, and a stale fencing tuple is rejected with no mutation.
- Emits: `LoopDecisionAccepted`, `LoopDecisionRejectedStale`, `RunWaitingTool`, `RunWaitingChild`, `RunWaitingHuman`, `RunCompleted`, `RunFailed`, `EffectPrepared`, `ApprovalRequested`, `RunStateChanged`

```text
LoopDecision
```

Internal. Validates run revision, loop epoch, step sequence, input cursor, turn ID and decision ID. Decision types in MVP (canonical variant names, GC-3):

- `Complete { output_ref? }` — emits `RunCompleted`
- `Fail { reason_code }` — emits `RunFailed`
- `Wait { reason, timer_id? }` — timer-backed wait emits `RunWaitingTool`
- `SpawnAgent { child_request }` — routes through `CreateTaskRun`; emits `RunWaitingChild` and the child events from that command
- `InvokeEffect { operation, payload, effect_claim }` — emits `EffectPrepared` and `RunWaitingTool`
- `RequestApproval { approval_draft }` — emits `ApprovalRequested` and `RunWaitingHuman`

Acceptance and resulting mutation/effect/child preparation are atomic; a stale decision emits `LoopDecisionRejectedStale` and mutates nothing.

## `ResolveUnknownEffect`

- Payload message: `agentos.spec.v1.ResolveUnknownEffect`
- Idempotency: requires `expected_effect_state = UNKNOWN`; an identical replay returns the stored outcome, and a different resolution of a settled effect is a conflict.
- Emits: `EffectReconciled`

```text
effect_id
expected_effect_state = UNKNOWN   # canonical EffectState literal
action = mark_succeeded | mark_failed | retry_accepting_duplicate_risk
result_ref?
reason
approval_request_id?       # required by policy for duplicate-risk retry
```

## `CreateApprovalRequest`

- Payload message: `agentos.spec.v1.CreateApprovalRequest`
- Idempotency: immutable request/digest; an identical `(request_id, request_digest)` replay returns the stored request, different content for the same ID is a conflict, and an expired request is replaced by a new request.
- Emits: `ApprovalRequested`

Usually generated internally by Permission Engine. Persists immutable request/digest and blocks the requesting operation/run as appropriate.

## `RespondApproval`

- Payload message: `agentos.spec.v1.RespondApproval`
- Idempotency: bound to `request_id` + `request_digest`; the first valid response wins, an identical replay returns the stored outcome, and any different response, expired request, or wrong principal/device is rejected.
- Emits: `ApprovalResolved`

```text
request_id
request_digest
decision
device_id
responder_principal_id
```

## `ProposeConfigGeneration`

- Payload message: `agentos.spec.v1.ProposeConfigGeneration`
- Idempotency: insert-only immutable generation; an identical `(generation_id, digest)` replay returns the stored record, different bytes for the same ID conflict.
- Emits: `ConfigProposed`

```text
generation_id?
document_bytes
digest
```

Insert immutable generation in proposed/unvalidated state.

## `MarkConfigTested`

- Payload message: `agentos.spec.v1.MarkConfigTested`
- Idempotency: idempotent for identical `(generation_id, digest, test_report_digest, test_result)`; a conflicting result for the same generation/digest is rejected.
- Emits: `ConfigTested`

Internal test worker command with exact generation ID/digest and test report digest/result.

## `ActivateConfigGeneration`

- Payload message: `agentos.spec.v1.ActivateConfigGeneration`
- Idempotency: CAS on `expected_active_revision`; an identical replay after activation returns the stored outcome, and a stale pointer or an untested/unvalidated generation fails `FAILED_PRECONDITION`.
- Emits: `ConfigActivated`

```text
generation_id
expected_active_revision
```

Requires validated/tested generation. CAS active pointer only. Does not touch running runs.

## `RollbackConfigGeneration`

- Payload message: `agentos.spec.v1.RollbackConfigGeneration`
- Idempotency: CAS on `expected_active_revision`; an identical replay returns the stored outcome, and a stale revision fails `FAILED_PRECONDITION`.
- Emits: `ConfigRolledBack`
- Requires a tested generation: the target must have passed `MarkConfigTested` (and must have been activated before). Reactivates a prior known-good generation by CAS-ing the active pointer only; running runs are unaffected because no `ResolvedRunEnvironment` row changes (R8.2).

```text
generation_id
expected_active_revision
reason
```

## `ScheduleTimer`

- Payload message: `agentos.spec.v1.ScheduleTimer`
- Idempotency: retry-safe with a caller-supplied `timer_id`; an identical replay returns the stored timer, and different due time/payload for the same ID conflicts.
- Emits: `TimerScheduled`

```text
timer_id?
run_id?
timer_kind
due_at_ms
payload
```

## `CancelTimer`

- Payload message: `agentos.spec.v1.CancelTimer`
- Idempotency: CAS from `Scheduled` at `expected_version`; exactly one of claim and cancel wins, a repeated cancel of the already-cancelled timer returns the stored outcome, and a version mismatch fails.
- Emits: `TimerCancelled`

```text
timer_id
expected_version
```

## `ResolveBlockedRun`

- Payload message: `agentos.spec.v1.ResolveBlockedRun`
- Idempotency: requires `runs.recovery_disposition = BLOCKED_MISSING_RESOURCE` (any other disposition is `FAILED_PRECONDITION`); an identical `(principal_id, idempotency_key)` replay returns the stored outcome.
- Emits: `RunRecoveryDispositionChanged`, `RunCancelled`, `RunStateChanged`
- `resume` re-validates the same frozen resource references without substituting anything (frozen `ResolvedRunEnvironment` stays immutable); `cancel` terminalizes the run through the cancellation path.

```text
run_id
action                     # resume | cancel
reason
```
