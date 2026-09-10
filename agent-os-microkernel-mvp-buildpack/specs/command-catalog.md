> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)

# Kernel Command Catalog

All commands use the common `CommandEnvelope` and are executed by Command Coordinator. Payloads below are logical Rust/protobuf shapes; exact serialization may use protobuf generated types.

## `CreateSession`

```text
session_id?          # caller-supplied UUIDv7 allowed for retry-safe creation; otherwise created before digesting request
metadata
```

Creates one session owned by the authenticated principal. Event: `SessionCreated` (add to implementation event catalog if not already canonical).

## `PutAgentSpecRevision`

```text
agent_spec_id
version
body_bytes
body_digest
```

Insert-only. Existing `(id,version)` with identical digest/body is idempotent; different bytes are conflict.

## `CreateTaskRun`

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

Creates immutable Task if new plus Run in `Created`. For child creation, validates parent authority/epoch and persists parent linkage/grants/budget references atomically.

## `BindRun`

```text
run_id
expected_run_revision
config_generation_id?      # normally current active generation, exact ID captured by resolver
resolved_environment
```

Internal command produced by Config/Runtime binder. Requires run `Created`. Inserts immutable ResolvedRunEnvironment/bindings, links run, increments revision, transitions to `Ready`, emits events.

## `ClaimReadyRun`

```text
run_id
claim_owner
claim_ttl_ms
```

Internal worker command. Checks dependencies/recovery/cancellation and creates claim atomically with `Ready -> Running`.

## `AddRunDependency`

```text
source_run_id
target_run_id
condition
expected_graph_revision
```

Target must be `Created` or unclaimed `Ready`; cycle check and insert occur in same transaction.

## `CancelRun`

```text
run_id
expected_run_revision?
reason
```

Advances cancellation epoch and transitions/cancels reachable nonterminal descendants according to state.

## `SubmitLoopDecision`

```text
LoopDecision
```

Internal. Validates run revision, loop epoch, step sequence, input cursor, turn ID and decision ID. Decision types in MVP:

- `Complete { output_ref? }`
- `Fail { reason_code }`
- `Wait { reason, timer_id? }`
- `SpawnAgent { child_request }`
- `InvokeEffect { operation, payload, effect_claim }`
- `RequestApproval { approval_draft }`

Acceptance and resulting mutation/effect/child preparation are atomic.

## `ResolveUnknownEffect`

```text
effect_id
expected_effect_state = Unknown
action = mark_succeeded | mark_failed | retry_accepting_duplicate_risk
result_ref?
reason
approval_request_id?       # required by policy for duplicate-risk retry
```

## `CreateApprovalRequest`

Usually generated internally by Permission Engine. Persists immutable request/digest and blocks the requesting operation/run as appropriate.

## `RespondApproval`

```text
request_id
request_digest
decision
device_id
responder_principal_id
```

## `ProposeConfigGeneration`

```text
generation_id?
document_bytes
digest
```

Insert immutable generation in proposed/unvalidated state.

## `MarkConfigTested`

Internal test worker command with exact generation ID/digest and test report digest/result.

## `ActivateConfigGeneration`

```text
generation_id
expected_active_revision
```

Requires validated/tested generation. CAS active pointer only. Does not touch running runs.

## `ScheduleTimer`

```text
timer_id?
run_id?
timer_kind
due_at_ms
payload
```

## `CancelTimer`

```text
timer_id
expected_version
```

CAS from Scheduled only.
