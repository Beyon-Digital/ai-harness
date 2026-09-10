# Plan — agentd-microkernel-mvp

<!-- Phase 1 artifact. Read-only research + decisions. NO code is written in this phase. -->

**Status:** draft
**Date:** 2026-09-10

## Problem statement

`/Users/jainamshah/Documents/GitHub/ai-harness` contains 224 markdown files, 32 canonical
protobuf/JSON-Schema contracts, and a 59-task build pack — and zero lines of executable code.
There is no `Cargo.toml`, no `crates/`, no `.git`. Both self-validators pass
(`tools/validate_repo.py` → "OK: 224 markdown, 13 canonical ports"; 
`agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` → "BUILD PACK OK: 59 tasks,
111 markdown files, contracts locked"), but they only check link integrity, task-ID presence,
and hash locks. They do not check whether the specs are *implementable*. They are not.

An engineer starting FND-001 today would hit an invented-behaviour decision within the first
three tasks: the protobuf snapshot does not compile, no command payload has a wire schema, and
14 categories of numeric threshold that task tests explicitly assert against have no value
specified anywhere in the pack.

## Outcome

A running `agentd` daemon on macOS that satisfies all 24 criteria in
`agent-os-microkernel-mvp-buildpack/MVP_EXIT_CRITERIA.md:5-28`, reached by a spec-flow
DAG whose tasks are individually implementable without inventing kernel behaviour. Before
implementation starts, the ~40 specification gaps found in this audit are closed as normative
addenda, not discovered one at a time by coding agents.

## Audit findings — shortcomings in the existing documentation

This is the "if you find any shortcomings or errors, lmk" deliverable. Ordered by severity.
Both validators pass, so none of these are caught by existing tooling.

### Blocking — would stop or corrupt work in the first three tasks

**A1. Two conflicting `ControlApi` definitions in one protobuf package — the snapshot does not compile.**
`contracts/control-api/control.proto:1` and `contracts/control-api/mvp_control.proto:2` both
declare `package agentos.spec.v1`, and both define `CommandRequest`, `CommandResponse`,
`GetRunRequest/Response`, `GetEffectRequest/Response`, `ApprovalResponseRequest`,
`ApprovalResponseResult`. The field sets differ: `control.proto:8` has 6 fields,
`mvp_control.proto:26-38` has 11 (adds `principal_id`, `device_id`, `deadline_unix_ms`,
`correlation_id`, `causation_id`). Both are locked in `contracts/contract-lock.sha256:6-7`, and
FND-002 (`dag.yaml:25-42`) requires generating Rust types from the whole snapshot. `protoc` will
reject duplicate message definitions in one package. Neither `contracts/README.md` nor
`SOURCE_CORRECTIONS.md` names which is normative. Also note `control.proto` is not in the
canonical repo-root `spec/` tree at all — only `spec/control-api/control.proto` is, and
`mvp_control.proto` is a build-pack addition with no explanation.

**A2. No wire schema for any command payload.**
`mvp_control.proto:36-37` types the command as `string command_type` + `bytes payload`, and
`specs/command-catalog.md:5` explicitly disclaims the encoding: "exact serialization may use
protobuf generated types." So there is no message definition for `CreateTaskRun` (13 logical
fields at `specs/command-catalog.md:29-41`), `SubmitLoopDecision`, `BindRun`, or
`ResolveUnknownEffect`; and no canonical `command_type` string values. The same hole exists for
loop decisions: `contracts/protocols/agent_loop.proto:8` is `bytes payload` + free-string
`decision_type`, and the six decision variants at `specs/command-catalog.md:96-101` have no
messages. API-002 (server), API-004 (CLI), and LOOP-001 (fixture loop) must agree byte-for-byte
across three separate tasks and nothing tells them how.

**A3. `request_digest` algorithm is undefined, which breaks idempotency end to end.**
`specs/types-and-ids.md:36` says SHA-256 "over canonical bytes… For JSON, canonicalize before
hashing" — no canonicalization algorithm named (no JCS/RFC 8785), no hex-vs-base64, and no
statement of which envelope fields are covered. MVP exit criterion 2
(`MVP_EXIT_CRITERIA.md:6`) and the tests `PST-005` "same key/different digest conflict" and
`API-002` "API idempotency replay test" cannot be written against a client that computes the
digest independently.

**A4. The `files` ownership model omits every Rust module root, which silently breaks the
parallel-agent guarantee.** Zero of the 59 tasks in `dag.yaml` declare a `lib.rs`, `main.rs`, or
`mod.rs` — verified by grep. FND-001 declares only `crates/*/Cargo.toml` (`dag.yaml:14`).
`crates/agentd/src/main.rs` is owned solely by INT-001 (`dag.yaml:916`), the *last* task before
release, yet six earlier tasks each add a sibling module that must be `mod`-declared in it:
PST-004 `lock.rs` (:178), EVT-003 `workers/outbox.rs` (:268), RUN-005 `recovery.rs` (:388),
SCH-001 `workers/scheduler.rs` (:492), LOOP-002 `workers/loops.rs` (:829), API-001 `api.rs`
(:847). Nine of those pairs have no dependency path in either direction, so
`DAG.md:274` explicitly authorizes them to run concurrently — each editing a file it does not
own. The same defect repeats in 11 more crates (`runtime` is worst: 13 modules across 8 tasks),
producing 38 concurrent same-crate pairs plus 23 more inside `kernel-store-sqlite`, whose
`repos/*.rs` glob (`dag.yaml:160`) is claimed wholesale by PST-003 and then re-claimed
file-by-file by ten downstream tasks.

