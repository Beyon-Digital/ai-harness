> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)

# MVP Event Catalog

`contracts/events/catalog.yaml` is the canonical machine-readable catalog; this file is its
human companion and mirrors it exactly. Every event declares an `id`, a `version`, a
`default_sensitivity`, a `default_retention`, a `stream_key`, and a `produced_by` naming the
transition or command that emits it (R9.1–R9.3). An event type with no producing transition
fails catalog validation. Stream keys use the canonical durable stream keys from
`event-pipeline.md`; timer, resource, and workspace events publish on the owning run's stream,
`ChildRunCreated` publishes on the parent run's stream, and kernel-global registry events
(`AgentSpecRevisionStored`, daemon fence, config) publish on `config/global`.

## Classification rule

Sensitivity levels are ordered `public` < `internal` < `confidential` < `secret`. The catalog
value is the event type's minimum and default classification. A caller may raise the
sensitivity of a payload above the type default, and the raised class is recorded on the
`outbox_events` row; a downgrade below the event type's minimum is rejected (R9.4). Retention
values are `ephemeral` (liveness telemetry only), `standard` (routine durable history), and
`audit` (authority, security, configuration, recovery, and irreversible outcome records).

Applied policy:

- no security, approval, or secret event is classified below `confidential`;
- externally mutating effect outcomes, workspace authority changes, cancellation epoch
  advances, recovery dispositions, daemon fencing, and resource-unknown records are `audit`;
- only adapter health telemetry is `ephemeral`.

## Session

| Event | Version | Sensitivity | Retention | Stream key | Produced by |
|---|---|---|---|---|---|
| `SessionCreated` | 1 | `internal` | `standard` | `session/<session-id>` | `CreateSession` |
| `AgentSpecRevisionStored` | 1 | `internal` | `audit` | `config/global` | `PutAgentSpecRevision` |

## Runtime

| Event | Version | Sensitivity | Retention | Stream key | Produced by |
|---|---|---|---|---|---|
| `TaskCreated` | 1 | `internal` | `standard` | `task/<task-id>` | `CreateTaskRun` |
| `RunCreated` | 1 | `internal` | `standard` | `run/<run-id>` | `CreateTaskRun` |
| `RunReady` | 1 | `internal` | `standard` | `run/<run-id>` | `BindRun` |
| `RunBound` | 1 | `internal` | `standard` | `run/<run-id>` | `BindRun` |
| `RunStarted` | 1 | `internal` | `standard` | `run/<run-id>` | `ClaimReadyRun` |
| `RunWaitingTool` | 1 | `internal` | `standard` | `run/<run-id>` | `SubmitLoopDecision` |
| `RunWaitingChild` | 1 | `internal` | `standard` | `run/<run-id>` | `SubmitLoopDecision` |
| `RunWaitingHuman` | 1 | `internal` | `standard` | `run/<run-id>` | `SubmitLoopDecision` |
| `RunStateChanged` | 1 | `internal` | `standard` | `run/<run-id>` | `RunStateTransition` |
| `RunCompleted` | 1 | `internal` | `standard` | `run/<run-id>` | `SubmitLoopDecision` |
| `RunFailed` | 1 | `internal` | `standard` | `run/<run-id>` | `SubmitLoopDecision` |
| `RunCancelled` | 1 | `internal` | `audit` | `run/<run-id>` | `CancelRun` |

## Graph

| Event | Version | Sensitivity | Retention | Stream key | Produced by |
|---|---|---|---|---|---|
| `ChildRunCreated` | 1 | `internal` | `standard` | `run/<parent-run-id>` | `CreateTaskRun` |
| `DependencyAdded` | 1 | `internal` | `standard` | `task/<task-id>` | `AddRunDependency` |
| `RunClaimed` | 1 | `internal` | `standard` | `run/<run-id>` | `ClaimReadyRun` |
| `CancellationEpochAdvanced` | 1 | `internal` | `audit` | `run/<run-id>` | `CancelRun` |

## Loop

| Event | Version | Sensitivity | Retention | Stream key | Produced by |
|---|---|---|---|---|---|
| `LoopTurnIssued` | 1 | `internal` | `standard` | `run/<run-id>` | `LoopTurnIssue` |
| `LoopDecisionAccepted` | 1 | `internal` | `standard` | `run/<run-id>` | `SubmitLoopDecision` |
| `LoopDecisionRejectedStale` | 1 | `internal` | `standard` | `run/<run-id>` | `SubmitLoopDecision` |

## Effects

| Event | Version | Sensitivity | Retention | Stream key | Produced by |
|---|---|---|---|---|---|
| `EffectPrepared` | 1 | `internal` | `standard` | `effect/<effect-id>` | `prepare_effect` |
| `EffectClaimed` | 1 | `internal` | `standard` | `effect/<effect-id>` | `Prepared -> Claimed` |
| `EffectDispatched` | 1 | `internal` | `standard` | `effect/<effect-id>` | `Claimed -> Dispatched` |
| `EffectAcknowledged` | 1 | `internal` | `standard` | `effect/<effect-id>` | `Dispatched -> Acknowledged` |
| `EffectCommitted` | 1 | `internal` | `audit` | `effect/<effect-id>` | `Acknowledged -> Committed` |
| `EffectFailed` | 1 | `internal` | `audit` | `effect/<effect-id>` | `Dispatched\|Acknowledged -> Failed` |
| `EffectCancelled` | 1 | `internal` | `audit` | `effect/<effect-id>` | `Dispatched -> Cancelled` |
| `EffectUnknown` | 1 | `internal` | `audit` | `effect/<effect-id>` | `Dispatched -> Unknown` |
| `EffectReconciled` | 1 | `internal` | `audit` | `effect/<effect-id>` | `ResolveUnknownEffect` |

