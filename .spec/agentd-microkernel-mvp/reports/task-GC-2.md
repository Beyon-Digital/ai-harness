# Task GC-2 — Align the command and event catalogs

- Status: DONE
- Owner: agent-gc2
- Commit: `9d340c8` — `docs(specs): unify command and event catalogs [GC-2]`
- Requirements: R2.4, R8.1, R8.2, R8.3, R8.4, R9.1, R9.2, R9.3, R9.4

## What was implemented

1. `specs/command-catalog.md` — rewritten as the single authority for 18 commands:
   - 18 `##` command sections, one per message in
     `contracts/control-api/commands.proto`, each with a `Payload message:` line naming the
     fully-qualified `agentos.spec.v1.<Command>` name (D2), an `Idempotency:` line (CAS,
     duplicate detection, or the common key/digest rule), and an `Emits:` line (R2.4, R8.4).
   - Added `RollbackConfigGeneration` (requires a tested generation; CAS active pointer only;
     running runs unaffected, R8.2) and `ResolveBlockedRun` (requires
     `BLOCKED_MISSING_RESOURCE`; `resume` re-validates frozen references without substitution,
     `cancel` terminalizes; R7.3).
   - Rules state `TransitionRun` is not a command, `SpawnChildRun` is superseded, and
     `CreateTaskRun` with `parent_run_id` is the one authoritative child-creation path that
     loop `SpawnAgent` decisions route through (R8.3).
   - `### Control API mapping` maps every `MvpControlApi` method: `SubmitCommand` dispatches
     exactly one command on `command_type`, `RespondApproval` maps 1:1 to
     `RespondApproval`, the seven reads map to none (R8.1). Worker transitions that are not
     commands are named in the event catalog `produced_by` and add no API surface.
   - Normalized wire literals: `expected_effect_state = UNKNOWN` and the canonical decision
     variants `Complete`, `Fail`, `Wait`, `SpawnAgent`, `InvokeEffect`, `RequestApproval`.
2. `specs/command-coordinator.md` — replaced the divergent 16-command list (`TransitionRun`,
   `SpawnChildRun` only in this file) with the same 18 as the catalog, in the same order, and
   restated the child-path, transition, and worker-transition rules.
3. `contracts/events/catalog.yaml` — converted from category lists to a list of 61 event
   records, each with `id`, `version`, `category`, `default_sensitivity`, `default_retention`,
   `stream_key`, `produced_by`; the file declares the allowed literal sets
   (`public|internal|confidential|secret`, `ephemeral|standard|audit`). Added
   `SessionCreated`, `AgentSpecRevisionStored`, `TimerScheduled`, `TimerClaimed`, `TimerFired`,
   `TimerCancelled`, `ResourceReserved`, `ResourceAllocated`, `ResourceReleased`,
   `ResourceUnknown`, `RunBound`, `RunWaitingTool`, `RunWaitingChild`, `RunWaitingHuman`,
   `RunStateChanged`, and removed `RunPaused`/`RunResumed` (D8). `produced_by` names the
   catalogued command or the worker transition (effect/timer/resource/workspace lifecycle
   arrows from the specs).
4. `specs/event-catalog.md` — rewritten as the human companion: 61 events in 12 category
   tables with version, sensitivity, retention, stream key, and producer, matching
   `catalog.yaml` exactly; states the classification rule that callers may raise sensitivity
   but downgrades below the type minimum are rejected (R9.4) and that an event type with no
   producing transition fails catalog validation (R9.3).
5. `specs/README.md` — index entries added for `event-journal-schema.sql` and `limits.yaml`.

## Acceptance criteria

