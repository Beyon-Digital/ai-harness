# Task Index

Total tasks: **59**. `dag.yaml` is authoritative for dependencies.

## Foundation

- [FND-001 — Bootstrap Rust workspace and quality gates](FND-001.md) — depends: none
- [FND-002 — Install canonical contract snapshot and protobuf generation](FND-002.md) — depends: FND-001
- [FND-003 — Implement domain IDs, core enums, and immutable value types](FND-003.md) — depends: FND-002
- [FND-004 — Implement stable error model](FND-004.md) — depends: FND-001
- [FND-005 — Build deterministic testkit and fault-injection hooks](FND-005.md) — depends: FND-001, FND-003
- [FND-006 — Initialize classified tracing and metrics substrate](FND-006.md) — depends: FND-001, FND-003, FND-004

## Persistence

- [PST-001 — Create inception SQLite KernelStore schema](PST-001.md) — depends: FND-003, FND-004
- [PST-002 — Define KernelStore transaction interfaces](PST-002.md) — depends: FND-003, FND-004
- [PST-003 — Implement SQLite KernelStore transactions and repositories](PST-003.md) — depends: PST-001, PST-002, FND-005
- [PST-004 — Implement OS daemon lock and durable fencing epoch](PST-004.md) — depends: PST-003, FND-005
- [PST-005 — Implement idempotency and transactional outbox primitives](PST-005.md) — depends: PST-003, PST-004

## Control Core

- [CMD-001 — Implement Command Coordinator linearization path](CMD-001.md) — depends: PST-005, FND-006

## Events

- [EVT-001 — Implement event envelopes, stream keys, cursors, and sequence helpers](EVT-001.md) — depends: FND-003, PST-005
- [EVT-002 — Implement EventJournalPort and SQLite Event Journal](EVT-002.md) — depends: EVT-001, FND-004
- [EVT-003 — Implement outbox dispatcher with journal-first publication](EVT-003.md) — depends: EVT-002, PST-005, FND-005
- [EVT-004 — Implement durable Live Event Bus and lag semantics](EVT-004.md) — depends: EVT-003

## Runtime

- [RUN-000 — Implement AgentSpec, Session, Task, and initial AgentRun commands](RUN-000.md) — depends: CMD-001, PST-003
- [RUN-001 — Implement AgentRun state machine and revision rules](RUN-001.md) — depends: CMD-001, FND-003, RUN-000
- [RUN-002 — Implement transactional RunGraph edge mutation and cycle prevention](RUN-002.md) — depends: RUN-001, PST-003
- [RUN-003 — Implement dependency satisfaction and atomic ready-run claiming](RUN-003.md) — depends: RUN-002
- [RUN-004 — Implement cancellation epochs and subtree cancellation](RUN-004.md) — depends: RUN-002, RUN-001
- [RUN-005 — Implement startup run reconstruction and recovery dispositions](RUN-005.md) — depends: RUN-003, RUN-004, PST-004

## Effects

- [EFF-001 — Implement effect contract model and conservative policy resolver](EFF-001.md) — depends: FND-003, FND-004
- [EFF-002 — Implement EffectRecord transitions and preparation](EFF-002.md) — depends: EFF-001, CMD-001, RUN-001
- [EFF-003 — Implement effect executor leases and fencing](EFF-003.md) — depends: EFF-002, PST-004
- [EFF-004 — Implement effect reconciliation and Unknown handling](EFF-004.md) — depends: EFF-003, RUN-005

## Resources

- [RES-001 — Implement durable resource reservations and budget delegation](RES-001.md) — depends: CMD-001, RUN-001

## Scheduler

- [SCH-001 — Implement durable one-shot scheduler timers](SCH-001.md) — depends: CMD-001, PST-003, FND-005

## Security

- [SEC-001 — Implement principals, actors, delegation chains, and grant lineage](SEC-001.md) — depends: FND-003, PST-003
- [SEC-002 — Implement capability and permission engine](SEC-002.md) — depends: SEC-001, FND-004
- [SEC-003 — Implement immutable approval request/response flow](SEC-003.md) — depends: SEC-002, CMD-001
- [SEC-004 — Implement Secrets Broker and macOS Keychain store](SEC-004.md) — depends: SEC-002, SEC-003, FND-006

