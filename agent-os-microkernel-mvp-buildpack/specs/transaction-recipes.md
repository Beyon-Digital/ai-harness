> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)

# Critical Transaction Recipes

These recipes are normative linearization sequences for the MVP.

## Recipe A — generic idempotent command

```text
BEGIN IMMEDIATE when mutation needs writer ordering, otherwise normal write transaction
assert daemon fencing epoch
lookup idempotency(principal,key)
  same digest -> return stored outcome, no mutation
  different digest -> conflict
validate authorization/revisions
apply canonical mutations
for each durable event:
  increment event_stream_heads(stream)
  insert outbox(event_id, stream, sequence, payload...)
insert idempotency outcome
COMMIT
return outcome
```

## Recipe B — bind Created run and freeze environment

```text
BEGIN
assert daemon fence
load run FOR logical CAS
require state=Created and expected run_revision
require environment not already present
validate active/exact config generation
validate each adapter identity still registered and eligible
insert resolved_run_environment
insert one resolved_binding per port
update run:
  resolved_environment_id = env
  state = Ready
  run_revision += 1
insert RunReady + bindings audit outbox events
COMMIT
```

No future command exposes an update operation for the environment/bindings rows.

## Recipe C — accept loop decision that creates an effect

```text
BEGIN
assert daemon fence
load run
compare run_revision, loop_epoch, step_sequence, input_cursor
validate issued turn_id and decision_id uniqueness
resolve exact adapter from frozen run environment
compute conservative effective EffectContract
insert EffectRecord(state=Prepared, immutable request/hash, adapter digest)
update run to WaitingTool
advance step/cursor as defined
increment run_revision
insert EffectPrepared + LoopDecisionAccepted + run-state outbox events
persist decision outcome/idempotency
COMMIT
```

Only after commit may an effect worker claim/dispatch.

## Recipe D — accept loop decision that spawns child

```text
BEGIN IMMEDIATE
assert daemon fence
load/fence parent loop decision
check parent current cancellation_epoch == child request observed epoch
check agent.spawn capability + delegated capability/budget subset
insert child Task if separate logical task is requested; MVP default same Task unless explicit
insert child Run(state=Created,parent_run_id=parent)
increment task RunGraph revision
persist child delegation chain/grants/reservations
if isolated fork is requested:
  prepare workspace-fork effect/resource record, child remains Created until ready
update parent waiting/running state according to decision
increment parent run_revision
insert events + decision outcome
COMMIT
```

## Recipe E — add dependency

```text
BEGIN IMMEDIATE
assert graph expected revision
require target Created or Ready+unclaimed
recursive reachability query from target to source
if reachable -> GRAPH_CYCLE
insert dependency
increment graph revision
outbox DependencyAdded
COMMIT
```

## Recipe F — ready-run claim

```text
BEGIN IMMEDIATE
load run + dependency terminal states + recovery disposition
require Ready, unclaimed/expired, recovery=Normal
require dependency conditions satisfied
require cancellation policy allows start
write claim_owner/token/expiry
transition Ready -> Running
increment run_revision
outbox RunClaimed + RunStarted
COMMIT
```

## Recipe G — parent cancellation

```text
BEGIN IMMEDIATE
load root run
increment root cancellation_epoch
find current descendant closure through parent_run_id
transition eligible descendants/root toward Cancelling/Cancelled
increment revisions of changed runs
outbox cancellation events
COMMIT
```

A child spawn transaction using the prior observed epoch fails after this commit.

## Recipe H — effect claim and dispatch

Claim transaction:

```text
BEGIN IMMEDIATE
assert daemon fence
load effect
require Prepared or expired Claim eligible by recovery policy
next fencing_token
state=Claimed, executor_id, lease_expiry, token
outbox EffectClaimed
COMMIT
```

Before external call:

```text
BEGIN
CAS effect Claimed/current token -> Dispatched
outbox EffectDispatched
COMMIT
call adapter(effect_id/operation_id/request_hash/token)
```

Response commit checks same token. Lost response uses reconciliation recipe, never a new logical operation ID.

## Recipe I — timer claim versus cancel

Both use expected `(state=Scheduled, version=N)`.

Claim updates to `Claimed, version=N+1`; cancel updates to `Cancelled, version=N+1`. SQLite write serialization/CAS ensures one wins.

## Recipe J — workspace exclusive transfer

```text
BEGIN IMMEDIATE
load lease
require owner=from_run, mode=EXCLUSIVE_WRITE, epoch=expected
verify delegation/right to transfer
advance lease_epoch and owner=to_run
persist delegation lineage
outbox WorkspaceLeaseTransferred
COMMIT
```

Then coordinator/sandbox enforcement revokes old writer handles before acknowledging usable authority to the child. If strong revocation cannot be enforced, the adapter cannot advertise strong exclusive-transfer security semantics.

## Recipe K — config activation

```text
BEGIN
assert generation validated+tested and digest exact
CAS active_config_generation revision
update pointer/revision only
outbox ConfigActivated
COMMIT
```

No existing `ResolvedRunEnvironment` row is updated.