**A5. Fourteen categories of threshold are asserted by tests but specified nowhere.**
The only concrete numbers in the entire pack are `0600` (`specs/control-api.md:8`),
`busy_timeout = 5000` (`architecture/persistence.md:18`, and that block is labelled
"Recommended" — non-normative), and `max_children: 4` in a non-normative example. Missing:
max frame size (`ADP-002` test "oversized frame rejected", `dag.yaml:602`); queue capacity
(`QUE-001` "bounded backpressure", :1018); live-bus capacity and lag threshold (`EVT-004`,
:284); effect lease duration (`EFF-003` "lease expiry/reclaim", :443); run `claim_ttl_ms`;
daemon fence lease duration and renewal interval; approval TTL (`SEC-003` "expired request
rejected", :546); request deadline, handshake timeout, health ping interval; supervisor restart
count/backoff; dispatcher retry/backoff/poll interval; scheduler poll interval; drain deadline
(`specs/lifecycle.md:29` says "configured deadline" with no value and no config key); `ReadStream`
default/max limit; DB file mode. Workspace leases have **no expiry column at all** in the schema
yet `WRK-003` tests lease enforcement. Compounding it,
`contracts/config/agent-os.schema.json:83` declares `policies` as bare `{"type":"object"}`, so
even the config *key names* for these knobs are undefined.

### High — a task cannot meet its own acceptance criteria

**B1. Four entities have repositories/tasks but no table.** `specs/kernel-store-schema.sql`
defines 25 tables (verified). Absent: **artifacts** — ART-001 (`dag.yaml:728-743`) and
`specs/artifacts.md:16` require digest/media-type/size/origin/sensitivity/retention metadata,
while PST-001's acceptance criteria (`dag.yaml:130-131`) forbid a repo auto-creating a table and
accept "only the exact inception schema" — a direct contradiction; **workspaces** —
`workspace_leases.workspace_id` (schema:128) is unconstrained TEXT with no parent table, so fork
lineage is unrepresentable though WRK-002/WRK-003 require it; **adapter instances / supervised
processes** — `specs/process-supervisor.md:33` requires durable PID/exit-reason/heartbeat and
`specs/recovery-table.md:24` requires detecting a process from a prior daemon instance, which
`AGENTS.md` forbids holding in memory; **conformance reports** —
`adapter_registrations.conformance_state` (schema:268) is a bare enum with no report digest,
but ADP-006 acceptance (`dag.yaml:1040-1042`) requires binding to exact bundle digest.

**B2. No table for issued loop turns or accepted decisions.** `specs/transaction-recipes.md:54`
requires validating `turn_id`/`decision_id` uniqueness; `specs/runtime-manager.md:32,39` requires
an issued-turn record and persisted decision ID; `testing/property-tests.md:10` makes the
decision tuple a uniqueness property. The schema has neither table — the only `decision_id`
column is `effects.decision_id` (schema:146), so a `Complete`/`Fail`/`Wait`/`SpawnAgent`
decision leaves no durable trace and LOOP-002's "duplicate decision does not duplicate
effect/child" test has nothing to CAS against. `runs` also has no `current_turn_id`.

**B3. `effects` has no daemon fencing-epoch column.** `dag.yaml:444` requires the test "daemon
fence change rejects old executor" and `specs/kernel-store.md:25` requires every write to assert
the current epoch, but `effects` (schema:142-169) stores only `executor_fencing_token` — a
separate counter from `daemon_fence.fencing_epoch` (schema:11), with nothing correlating a claim
to the epoch that issued it. Same gap on `timers.claim_fencing_token` (:198) and
`runs.claim_token` (:80).

**B4. State-enum CHECK constraints are missing on 14 columns, contradicting PST-003.**
`dag.yaml:169` requires "every correctness constraint backed by DB condition/constraint, not
only pre-check logic." CHECKs exist on `resource_reservations.state`, `timers.state`,
`approval_requests.state`, `approval_responses.decision`. Absent on `runs.state`,
`runs.recovery_disposition`, `effects.state`, `effects.effect_class`,
`effects.idempotency_semantics`, `effects.reconciliation_semantics`, `workspace_leases.mode`,
`workspace_leases.enforcement_state`, `outbox_events.sensitivity`, `outbox_events.retention`,
`adapter_registrations.trust_state`/`conformance_state`,
`config_generations.validation_state`/`test_state`, `run_dependencies.dependency_condition`.
The int↔enum mapping is never written down either, and the `workspace_leases` partial index
hardcodes `mode = 2` (schema:140) with no comment tying it to `EXCLUSIVE_WRITE`. Nothing makes
`resolved_run_environments` immutable, yet CFG-003 (`dag.yaml:796`) tests that an UPDATE is
rejected.

**B5. `kernel_meta` never gets a version row, and the PRAGMAs are non-normative.** The schema
file's only PRAGMA is `foreign_keys = ON` (schema:1). WAL, `synchronous`, and `busy_timeout`
appear only under the word "Recommended" at `architecture/persistence.md:12-19`, while PST-001
step 3 mandates them. `kernel_meta` (schema:3-6) is defined but the SQL never inserts a row, and
neither the key name nor the BLOB encoding is specified — so PST-001's "wrong schema version
fails closed" test is not reproducible across two agents.

**B6. Recovery is not a matrix, and 2 of 6 dispositions are never assigned.**
`contracts/domain/core.proto:8` defines six `RecoveryDisposition` values;
`NEEDS_RECONCILIATION` and `REQUIRES_HUMAN_DECISION` appear in no row of
`specs/recovery-table.md` and in no spec sentence. `examples/default-config.yaml:24` sets
`effects.unknown_default: require_human_decision` while `specs/effect-coordinator.md:65` says
the same condition yields `BlockedUnknownEffect` — two dispositions for one state. Missing rows
that tasks and chaos scenarios reference: effect `Acknowledged` with run transition uncommitted
(exactly `testing/chaos-scenarios.md:19` item 12); effect `Claimed` with live lease but changed
daemon epoch; run `WaitingChild` at restart; run `WaitingHuman` with expired approval; run
`Cancelling` at restart; run `Suspended` at restart; effect `Prepared` with terminal owning run.
And no command clears `BLOCKED_MISSING_RESOURCE` — `ResolveUnknownEffect`
(`command-catalog.md:105`) covers only `Unknown`, so a run blocked per `recovery-table.md:12`
has no documented exit.

### Medium — coverage and consistency

**C1. All 18 chaos scenarios are unowned by the DAG.** No task lists a chaos test file and no
task doc references `testing/chaos-scenarios.md`. Yet `MVP_EXIT_CRITERIA.md:28` requires
"chaos/recovery tests all pass" and REL-001 must link every criterion to a passing test name.
Indirect partial coverage exists for 17 of them via other tasks, but scenario 6 (crash after
live publish, before ack) has none anywhere, and INT-003's "all described process-level
scenarios" (`dag.yaml:950`) never names which document it means. Similarly 3 of 6 integration
scenarios have no e2e file: D stale loop response (`integration-scenarios.md:20`), E config
freeze (:24), F sandbox fail-closed (:28) — each is framed as process-level but only unit tests
are assigned. Property tests are clean: all 15 are owned by INT-004.

**C2. Command catalog and coordinator list different command sets.**
`specs/command-catalog.md` documents 16 commands; `specs/command-coordinator.md:44-59` lists a
different 16. In the coordinator with no payload spec anywhere: `TransitionRun` (:48),
`SpawnChildRun` (:49). In the catalog but absent from the coordinator's MVP set: `BindRun`
(`command-catalog.md:46`), `MarkConfigTested` (:140). In neither: any config rollback command,
although `contracts/capabilities/security-capabilities.yaml:8` grants `config: rollback`,
`contracts/events/catalog.yaml:9` defines `ConfigRolledBack`, `specs/config-engine.md:43`
requires it, and CFG-002 tests "reactivation" (`dag.yaml:778`). Child creation has three
overlapping paths (`SpawnChildRun`, `CreateTaskRun` with `parent_run_id`,
`SubmitLoopDecision{SpawnAgent}`) with no statement of which is authoritative.

**C3. Ten event IDs exist only in markdown; three emitted events exist in no catalog.**
`contracts/events/catalog.yaml` is locked (`contract-lock.sha256:12`) and has no timer or
resource category. Present only in `specs/event-catalog.md`: `SessionCreated:9`,
`AgentSpecRevisionStored:10`, four timer events (:39), four resource events (:38).
`specs/command-catalog.md:14` even instructs agents to "add to implementation event catalog if
not already canonical" — which the contract lock forbids without a lock update. Emitted by a
recipe but in neither catalog: the "bindings audit" event
(`specs/transaction-recipes.md:41`) and any `RunWaitingTool`/`RunWaitingChild`/
`RunWaitingHuman`/`RunStateChanged` (recipe at :58,61 moves a run to `WaitingTool` and emits
"run-state outbox events"). Inversely, `RunPaused`/`RunResumed` are in both catalogs with no
run state, no command, and no transition producing them. Also unspecified pack-wide: per-event
`event_version`, and the default sensitivity/retention class that
`specs/event-catalog.md:45` and EVT-001 acceptance (`dag.yaml:239`) both require — so every
agent picks different values. Which `stream_key` each event type writes to is never mapped
despite `specs/event-pipeline.md:20` noting one command may emit to several.

**C4. Three task files contradict `dag.yaml`.** The six DAG sources (`dag.yaml`, `dag.json`,
`DAG.md` mermaid, `tasks.csv`, `tasks/*.md`, `TASK_STATUS.yaml`) agree on all 59 IDs and all 140
edges — that part is genuinely well maintained. But: `tasks/API-001.md:35` adds a fifth required
test, "peer principal mismatch rejected", absent from `dag.yaml:848-852`; it is the only test
enforcing the peer-credential boundary at `specs/control-api.md:33`, so a tool consuming
`dag.yaml` as machine-readable truth drops it. `tasks/RUN-002.md:8` scopes the task to deriving
ancestry from the single `runs.parent_run_id` field, matching `specs/run-graph.md:8` and the
validator at `scripts/validate_buildpack.py:78`; `dag.yaml:336` says "parent/child **and**
dependency graph storage", which reads as building the second edge table the validator forbids —
`dag.yaml` is the wrong one here. `tasks/CFG-002.md:31-35` lists the same five tests in a
different order.

**C5. Cursor representation is inconsistent.** `runs.input_event_cursor` is TEXT (schema:76) and
a string in `core.proto:15`, but `SubscribeEventsRequest` uses `uint64 after_sequence`
(`mvp_control.proto:74`). EVT-001 requires canonical cursor encoding (`dag.yaml:229`) and
`specs/control-api.md:29` requires returning a resumable cursor on lag, yet the proto
`Subscribe` has no trailer or error message that can carry one. Single-stream sequence or
composite across streams? Undefined.

### Low

**C6.** `MANIFEST.json:4` declares `"file_count": 156` but the `files` array holds 155 entries
(the manifest omits itself). All 155 hashes and byte sizes verify exactly, and
`contracts/contract-lock.sha256` verifies clean across all 32 entries — but no script reads
`MANIFEST.json`, so it drifts silently.

**C7.** Twelve `files` entries are unusable as ownership declarations, including the prose entry
`build.rs or dedicated proto crate` (`dag.yaml:33`) and recursive globs `proto/**` (:32) which
nests over `proto/control/**` (:867) and `proto/events/**` (:885).

**C8.** Path/name drift: PST-001 declares `schema/kernel_store.sql` (`dag.yaml:122`) but the
pack ships the source as `specs/kernel-store-schema.sql` — different name and directory, no
mapping note. EVT-002 declares `schema/event_journal.sql` (:250) whose content exists only as an
inline block at `specs/event-pipeline.md:24-35`, never as a file.

**C9.** `specs/types-and-ids.md` is not sufficient for two agents to agree: UUIDv7 is named (:8)
but not its string encoding (hyphenated lowercase vs 32-char simple hex); digest encoding is not
specified (hex vs base64, `sha256:` prefix or not) even though `adapter_registrations`' primary
key includes `bundle_digest` (schema:262) and `specs/resource-uri.md:16` must round-trip
the `adapter://ID@VERSION#DIGEST` form; no semver requirement exists anywhere although ADP-004
tests "port version mismatch" (`dag.yaml:636`); and the newtype list (:13-25) omits ~17 IDs used
throughout (`PrincipalId`, `ActorId`, `DeviceId`, `CommandId`, `DecisionId`, `TurnId`,
`OperationId`, `AgentSpecId`, `AdapterId`, `DependencyId`, `DelegationChainId`, `ArtifactId`,
`SandboxId`, `EnvironmentId`, `DaemonInstanceId`, `EventStreamKey`, `IdempotencyKey`, `Nonce`),
while :28 forbids raw `String` IDs in kernel APIs.

**C10.** `LoopDecision::Complete { output_ref? }` (`command-catalog.md:96`) has nowhere to
persist — `runs` has `terminal_reason` (schema:82) but no `output_ref`, so INT-001's "restart
after completed run retains audit data" loses the output. And nothing says who inserts the
`run_graph_heads` row (schema:104) that every task requires; RUN-000 creates tasks, and Recipe D
(`transaction-recipes.md:77`) only increments an existing row, so first child spawn hits a
missing row.

### What is clean

Worth stating, because it changes how much of the pack we keep: all 59 task IDs and all 140
dependency edges are identical across six sources; the DAG is acyclic and
`DAG.md:212-270` is a valid topological order with zero violations; the 31-crate map agrees
across `architecture/crate-map.md`, the README tree, and every crate path in `dag.yaml`;
`contracts/contract-lock.sha256` verifies byte-exact; the three intentional divergences from
canonical `spec/` are documented in `SOURCE_CORRECTIONS.md` and frozen as D-029; there are no
unresolved placeholder markers anywhere; 23 of 24 exit criteria map to an owning task; the 9-RPC
Control API surface agrees across proto, spec, and API-002; `run_dependencies` /
`run_graph_heads` correctly avoid duplicating ancestry.

## Assumptions surfaced

| # | Assumption | If wrong, what changes |
|---|---|---|
| 1 | The 30 frozen decisions in `DECISIONS.md:5-34` and the 24 exit criteria stay frozen; my job is to close spec gaps, not revisit architecture | If any decision is open, the requirements phase reopens it and the whole DAG shifts |
| 2 | Confirmed: one git repository rooted at `ai-harness`, with the Rust code in an `agent-os/` subdirectory; spec-flow artifacts stay at `.spec/`, and build-pack task file paths inherit an `agent-os/` prefix | If `agent-os/` is meant to be its own repository, every Phase 3 file path and the git workflow change |
| 3 | Confirmed: `git init` at the `ai-harness` root before implementation; per-task commit evidence and spec-flow's audit trail both depend on it | If you version this elsewhere, the TASK_STATUS commit field stays null and spec-flow cannot cite commits |
| 4 | Confirmed: spec gaps are closed as **normative addenda inside the build pack** (`specs/`, `contracts/`, `dag.yaml`), and `contract-lock.sha256` plus `MANIFEST.json` are regenerated; the pack remains the single source of truth | If gaps are closed only in `.spec/`, the build pack goes stale and diverges from what we build |
| 5 | `protoc` stays vendored (`protoc-bin-vendored`) so builds are hermetic on a machine without it | If you want system `protoc`, FND-002 gains an install step and CI needs it too |
| 6 | Rust 1.94.0 / cargo 1.94.0 and SQLite 3.39.5 are the targets (all installed). `rust-toolchain.toml` pins 1.94.0 | A different MSRV changes FND-001 and possibly dependency choices |
| 7 | Confirmed: `sqlx` over `rusqlite`, with bundled SQLite. The transaction layer must supply `BEGIN IMMEDIATE` explicitly, and no `sqlx` migration machinery is used — the inception schema is executed verbatim per PST-001 | `rusqlite` would make the repository layer synchronous instead. `sqlx`'s compile-time query macros need `DATABASE_URL` or committed offline metadata, a choice PST-002/PST-003 must make |
| 8 | The MVP is single-machine macOS-only; no cross-platform CI matrix | Adding Linux changes the Keychain store (SEC-004) and the socketpair/UDS assumptions (ADP-001) |
| 9 | I keep the pack's 59 task IDs as the vocabulary and map them into the spec-flow DAG, rather than renumbering — so `TASK_STATUS.yaml` and `tasks/*.md` stay usable as per-task briefs | Renumbering orphans 59 hand-written task briefs, which are the pack's highest-value asset |
| 10 | Confirmed: `mvp_control.proto` is normative for the MVP; `control.proto` is deleted from the build-pack contract snapshot and the lock is regenerated | If `control.proto` were normative, the API surface shrinks to 4 RPCs and API-002/003 lose most of their scope |

Correct any of these and I will revise before going further.

## Codebase evidence

| Finding | Evidence (`path:line`) | Consequence for this work |
|---|---|---|
| Zero executable code; docs + contracts only | `find` over repo: 224 md, 32 contracts, 2 python validators, no `Cargo.toml` | Every task is greenfield; no legacy integration risk, no existing conventions to match beyond the pack's own rules |
| Repo is not a git repository | `git status` → "fatal: not a repository" | `git init` needed before implementation; per-task commit evidence (`AGENTS.md` completion protocol) depends on it |
| Agent rules are normative and override skill defaults | `agent-os-microkernel-mvp-buildpack/AGENTS.md` (15 execution rules, 8 prohibited shortcuts) | These become first-class requirements, including "never weaken a kernel invariant to make a test pass" and the `OPEN_QUESTIONS.md` escalation protocol |
| Exact quality-gate commands are specified | `AGENTS.md` completion protocol; `dag.yaml:17-20` | Verification commands are given, not guessed: `cargo check --workspace`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` |
| 24 machine-checkable exit criteria | `MVP_EXIT_CRITERIA.md:5-28` | These are the acceptance criteria for Phase 2a; each becomes an EARS line with a named test |
| 30 frozen architectural decisions | `DECISIONS.md:5-34` | Constrain the design phase; D-004 (one transaction), D-005 (journal-first), D-007 (never retry Unknown), D-019 (T0 not a boundary) are the load-bearing ones |
| 59 tasks with goals, files, tests, acceptance already written | `dag.yaml:1-1042`, `tasks/*.md` | Phase 3 is largely a translation with ownership fixes, not a from-scratch decomposition |
| DAG is acyclic and topologically ordered correctly | verified via Kahn's algorithm over 59 nodes / 140 edges | The dependency structure is trustworthy; only file-ownership needs repair |
| Inception schema defines 25 tables | `specs/kernel-store-schema.sql` (verified via CREATE TABLE grep) | Substantial head start; 4 tables + 14 CHECK constraints + immutability triggers must be added |
| Both validators pass but check only structure | `tools/validate_repo.py` → "OK: 224 markdown, 13 canonical ports"; `scripts/validate_buildpack.py` → "BUILD PACK OK: 59 tasks, contracts locked" | Existing tooling cannot catch any finding above; the gap-closure work needs its own validation |
| Toolchain present: cargo/rustc 1.94.0, sqlite 3.39.5; `protoc` absent | verified via `--version` | Drives assumptions 5 and 6 |

## Existing conventions to follow

The pack specifies these; they are not my invention.

- Build: `cargo check --workspace` (`dag.yaml:17`)
- Test: `cargo test --workspace` (`dag.yaml:20`); focused form is `cargo test -p CRATE FILTER`
- Lint: `cargo clippy --workspace --all-targets -- -D warnings` (`AGENTS.md`)
- Format: `cargo fmt --check` (`AGENTS.md`)
- Pack validators: `python3 tools/validate_repo.py` and
  `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py`
- Error handling: single typed taxonomy in `crates/errors` with machine-readable codes and
  structural retry-safety (`dag.yaml:62-77`); no `unwrap()`/`expect()` in request or runtime
  paths (`AGENTS.md` prohibited shortcuts)
- Module boundaries: 31 crates with explicit "must not own" columns
  (`architecture/crate-map.md:10-40`); libraries in one process, never network services (D-028)
- Per-task evidence: update `TASK_STATUS.yaml` with status, commit SHA, exact test commands, and
  evidence (`AGENTS.md` completion protocol)
- Ambiguity protocol: append to `OPEN_QUESTIONS.md` with task ID, the ambiguous contract, two or
  more interpretations, and the safest default — never invent silently (`AGENTS.md`)

## Approach

### Chosen

**Close the specification gaps as the first documentation wave of the foundation module, then
implement the pack's 59 tasks through the spec-flow DAG with repaired file ownership.**

You rejected a separate spec-gap stage and asked for the gaps to be fixed directly from this
research, so gap closure is not a module of its own: its normative pack edits are the first wave
of the foundation module, before any Rust task can start, and they are ordinary DAG tasks whose
`files` sit under the build pack, so conflict detection covers them like anything else.

Stages:

1. **Gap closure (foundation wave 1, documentation only — no Rust).** Resolve the protobuf
   collision (A1); add command and decision payload messages (A2); pin the digest algorithm and
   ID/version/cursor encodings (A3, C5, C9); add the four missing tables, 14 CHECK constraints,
   fencing-epoch columns, immutability triggers, `kernel_meta` seed row, and normative PRAGMAs
   (B1–B5); complete the recovery matrix (B6); reconcile the command catalog and event catalog
   including sensitivity/retention defaults (C2, C3); publish one normative `limits`/`timeouts`
   config section with every threshold from A5; fix the `dag.yaml` ↔ task-file contradictions and
   path drift (C4, C8); extend `scripts/validate_buildpack.py` to fail on a threshold with no
   value and on a task whose `files` omits a module root; regenerate `contract-lock.sha256` and
   correct `MANIFEST.json` (C6). No Rust is written in this wave.

2. **Ownership repair.** Assign every `lib.rs`, `main.rs`, and `mod.rs` to exactly one task.
   FND-001 gains `crates/*/src/lib.rs` and `crates/agentd/src/main.rs` as stubs with all `mod`
   declarations pre-declared as empty modules, so later tasks add file bodies without editing a
   shared root. Where that is impossible, add a dependency edge. Then `specflow validate` can
   actually enforce non-overlap.

3. **Foundation → verification.** The pack's Stages A–K (`buildpack/PLAN.md:9-89`) mapped onto
   spec-flow waves, preserving all 140 edges and the 59 task IDs as vocabulary.

4. **Release gate.** REL-001 extended to own a chaos suite (C1) and the three missing e2e
   scenarios, so all 24 exit criteria have a named passing test.

5. **Per-wave checkpoint.** After each wave: full workspace test, clippy, fmt, both pack
   validators, and `TASK_STATUS.yaml` update.

Because the request bundles 15 independently testable capabilities across 59 tasks, this warrants
a **capability map with one spec per module** (see below) rather than one 59-task spec — the
skill's own guidance at the "several independently testable capabilities" row. The alternative,
one monolithic spec, produces a requirements document nobody reviews carefully.

### Rejected

| Alternative | Why not |
|---|---|
| Separate spec-gap stage with its own approval gate | You rejected it on review: it adds a whole gate cycle for documentation edits that the foundation module must make anyway. Folding the edits into foundation wave 1 keeps them first without a second gate |
| Start FND-001 immediately and file `OPEN_QUESTIONS.md` entries as gaps surface | The pack's own protocol says continue only when the default "cannot violate a frozen decision." A1 (proto collision) blocks FND-002 outright; A2/A3 sit under D-004's single-transaction idempotency invariant; A5's missing thresholds are asserted by 14 task tests. These are stop conditions, not defaults — you would stall in wave 1 with agents inventing incompatible answers in parallel |
| Treat the build pack as the spec and skip spec-flow entirely | The pack has no approval gates, no file-conflict detection, and no atomic task claiming. Its parallelism rule (`DAG.md:274`) is prose, and finding A4 shows it is already violated by nine authorized-concurrent pairs. `specflow validate`/`claim` mechanically enforce what the pack only asks for |
| Discard the pack and re-derive tasks from `docs/` | Throws away 59 hand-written task briefs with tests and acceptance criteria, a verified-acyclic 140-edge graph, and a locked contract snapshot — the repo's most valuable assets. The defects are localized and repairable |
| One spec directory for all 59 tasks | A requirements document covering persistence, events, effects, security, sandboxing, workspaces, config, and API is not reviewable. Gate approval becomes rubber-stamping, which defeats the only gate that matters |
| Fix gaps only inside `.spec/` and leave the pack untouched | Creates two competing sources of truth. `README.md:42` establishes `spec/` > `docs/` authority and the pack claims frozen authority over the MVP; a third tree with different answers is exactly the "second source of truth" the architecture forbids |
| Defer the schema additions to the tasks that need them (ART-001 adds `artifacts`, etc.) | PST-001 acceptance (`dag.yaml:130-131`) explicitly forbids a repository auto-creating a table and accepts only the exact inception schema. Deferring means violating a stated acceptance criterion |
| Substitute Postgres for SQLite to get real constraint/trigger ergonomics | D-003 freezes SQLite as the inception `KernelStore` and D-015 makes it bootstrap-global and non-swappable. Out of bounds for this work |

### Reframed targets

The pack's qualitative claims, restated measurably for Phase 2a. You accepted these values
(answer 3); they become normative in the gap-closure wave and live in one config `limits` section:

- "bounded backpressure" (`QUE-001`) → in-memory queue capacity 1024 messages per topic;
  publish beyond capacity returns `RESOURCE_EXHAUSTED` rather than blocking
- "oversized frame rejected" (`ADP-002`) → max adapter frame 4 MiB; a declared length above it
  closes the connection without reading the body
- "slow subscriber lag" (`EVT-004`) → live-bus buffer 256 events per subscriber; on overflow the
  durable subscriber is disconnected with its last journal-backed cursor, never silently skipped
- "lease expiry/reclaim" (`EFF-003`) → effect lease 30 s, renewed at 10 s
- run claim TTL (`RUN-003`) → 30 s; daemon fence lease 15 s renewed at 5 s
- approval TTL (`SEC-003`) → 15 min default, per-request override
- drain deadline (`specs/lifecycle.md:29`) → 30 s, then forced close
- adapter handshake timeout 5 s; request deadline default 60 s; health ping every 10 s with 3
  missed pings before restart; supervisor restart 5 attempts with exponential backoff 100 ms →
  10 s
- `ReadStream` limit default 100, max 1000
- `kernel.db` mode `0600`, containing directory `0700`

## Scope

**In scope**

Everything lands under `agent-os/` in this repository, except the gap-closure pack edits:

- Closing all findings A1–A5, B1–B6, C1–C10 as normative pack addenda
- The 59 build-pack tasks: everything in `SCOPE.md:5-39` — `agentd` lifecycle, SQLite
  `KernelStore`, daemon fencing, Command Coordinator, outbox, Event Journal + live bus, runtime
  and RunGraph, Effect Coordinator with `Unknown` handling, reservations, timers, identity and
  capabilities, approvals, Secrets Broker with macOS Keychain, Process Supervisor, Adapter
  Registry with capability negotiation, Config Engine with immutable generations,
  `ResolvedRunEnvironment`, Workspace Coordinator with Git worktrees, T0 sandbox, artifact store,
  Resource URI resolver, gRPC Control API over UDS, observability hooks
- Fixture AgentLoop and fixture effect adapter (`SCOPE.md:35-39`)
- `agentctl` CLI
- Test suites: unit, integration, concurrency, property, chaos, security
- `scripts/release_gate.sh` proving all 24 exit criteria
- `git init` and CI workflow

**Explicitly out of scope**

Per `buildpack/README.md:27-36` and `SCOPE.md:41-45`, plus my own additions:

- Production Hermes/Codex loops; real OpenAI/Anthropic integration
- Memory strategies, context engines, RAG
- Remote relay, mobile app, web app
- Agent-authored extension generation
- Cloud infrastructure adapters
- T2/T3 sandbox *implementations* — the kernel enforces the tier contract and fails closed
- Any DB migration lifecycle; inception schema only
- Phases 9–15 of the root `PLAN.md` beyond what the MVP substrate requires
- **Mine:** no Linux/Windows support; no `docs/` rewrite (only `agent-os-microkernel-mvp-buildpack/`
  is edited in the gap-closure wave); no performance tuning beyond correctness; no revisiting the
  30 frozen decisions; no renumbering of task IDs

## Capability map

Fifteen modules. Each is independently testable, has its own consumers, and could be cut without
rewriting the others. The foundation module carries the gap closure as its first wave.

| Module id | Responsibility | Depends on |
|---|---|---|
| foundation | Gap closure (A1–C10 as normative pack addenda), workspace, toolchain, CI, contract codegen, domain types, errors, testkit, observability, module-root ownership | — |
| persistence | Inception schema, transaction interfaces, SQLite repos, daemon lock and fencing epoch, idempotency, outbox | foundation |
| command-core | Command Coordinator as the sole linearization path | persistence |
| events | Envelopes, stream keys, cursors, Event Journal, journal-first dispatcher, live bus | command-core |
| runtime-graph | AgentSpec/Session/Task/Run, run state machine, RunGraph, readiness and claiming, cancellation epochs, startup recovery | command-core |
| effects | Effect contract policy resolver, `EffectRecord` transitions, executor leases and fencing, reconciliation and `Unknown` | runtime-graph |
| resources-sched | Durable reservations with budget delegation, one-shot CAS timers | command-core, runtime-graph |
| security | Principals, actors, delegation chains, capability engine, immutable approvals, Secrets Broker and Keychain | persistence |
| adapters | Process Supervisor with private socketpair IPC, framed protobuf protocol, content-addressed registry, capability negotiation, conformance harness, fixture effect adapter | persistence, effects |
| workspace-exec | Resource URI resolver, local workspace and Git worktrees, Workspace Coordinator leases/transfer/fork/merge, Sandbox Manager and T0, artifact store | adapters, security, runtime-graph |
| queue | `MessageQueuePort` and in-memory generation-global adapter | foundation |
| config | Config parser and schema, immutable generations with tested activation, profile resolution, frozen `ResolvedRunEnvironment` | adapters, workspace-exec, queue |
| control-api | gRPC UDS server, command/query methods, event read and subscribe, `agentctl` | config, events, security |
| loop-runtime | Fixture external AgentLoop, turn supervisor with fenced decision acceptance | adapters, runtime-graph, effects |
| verification | e2e basic/children/effects, concurrency, property, chaos, security suites, release gate | control-api, loop-runtime, workspace-exec |

**Build order:**
`foundation → persistence → command-core → {events, runtime-graph, security, queue} →
{effects, resources-sched, adapters} → workspace-exec → config → {control-api, loop-runtime} →
verification`

Each module gets its own five-phase spec directory, run in this order. Per your answer, we start
with the foundation module only — gap closure plus implementation — and revisit the remaining 14
module specs once it ships.

## Risks

| Risk | Likelihood | Blast radius | Mitigation |
|---|---|---|---|
| Closing the spec gaps surfaces a genuine architectural contradiction requiring a frozen decision to change | Medium | Whole DAG reshapes | Gap closure is foundation wave 1 and documentation-only. Any decision change comes back to you as a gate question before code exists |
| The accepted threshold values prove wrong under real contention (e.g. a 30 s effect lease is too short) | Low | Test assertions and config defaults across ~14 tasks | All values live in one normative `limits` section, so one edit moves every test and default |
| A4's ownership repair proves impossible for `crates/runtime` (13 modules, 8 tasks) | Medium | Serialization of 8 tasks that were meant to be parallel | Pre-declare all `mod` lines in FND-001 stubs. If that fails, add edges and accept the serialization — a correct sequential build beats a racing parallel one |
| Effect `Unknown` semantics (D-007, exit criterion 11) are subtle and easy to get wrong in a way tests do not catch | Medium | Correctness of the pack's central claim | Property test "never auto retry Unknown" (`dag.yaml:461`) plus the fixture adapter that deliberately crashes after its side effect (`dag.yaml:651`). This module goes early, not late |
| SQLite concurrency behaviour differs from what the specs assume, invalidating fencing or CAS tests | Medium | Persistence and every module above it | PST-003 includes a concurrent-writer contention test; `BEGIN IMMEDIATE` per `architecture/persistence.md:21`; bundled SQLite removes host variance |
| `sqlx` has no first-class `BEGIN IMMEDIATE` and its compile-time macros need `DATABASE_URL` or committed offline metadata | High | PST-002/PST-003 design, every repository signature, and CI | Decide in PST-002/PST-003: issue `BEGIN IMMEDIATE` via raw SQL through a managed transaction wrapper, and use runtime-checked queries unless offline metadata is committed. A contention test proves writer reservation |
| macOS Keychain integration tests are environment-dependent and flaky in CI | High | SEC-004 and the security suite | Test store is the default; Keychain tests gated behind a feature flag and a dedicated test keychain, per `dag.yaml:564` |
| Chaos suite (18 scenarios) is large, unowned, and lands at the end where schedule pressure is worst | High | Exit criteria 22 and 24 cannot be met | Give chaos scenarios owning tasks in the gap-closure wave, wire fault points into `testkit` at FND-005 so each scenario is a few lines rather than bespoke infrastructure |
| 59 tasks is a long runway; context loss across sessions | High | Rework, duplicated work | `TASK_STATUS.yaml` plus spec-flow `state.json`/`ledger.md` plus git history are the resume points. Trust files over recollection |

## Parallelisation forecast

Early read. Phase 3 computes the real DAG per module.

- **Independent tracks after `foundation`:** `persistence` and `queue` have no shared surface.
  After `command-core`: `events`, `runtime-graph`, and `security` are three genuinely independent
  tracks — `security` touches only `crates/identity|permissions|approvals|secrets` plus its own
  repo file. `adapters` runs alongside `runtime-graph` up to the point ADP-005 needs EFF-004.
- **Widest frontier:** after `command-core`, roughly 4 concurrent tracks; within `foundation`,
  FND-004/FND-005/FND-006 can go together once FND-003 lands.
- **Forced sequential (and why):** gap closure precedes every code task inside foundation because
  codegen depends on the proto collision being resolved. `foundation → persistence` because the
  schema depends on final domain types. Everything through `command-core` because D-004 makes it
  the sole mutation path. `config → control-api` because API-002 needs `CFG-003`'s frozen
  bindings. `verification` last by definition. Inside `kernel-store-sqlite`, PST-003 must
  establish `repos/mod.rs` before the ten downstream repo tasks, which is a new edge the pack
  lacks.

## Open questions for the user

All seven were answered on 2026-09-10; none remain open.

1. **Code location** — `agent-os/` (assumption 2). Task file paths gain that prefix.
2. **`git init`** — yes, at the `ai-harness` root (assumption 3).
3. **Threshold values** — accepted as written under "Reframed targets"; they become normative in
   the gap-closure wave.
4. **Protobuf precedence** — `mvp_control.proto` wins; `control.proto` is deleted from the pack
   snapshot and the lock is regenerated (assumption 10).
5. **Separate spec-gap stage** — rejected; the fixes are the first foundation wave (Approach).
6. **Store driver** — `sqlx` (assumption 7).
7. **Module scope** — start with foundation only, then revisit (Capability map).

The only blocker left is the approval marker below.

---

## Approval

<!-- The user writes here. The agent NEVER fills this in on their behalf. -->

**Decision:** approved
**Approved by:** jainamshah (recorded from explicit chat instruction)
**Date:** 2026-09-11