## Adapters

- [ADP-001 — Implement supervised external process lifecycle and private socketpair IPC](ADP-001.md) — depends: PST-004, FND-006
- [ADP-002 — Implement framed protobuf external adapter protocol](ADP-002.md) — depends: ADP-001, FND-002
- [ADP-003 — Implement content-addressed adapter registry](ADP-003.md) — depends: ADP-002, PST-003
- [ADP-004 — Implement port capability negotiation and deterministic resolver](ADP-004.md) — depends: ADP-003
- [ADP-005 — Build fixture external effect adapter](ADP-005.md) — depends: ADP-004, EFF-004
- [ADP-006 — Implement adapter conformance harness and activation gate](ADP-006.md) — depends: ADP-005

## Workspace

- [WRK-001 — Implement logical Resource URI parser/resolver](WRK-001.md) — depends: FND-003, SEC-002
- [WRK-002 — Implement local workspace adapter and Git worktree primitives](WRK-002.md) — depends: WRK-001, ADP-004
- [WRK-003 — Implement Workspace Coordinator leases, transfer, fork, and merge](WRK-003.md) — depends: WRK-002, RUN-004, SEC-002

## Sandbox

- [SBOX-001 — Implement Sandbox Manager and T0 trusted local-process adapter](SBOX-001.md) — depends: ADP-004, WRK-003, SEC-004

## Artifacts

- [ART-001 — Implement local ArtifactStore and metadata](ART-001.md) — depends: WRK-001, PST-003

## Config

- [CFG-001 — Implement inception config parser and schema validation](CFG-001.md) — depends: ADP-004, ADP-006, FND-002, QUE-001
- [CFG-002 — Implement immutable config generations and tested activation](CFG-002.md) — depends: CFG-001, CMD-001
- [CFG-003 — Implement runtime profile resolution and frozen ResolvedRunEnvironment](CFG-003.md) — depends: CFG-002, WRK-003, SBOX-001, ART-001

## Loop

- [LOOP-001 — Build fixture external AgentLoop process](LOOP-001.md) — depends: ADP-004, RUN-001
- [LOOP-002 — Implement loop turn supervisor and fenced decision acceptance](LOOP-002.md) — depends: LOOP-001, RUN-005, EFF-002, RUN-004

## Api

- [API-001 — Implement local gRPC server over Unix-domain socket](API-001.md) — depends: PST-004, FND-006, FND-002
- [API-002 — Implement command/query Control API methods](API-002.md) — depends: API-001, CMD-001, CFG-003, EFF-004, SEC-003
- [API-003 — Implement Event API read and live subscription](API-003.md) — depends: API-001, EVT-004, EVT-002
- [API-004 — Build local control CLI](API-004.md) — depends: API-002, API-003

## Integration

- [INT-001 — Compose agentd startup/shutdown and basic end-to-end run](INT-001.md) — depends: API-004, LOOP-002, SCH-001, RES-001
- [INT-002 — Verify hierarchical children and workspace delegation end-to-end](INT-002.md) — depends: INT-001, WRK-003
- [INT-003 — Verify effect crash/restart and reconciliation end-to-end](INT-003.md) — depends: INT-001, ADP-005
- [INT-004 — Run concurrency and property invariant suite](INT-004.md) — depends: RUN-004, EFF-004, RES-001, SCH-001, WRK-003, CFG-002
- [INT-005 — Run security and protocol invariant suite](INT-005.md) — depends: SEC-004, SBOX-001, ADP-005, API-003

## Release

- [REL-001 — Implement automated MVP release gate](REL-001.md) — depends: INT-002, INT-003, INT-004, INT-005

## Queue

- [QUE-001 — Implement MessageQueuePort and in-memory generation-global adapter](QUE-001.md) — depends: FND-002, FND-003

