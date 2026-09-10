> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Command Coordinator

The Command Coordinator is the only path for state-changing kernel commands.

## Command envelope

```rust
CommandEnvelope<C> {
  command_id,
  idempotency_key,
  principal_id,
  actor_id,
  device_id: Option<_>,
  delegation_chain_id: Option<_>,
  request_digest,
  correlation_id,
  causation_id,
  deadline,
  payload: C,
}
```

## Algorithm

1. Validate syntactic request and request digest.
2. Resolve authenticated principal/actor context.
3. Begin write transaction and assert daemon fencing epoch.
4. Load `(principal_id, idempotency_key)` if present.
   - same request digest: return stored outcome after rollback/read-only exit;
   - different digest: `IDEMPOTENCY_CONFLICT`.
5. Evaluate command-specific authorization and expected revisions.
6. Execute pure domain validation.
7. Apply all canonical row changes.
8. Allocate event sequence(s) and insert outbox rows.
9. Insert idempotency outcome.
10. Commit transaction.
11. Return exactly the committed outcome.

## MVP command set

- `CreateSession`
- `PutAgentSpecRevision`
- `CreateTaskRun`
- `ClaimReadyRun`
- `TransitionRun`
- `SpawnChildRun`
- `AddRunDependency`
- `CancelRun`
- `SubmitLoopDecision`
- `ResolveUnknownEffect`
- `CreateApprovalRequest`
- `RespondApproval`
- `ProposeConfigGeneration`
- `ActivateConfigGeneration`
- `ScheduleTimer`
- `CancelTimer`

Worker actions such as effect claim or timer claim use the same transactional primitives and fencing checks even if they are internal rather than public commands.