| Criterion | Status | Evidence |
|---|---|---|
| R8.1 — every state-changing Control API method maps to exactly one catalogued command | met | `command-catalog.md` `### Control API mapping` table covers all nine `MvpControlApi` RPCs; `SubmitCommand` → one command selected by `command_type`; `RespondApproval` → `RespondApproval`; reads → none |
| R8.2 — `RollbackConfigGeneration` catalogued, requires tested generation, running runs unaffected | met | `command-catalog.md` `## RollbackConfigGeneration`: "the target must have passed `MarkConfigTested`", "no `ResolvedRunEnvironment` row changes"; present in the 18 and in `commands.proto` |
| R8.3 — child creation has one authoritative path, stated in both files | met | `command-catalog.md` Command set rules + `CreateTaskRun`; `command-coordinator.md` "`SpawnChildRun` is not a command. `CreateTaskRun` with `parent_run_id` is the one authoritative child-creation path" |
| R9.1/R9.2 — every catalogued event declares version, sensitivity, retention, stream key, producer | met | 61/61 records pass the field check; `category`, `version: 1`, enum values validated |
| R9.3 — no event without a producing transition; paused/resumed removed | met | all 61 `produced_by` values non-empty; `grep -n 'RunPaused\|RunResumed'` prints nothing |
| R9.4 — classification rule: raises allowed, downgrades rejected | met | `event-catalog.md` "Classification rule" section |
| No file outside `files:` changed | met | commit `9d340c8` touches exactly the 5 leased paths |

## Verification output

```text
$ python3 tools/validate_repo.py
OK: 237 markdown, 13 canonical ports, no link/schema/catalog errors

$ grep -c '^## ' agent-os-microkernel-mvp-buildpack/specs/command-catalog.md
18

$ grep -n 'RunPaused\|RunResumed' agent-os-microkernel-mvp-buildpack/specs/event-catalog.md agent-os-microkernel-mvp-buildpack/contracts/events/catalog.yaml
(no output; exit 1 = no matches)

$ python3 -c "import re,yaml; c=...; ids={e['id'] for e in c['events']}; md=set(re.findall(<task regex>, open('.../specs/event-catalog.md').read())); print('catalog-only', sorted(md-ids)); print('md-only', sorted(ids-md))"
catalog-only []
md-only ['AdapterHealthy', 'AdapterRegistered', 'AdapterStarted', 'AdapterStopped', 'AdapterUnhealthy', 'ApprovalRequested', 'ApprovalResolved', 'CapabilityDenied', 'CapabilityRequested', 'ConfigActivated', 'ConfigProposed', 'ConfigRolledBack', 'ConfigTested', 'DaemonFenceAcquired', 'DaemonFenceLost', 'LoopDecisionRejectedStale', 'RecoveryStarted', 'RunCompleted', 'RunReady', 'RunStarted', 'SecretActionPerformed']

# Reconciliation of the residual 'md-only' list: the task regex only recognizes a subset of
# event-name endings, so it cannot extract names ending in Ready/Started/Completed/Requested/
# Denied/Healthy/Stopped/Proposed/Tested/Activated/RolledBack/Acquired/Lost/Performed/Stale.
# Each reported name is present verbatim in event-catalog.md, and a structure-aware extraction
# of the markdown tables shows the sets are exactly equal:
$ python3 -c "
import re,yaml
c=yaml.safe_load(open('.../contracts/events/catalog.yaml'))
ids={e['id'] for e in c['events']}
md=set(re.findall(r'^\| \`([A-Za-z][A-Za-z0-9]+)\` \|', open('.../specs/event-catalog.md').read(), re.M))
print('md-not-catalog', sorted(md-ids)); print('catalog-not-md', sorted(ids-md)); print('both-empty', md==ids)"
md-not-catalog []
catalog-not-md []
both-empty True

$ python3 -c "... field/count validation ..."
count 61
dupes []
missing []
bad sens []
bad ret []

$ python3 -c "... cross-check catalog vs coordinator vs proto and emitted-event names ..."
catalog commands: 18 coordinator: 18 proto: 18
catalog==coordinator: True
catalog==proto(set): True
emitted names not in catalog: {}
catalog events whose producing command omits them in its Emits list: []
UNKNOWN literal present: True
decision variants present: True
```

## Files changed