## Timers

| Event | Version | Sensitivity | Retention | Stream key | Produced by |
|---|---|---|---|---|---|
| `TimerScheduled` | 1 | `internal` | `standard` | `run/<run-id>` | `ScheduleTimer` |
| `TimerClaimed` | 1 | `internal` | `standard` | `run/<run-id>` | `Scheduled -> Claimed` |
| `TimerFired` | 1 | `internal` | `standard` | `run/<run-id>` | `Claimed -> Fired` |
| `TimerCancelled` | 1 | `internal` | `standard` | `run/<run-id>` | `Scheduled -> Cancelled` |

## Resources

| Event | Version | Sensitivity | Retention | Stream key | Produced by |
|---|---|---|---|---|---|
| `ResourceReserved` | 1 | `internal` | `standard` | `run/<run-id>` | `none -> reserved` |
| `ResourceAllocated` | 1 | `internal` | `standard` | `run/<run-id>` | `reserved -> allocated` |
| `ResourceReleased` | 1 | `internal` | `standard` | `run/<run-id>` | `allocated -> released` |
| `ResourceUnknown` | 1 | `internal` | `audit` | `run/<run-id>` | `allocated\|reserved -> unknown` |

## Workspace

| Event | Version | Sensitivity | Retention | Stream key | Produced by |
|---|---|---|---|---|---|
| `WorkspaceLeaseGranted` | 1 | `internal` | `audit` | `run/<run-id>` | `WorkspaceLeaseGrant` |
| `WorkspaceLeaseTransferred` | 1 | `internal` | `audit` | `run/<run-id>` | `WorkspaceLeaseTransfer` |
| `WorkspaceForked` | 1 | `internal` | `audit` | `run/<run-id>` | `WorkspaceFork` |
| `WorkspaceMerged` | 1 | `internal` | `audit` | `run/<run-id>` | `WorkspaceMerge` |

## Security

| Event | Version | Sensitivity | Retention | Stream key | Produced by |
|---|---|---|---|---|---|
| `CapabilityRequested` | 1 | `confidential` | `audit` | `security/principal/<principal-id>` | `PermissionEvaluation` |
| `CapabilityGranted` | 1 | `confidential` | `audit` | `security/principal/<principal-id>` | `Allow` |
| `CapabilityDenied` | 1 | `confidential` | `audit` | `security/principal/<principal-id>` | `Deny` |
| `ApprovalRequested` | 1 | `confidential` | `audit` | `security/principal/<principal-id>` | `CreateApprovalRequest` |
| `ApprovalResolved` | 1 | `confidential` | `audit` | `security/principal/<principal-id>` | `RespondApproval` |
| `SecretActionPerformed` | 1 | `confidential` | `audit` | `security/principal/<principal-id>` | `sign_or_act` |

## Adapters

| Event | Version | Sensitivity | Retention | Stream key | Produced by |
|---|---|---|---|---|---|
| `AdapterRegistered` | 1 | `internal` | `standard` | `adapter/<adapter-id>/<version>/<digest>` | `AdapterRegistration` |
| `AdapterStarted` | 1 | `internal` | `standard` | `adapter/<adapter-id>/<version>/<digest>` | `AdapterInstanceStart` |
| `AdapterHealthy` | 1 | `internal` | `ephemeral` | `adapter/<adapter-id>/<version>/<digest>` | `AdapterHealthPing` |
| `AdapterUnhealthy` | 1 | `internal` | `ephemeral` | `adapter/<adapter-id>/<version>/<digest>` | `AdapterHealthMiss` |
| `AdapterStopped` | 1 | `internal` | `standard` | `adapter/<adapter-id>/<version>/<digest>` | `AdapterInstanceStop` |

## Config

| Event | Version | Sensitivity | Retention | Stream key | Produced by |
|---|---|---|---|---|---|
| `ConfigProposed` | 1 | `internal` | `audit` | `config/global` | `ProposeConfigGeneration` |
| `ConfigTested` | 1 | `internal` | `audit` | `config/global` | `MarkConfigTested` |
| `ConfigActivated` | 1 | `internal` | `audit` | `config/global` | `ActivateConfigGeneration` |
| `ConfigRolledBack` | 1 | `internal` | `audit` | `config/global` | `RollbackConfigGeneration` |

## Recovery

| Event | Version | Sensitivity | Retention | Stream key | Produced by |
|---|---|---|---|---|---|
| `RecoveryStarted` | 1 | `internal` | `standard` | `run/<run-id>` | `RecoveryReconciliation` |
| `RunRecoveryDispositionChanged` | 1 | `internal` | `audit` | `run/<run-id>` | `RecoveryReconciliation` |
| `DaemonFenceAcquired` | 1 | `internal` | `audit` | `config/global` | `acquire_daemon_fence` |
| `DaemonFenceLost` | 1 | `internal` | `audit` | `config/global` | `DaemonAuthorityRelease` |
