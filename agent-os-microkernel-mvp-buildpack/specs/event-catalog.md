> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)

# MVP Event Catalog

The canonical contract catalog is under `contracts/events/catalog.yaml`. The MVP also requires inception lifecycle events for entities introduced by the implementation pack.

## Session/spec/task

- `SessionCreated`
- `AgentSpecRevisionStored`
- `TaskCreated`

## Runtime

- `RunCreated`
- `RunReady`
- `RunClaimed`
- `RunStarted`
- `RunPaused`
- `RunResumed`
- `RunCompleted`
- `RunFailed`
- `RunCancelled`
- `RunRecoveryDispositionChanged`

## Graph/loop

- `ChildRunCreated`
- `DependencyAdded`
- `CancellationEpochAdvanced`
- `LoopTurnIssued`
- `LoopDecisionAccepted`
- `LoopDecisionRejectedStale`

## Effects/resources/timers

- `EffectPrepared`, `EffectClaimed`, `EffectDispatched`, `EffectAcknowledged`, `EffectCommitted`, `EffectFailed`, `EffectCancelled`, `EffectUnknown`, `EffectReconciled`
- `ResourceReserved`, `ResourceAllocated`, `ResourceReleased`, `ResourceUnknown`
- `TimerScheduled`, `TimerClaimed`, `TimerFired`, `TimerCancelled`

## Workspace/security/adapters/config/recovery

Use the names in `contracts/events/catalog.yaml` plus `WorkspaceLeaseGranted`, `WorkspaceLeaseTransferred`, `WorkspaceForked`, `WorkspaceMerged`.

Each emitted event must have a declared default sensitivity and retention class in code; callers may raise sensitivity but may not downgrade below the event type's minimum.