- `agent-os-microkernel-mvp-buildpack/specs/command-catalog.md` (18 commands, FQN/idempotency/emits, mapping, D9 commands)
- `agent-os-microkernel-mvp-buildpack/specs/command-coordinator.md` (18-command list aligned)
- `agent-os-microkernel-mvp-buildpack/contracts/events/catalog.yaml` (61 classified events)
- `agent-os-microkernel-mvp-buildpack/specs/event-catalog.md` (human companion, classification rule)
- `agent-os-microkernel-mvp-buildpack/specs/README.md` (index entries)

## Concerns

- The task's event-set comparison regex cannot extract event names that do not end in its
  suffix list (e.g. `RunReady`, `RunCompleted`, `CapabilityRequested`); its `md-only` output
  is a coverage artifact, not a content difference. I verified every reported name is present
  verbatim and demonstrated exact set equality with a table-aware extraction (output above).
- Sensitivity/retention literals in `catalog.yaml` use the task's canonical values
  (`public|internal|confidential|secret`, `ephemeral|standard|audit`). `contracts/events/event.proto`
  still declares `PRIVATE` and `SESSION`/`DURABLE` enum names, and is outside this task's file
  lease; GC-7 or a later contract task should reconcile the enum spellings.
- Worker `produced_by` names (e.g. `LoopTurnIssue`, `WorkspaceLeaseGrant`,
  `AdapterInstanceStart`, `RecoveryReconciliation`, `DaemonAuthorityRelease`) and the stream
  choices for events without their own canonical stream (`AgentSpecRevisionStored`,
  `DaemonFenceAcquired/Lost` on `config/global`; timer/resource/workspace events on the owning
  `run/<run-id>`; `ChildRunCreated` on `run/<parent-run-id>`) are derived from the specs'
  transition vocabulary, since the pack names neither; `event-pipeline.md` is outside the
  lease and was not touched.

## Fix — review finding (Important)

- Finding: `specs/event-catalog.md:25` claimed no security, config, approval, or secret event
  is classified below `confidential`, but the four Config events are `internal` in
  `contracts/events/catalog.yaml` and the md tables; the same line had stray code formatting
  on `no`.
- Decision applied: keep the catalog values as `internal` and drop `config` from the
  confidential-floor bullet, matching the catalog.
- Change: `specs/event-catalog.md` — bullet is now "no security, approval, or secret event is
  classified below `confidential`;" (backticks removed from `no`).
- Cheap follow-through: `specs/command-catalog.md` — added, under both the
  `SubmitLoopDecision` and `ResolveBlockedRun` `Emits:` lists, that the list includes
  transitively emitted events (so a reverse-direction check against `produced_by` does not
  false-positive).
- Commit: `7712534` — `docs(specs): fix event classification policy and emits wording [GC-2]`
  (2 files, both leased).
- Verification:

```text
$ python3 tools/validate_repo.py
OK: 241 markdown, 13 canonical ports, no link/schema/catalog errors

$ python3 -c "... bullet vs Config classifications ..."
policy bullet: - no security, approval, or secret event is classified below `confidential`;
config events: {'ConfigProposed': 'internal', 'ConfigTested': 'internal', 'ConfigActivated': 'internal', 'ConfigRolledBack': 'internal'}
bullet mentions config: False
stray backtick-no: False
command sections: 18
transitive note count: 2

$ grep -n '^| `Config' agent-os-microkernel-mvp-buildpack/specs/event-catalog.md
137:| `ConfigProposed` | 1 | `internal` | `audit` | `config/global` | `ProposeConfigGeneration` |
138:| `ConfigTested` | 1 | `internal` | `audit` | `config/global` | `MarkConfigTested` |
139:| `ConfigActivated` | 1 | `internal` | `audit` | `config/global` | `ActivateConfigGeneration` |
140:| `ConfigRolledBack` | 1 | `internal` | `audit` | `config/global` | `RollbackConfigGeneration` |
```

Task left in `review` state for controller re-review (not marked done).

