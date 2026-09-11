# Tasks — agentd-microkernel-mvp

**Date:** 2026-09-11
**Requirements:** `requirements.md` (approved)
**Design:** `design.md` (approved)

## Global constraints

- Code root: all Rust lives under `agent-os/`; all pack edits under `agent-os-microkernel-mvp-buildpack/`.
- Build (from `agent-os/`): `cargo check --workspace`
- Test (from `agent-os/`): `cargo test --workspace`
- Lint (from `agent-os/`): `cargo clippy --workspace --all-targets -- -D warnings`
- Format (from `agent-os/`): `cargo fmt --check`
- Pack validator (from repo root): `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py`
- Repo validator (from repo root): `python3 tools/validate_repo.py`
- Never: weaken a kernel invariant to pass a test · use `unwrap()`/`expect()` outside tests and proven startup invariants · put secrets in logs · inject raw global env into adapters · auto-retry an Unknown effect · edit the repo-root `spec/` or `docs/` trees.
- Exact values: every threshold, ID format, wire name, and schema field comes from `design.md` or the build pack. Invent nothing.

## Execution contract for subagents

1. **Claim before writing.** `specflow claim` is atomic and refuses a task whose files are already held.
2. **Stay inside your `files:` list.** Those paths are your lease. Need another file? `specflow block` and report.
3. **Your `depends_on` interfaces are contracts.** Consume the exact signatures in `design.md`; do not redesign a neighbour's interface.
4. **Test before you report.** Focused test first, full suite once before committing. Report the real command and output.
5. **Report honestly:** `DONE` · `DONE_WITH_CONCERNS` · `BLOCKED` · `NEEDS_CONTEXT`.
6. **Never dispatch your own reviewer.**
7. **Commit on completion**, scoped to your files, message ending with the task id. If `git commit` hits `index.lock`, wait two seconds and retry, up to five times.

---

### Task GC-1: Unify the contract snapshot

- status: done
- owner: agent-gc1
- depends_on: none
- files: `agent-os-microkernel-mvp-buildpack/contracts/control-api/control.proto`, `agent-os-microkernel-mvp-buildpack/contracts/control-api/commands.proto`, `agent-os-microkernel-mvp-buildpack/contracts/control-api/mvp_control.proto`, `agent-os-microkernel-mvp-buildpack/contracts/protocols/agent_loop.proto`, `agent-os-microkernel-mvp-buildpack/contracts/domain/core.proto`, `agent-os-microkernel-mvp-buildpack/contracts/README.md`, `agent-os-microkernel-mvp-buildpack/SOURCE_CORRECTIONS.md`
- requirements: R1.1, R1.2, R1.3, R2.1, R2.3, N3
- scope: medium
- model: standard

**Objective:** The contract snapshot becomes compilable and single-sourced: `control.proto` is gone, every catalogued command has a payload message, loop decisions are typed, subscription frames can carry a lag notice, and `AgentRun` can persist loop output.

**Context the implementer cannot infer:**

- `contracts/control-api/control.proto` and `mvp_control.proto` both declare `package agentos.spec.v1` and both define `CommandRequest`, `CommandResponse`, `GetRunRequest/Response`, `GetEffectRequest/Response`, `ApprovalResponseRequest`, `ApprovalResponseResult`. This cannot compile. Design D1: `mvp_control.proto` is normative; delete `control.proto` from the pack snapshot only. The repo-root `spec/control-api/control.proto` is canonical architecture and must not be touched.
- Design D2: `command_type` is the fully-qualified protobuf message name. Create `commands.proto` with one message per catalogued command, named exactly as the command. The 16 in `specs/command-catalog.md` plus the two new commands from design D9: `RollbackConfigGeneration { string generation_id = 1; uint64 expected_active_revision = 2; string reason = 3; }` and `ResolveBlockedRun { string run_id = 1; string action = 2; string reason = 3; }` (action is `resume` or `cancel`). Fields for the other 16 follow the shapes written in `specs/command-catalog.md`, with optional ids as `string`, digests as `string`, payloads as `bytes`, and `agent_spec_ref` typed as the existing `VersionedRef` from `contracts/domain/core.proto`.
- `contracts/protocols/agent_loop.proto` currently carries `bytes payload` and a free-string `decision_type`. Replace that with the six typed variants from `specs/command-catalog.md:94-101`: `Complete`, `Fail`, `Wait`, `SpawnAgent`, `InvokeEffect`, `RequestApproval`, wrapped in `message LoopDecision` with the fencing tuple (`run_id`, `run_revision`, `loop_epoch`, `step_sequence`, `input_event_cursor`, `turn_id`, `decision_id`) and a `oneof decision`.
- `contracts/domain/core.proto` `AgentRun` gains `string output_ref = 13;` and `string current_turn_id = 14;` (design data model).
- `mvp_control.proto` subscription: keep `stream_key` and `after_sequence` in `SubscribeEventsRequest`, and replace the `EventEnvelope` stream element with `EventStreamFrame { oneof frame { EventEnvelope event = 1; LagNotice lag = 2; } }` plus `LagNotice { string stream_key = 1; uint64 resume_sequence = 2; }` so a lagging subscriber receives a resumable cursor (design D15).
- `contracts/README.md` must state that `mvp_control.proto` is the single normative Control API and note the snapshot inventory.
- `SOURCE_CORRECTIONS.md` must document this removal and the added files, including the previously undocumented `mvp_control.proto`, `entities.proto`, and `adapter_frames.proto`.
- Do NOT touch `contracts/contract-lock.sha256`; task GC-7 regenerates it after all contract edits.

**Steps:**

- [ ] Delete `contracts/control-api/control.proto`
- [ ] Write `contracts/control-api/commands.proto` with the 18 messages
- [ ] Rewrite the decision section of `contracts/protocols/agent_loop.proto`
- [ ] Add the two fields to `AgentRun` in `contracts/domain/core.proto`
- [ ] Update the subscription messages in `mvp_control.proto`
- [ ] Update `contracts/README.md` and `SOURCE_CORRECTIONS.md`
- [ ] Run `grep -rhoE '^(message|service) [A-Za-z0-9_]+' agent-os-microkernel-mvp-buildpack/contracts --include='*.proto' | sort | uniq -d` and confirm empty output
- [ ] Run `python3 tools/validate_repo.py` and confirm `OK`
- [ ] Commit: `docs(contracts): unify snapshot, add command payloads and typed decisions [GC-1]`

**Acceptance criteria:**

- [ ] R1.1 — no protobuf package declares a duplicate message or service name, demonstrated by the empty `uniq -d` output
- [ ] R1.2 — `control.proto` no longer exists in the pack; `mvp_control.proto` remains
- [ ] R1.3 — `mvp_control.proto` is the only Control API service definition in the snapshot
- [ ] R2.1 / R2.3 — `commands.proto` defines exactly 18 messages and `agent_loop.proto` defines the six decision messages
- [ ] N3 — no lock regeneration attempted here; GC-7 owns it
- [ ] No file outside `files:` changed (repo-root `spec/` untouched)

**Verification:**

- [ ] `grep -rhoE '^(message|service) [A-Za-z0-9_]+' agent-os-microkernel-mvp-buildpack/contracts --include='*.proto' | sort | uniq -d` prints nothing
- [ ] `python3 tools/validate_repo.py` prints `OK`
- [ ] Message-name spot check: `for n in CreateSession CreateTaskRun SubmitLoopDecision RollbackConfigGeneration ResolveBlockedRun; do grep -rq "message $n" agent-os-microkernel-mvp-buildpack/contracts/control-api/commands.proto || echo "missing $n"; done` prints nothing

---

### Task GC-2: Align the command and event catalogs

- status: done
- owner: agent-gc2
- depends_on: GC-1, GC-3, GC-5
- files: `agent-os-microkernel-mvp-buildpack/specs/command-catalog.md`, `agent-os-microkernel-mvp-buildpack/specs/command-coordinator.md`, `agent-os-microkernel-mvp-buildpack/specs/event-catalog.md`, `agent-os-microkernel-mvp-buildpack/contracts/events/catalog.yaml`, `agent-os-microkernel-mvp-buildpack/specs/README.md`
- requirements: R2.4, R8.1, R8.2, R8.3, R8.4, R9.1, R9.2, R9.3, R9.4
- scope: medium
- model: standard

**Objective:** One authoritative command set and one complete, classified event catalog, with the spec index covering the new normative files.

**Context the implementer cannot infer:**

- The command catalog currently documents 16 commands; `command-coordinator.md:44-59` lists a different 16 (`TransitionRun` and `SpawnChildRun` appear only there; `BindRun` and `MarkConfigTested` appear only in the catalog). Design D9 adds `RollbackConfigGeneration` and `ResolveBlockedRun`. Resolution: the catalog becomes the single list of 18; `TransitionRun` is not a command (state transitions happen inside other commands); `SpawnChildRun` is superseded by `CreateTaskRun` with `parent_run_id`, and loop `SpawnAgent` decisions route through `CreateTaskRun`. State this explicitly in both files.
- Every catalogued command gets a line naming its payload message as the fully-qualified name (the `agentos.spec.v1.` package prefix joined with the command name), its idempotency semantics, and its emitted events (R2.4, R8.4).
- Event catalog: `contracts/events/catalog.yaml` is the canonical machine-readable catalog; `specs/event-catalog.md` is the human companion. Add to `catalog.yaml` the ten events that exist only in the markdown: `SessionCreated`, `AgentSpecRevisionStored`, `TimerScheduled`, `TimerClaimed`, `TimerFired`, `TimerCancelled`, `ResourceReserved`, `ResourceAllocated`, `ResourceReleased`, `ResourceUnknown`. Add the "bindings audit" event `RunBound` and the four waiting/running events `RunWaitingTool`, `RunWaitingChild`, `RunWaitingHuman`, `RunStateChanged`; remove `RunPaused` and `RunResumed` (design D8). Every event needs `version`, `default_sensitivity`, `default_retention`, `stream_key`, and `produced_by` naming the transition or command that emits it (R9.1–R9.3). Sensitivity values: `public`, `internal`, `confidential`, `secret`; retention values: `ephemeral`, `standard`, `audit`.
- Normalize wire literals while you are in these files: the command catalog must use `expected_effect_state = UNKNOWN` (canonical `EffectState` literal) and the canonical decision variant names `Complete`, `Fail`, `Wait`, `SpawnAgent`, `InvokeEffect`, `RequestApproval`.
- `specs/README.md` gains entries for `limits.yaml` and `event-journal-schema.sql` (created by GC-5 and GC-3).
- Do not touch `contract-lock.sha256`; GC-7 regenerates it.

**Steps:**

- [ ] Add the two commands and payload-message lines to `command-catalog.md`
- [ ] Rewrite the command list in `command-coordinator.md` to the same 18
- [ ] Extend `contracts/events/catalog.yaml` with the missing fields and events; remove paused/resumed
- [ ] Align `specs/event-catalog.md` with the catalog and state the classification rule (raises allowed, downgrades rejected)
- [ ] Update `specs/README.md` index
- [ ] Run the event-set comparison: `python3 -c "import re,yaml; c=yaml.safe_load(open('agent-os-microkernel-mvp-buildpack/contracts/events/catalog.yaml')); ids={e['id'] for e in c['events']} if isinstance(c,dict) and 'events' in c else set(); md=set(re.findall(r'\\b([A-Z][A-Za-z]+(?:Created|Stored|Added|Advanced|Issued|Accepted|Rejected|Prepared|Claimed|Dispatched|Acknowledged|Committed|Failed|Cancelled|Unknown|Reconciled|Reserved|Allocated|Released|Scheduled|Fired|Bound|Tool|Child|Human|Changed|Granted|Transferred|Forked|Merged))\b', open('agent-os-microkernel-mvp-buildpack/specs/event-catalog.md').read())); print('catalog-only', sorted(md-ids)); print('md-only', sorted(ids-md))"` and reconcile any difference by editing the files (adjust the catalog shape in the command if the file nests differently)
- [ ] Run `python3 tools/validate_repo.py` and confirm `OK`
- [ ] Commit: `docs(specs): unify command and event catalogs [GC-2]`

**Acceptance criteria:**

- [ ] R8.1 — every state-changing Control API method maps to exactly one catalogued command (the nine RPCs in `mvp_control.proto` and the internal worker commands)
- [ ] R8.2 — `RollbackConfigGeneration` is catalogued and requires a tested generation; running runs unaffected
- [ ] R8.3 — child creation has one authoritative path, stated in both files
- [ ] R9.1 / R9.2 / R9.3 / R9.4 — every catalogued event declares version, sensitivity, retention, stream key, and producing transition; paused/resumed removed
- [ ] No file outside `files:` changed

**Verification:**

- [ ] `python3 tools/validate_repo.py` prints `OK`
- [ ] `grep -c '^## ' agent-os-microkernel-mvp-buildpack/specs/command-catalog.md` reports 18 command sections (adjust to the heading level used; the count must be 18)
- [ ] `grep -n 'RunPaused\|RunResumed' agent-os-microkernel-mvp-buildpack/specs/event-catalog.md agent-os-microkernel-mvp-buildpack/contracts/events/catalog.yaml` prints nothing

---

### Task GC-3: Complete the inception schema and durable records

- status: done
- owner: agent-gc3
- depends_on: none
- files: `agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql`, `agent-os-microkernel-mvp-buildpack/specs/event-journal-schema.sql`, `agent-os-microkernel-mvp-buildpack/specs/kernel-store.md`, `agent-os-microkernel-mvp-buildpack/specs/run-graph.md`, `agent-os-microkernel-mvp-buildpack/specs/artifacts.md`, `agent-os-microkernel-mvp-buildpack/specs/workspace.md`, `agent-os-microkernel-mvp-buildpack/specs/process-supervisor.md`, `agent-os-microkernel-mvp-buildpack/specs/adapter-registry.md`
- requirements: R5.1, R5.2, R5.3, R5.4, R6.1, R6.2, R6.3, R6.4
- scope: large
- model: capable

**Objective:** The inception schema durably stores every entity the MVP persists, enforces state domains and fencing at the storage layer, and applies its PRAGMAs and immutability rules normatively.

**Context the implementer cannot infer:**

- Add these six tables with exactly the fields listed in `design.md` Data model: `artifacts`, `workspaces`, `loop_turns`, `decisions`, `adapter_instances`, `conformance_reports`.
- Add columns: `runs.output_ref`, `runs.current_turn_id`, `runs.claim_daemon_epoch`, `effects.daemon_fencing_epoch`, `timers.claim_daemon_epoch`. Seed `kernel_meta` with a `schema_version` row whose value is the text `1` (key name `schema_version`).
- Add CHECK constraints on the fifteen columns listed in `design.md` Data model. Integer enum mappings come from `contracts/domain/core.proto` and `contracts/domain/effects.proto`; include a comment mapping each integer range to its enum names.
- Add `BEFORE UPDATE` and `BEFORE DELETE` triggers that `RAISE(ABORT, 'immutable record')` for `resolved_run_environments`, `resolved_bindings`, `agent_specs`, `approval_requests`, and `conformance_reports`. For `outbox_events`, block updates to all columns except the publication-metadata columns already described in `architecture/persistence.md:37-44`.
- Apply the PRAGMA block from `architecture/persistence.md:14-18` at the top of the schema file: `foreign_keys=ON`, `journal_mode=WAL`, `synchronous=FULL`, `busy_timeout=5000`.
- Extract the event-journal DDL currently inlined at `specs/event-pipeline.md:24-35` into `specs/event-journal-schema.sql` verbatim, and leave a pointer in `event-pipeline.md`? Do not edit `event-pipeline.md` (not in your files); instead make the new file self-contained and note its relationship in `kernel-store.md`.
- Update the five companion specs so each new durable record is described where its module lives: `kernel-store.md` (table inventory and version rule), `run-graph.md` (`run_graph_heads` row is inserted in the same transaction that creates the task — design D16), `artifacts.md` (metadata maps to the `artifacts` table), `workspace.md` (identity, base revision, fork lineage), `process-supervisor.md` (durable adapter-instance record), `adapter-registry.md` (conformance report binds to adapter id/version/digest).

**Steps:**

- [ ] Write the schema additions and triggers in `kernel-store-schema.sql`
- [ ] Create `event-journal-schema.sql`
- [ ] Update the five companion specs
- [ ] Verify bootstrap: `sqlite3 ":memory:" ".read agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql"` exits 0
- [ ] Verify a CHECK: `sqlite3 ":memory:" ".read agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql" "INSERT INTO runs (run_id, task_id, state, recovery_disposition, created_at_ms, updated_at_ms) VALUES ('r','t',999,1,0,0);"` must fail with a constraint error (create a task row first if the FK fires first)
- [ ] Verify a trigger: insert a resolved environment and attempt `UPDATE`; expect `immutable record`
- [ ] Commit: `docs(schema): complete inception schema and durable records [GC-3]`

**Acceptance criteria:**

- [ ] R5.1 — all six tables exist and cover artifacts, workspaces, loop turns, decisions, adapter instances, conformance reports
- [ ] R5.2 / R5.3 — the schema version row exists and the table inventory states that only this schema creates tables
- [ ] R5.4 — immutability triggers block updates and deletes on the five record types
- [ ] R6.1 — an out-of-domain state value is rejected by SQLite, not only by a pre-check
- [ ] R6.2 — fencing-epoch columns exist on `runs`, `effects`, and `timers`
- [ ] R6.3 — the normative PRAGMA block is in the schema
- [ ] R6.4 — owner-only file modes are declared in `limits.yaml` (GC-5) and referenced, not redefined
- [ ] No file outside `files:` changed

**Verification:**

- [ ] `sqlite3 ":memory:" ".read agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql"` exits 0
- [ ] `grep -c '^CREATE TABLE' agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql` reports 30 (24 existing plus 6 new; the audit's count of 25 wrongly included one `CREATE TABLE` from `event-pipeline.md`)
- [ ] Constraint and trigger probes above fail as expected

---

### Task GC-4: Pin encodings and complete the recovery matrix

- status: done
- owner: agent-gc4
- depends_on: none
- files: `agent-os-microkernel-mvp-buildpack/specs/types-and-ids.md`, `agent-os-microkernel-mvp-buildpack/specs/recovery-table.md`
- requirements: R3.1, R3.2, R3.3, R3.4, R4.1, R4.2, R4.3, R4.4, R4.5, R7.1, R7.2, R7.3, R7.4
- scope: medium
- model: standard

**Objective:** Every encoding two agents must agree on is specified, and every reachable restart situation maps to exactly one recovery disposition.

**Context the implementer cannot infer:**

- `types-and-ids.md` gains: the full newtype list from `design.md` interfaces (including `PrincipalId`, `ActorId`, `DeviceId`, `CommandId`, `DecisionId`, `TurnId`, `OperationId`, `AgentSpecId`, `AdapterId`, `DependencyId`, `DelegationChainId`, `ArtifactId`, `SandboxId`, `EnvironmentId`, `DaemonInstanceId`); UUIDv7 serialized as lowercase hyphenated; digests as lowercase hex SHA-256 with no prefix; adapter and protocol versions as semantic versioning; `EventCursor` format `v1:{stream_key}:{sequence}`; which envelope fields the request digest covers (all of `command_id`, `idempotency_key`, `principal_id`, `actor_id`, `device_id`, `command_type`, and the payload bytes; excludes `deadline_unix_ms`, `correlation_id`, `causation_id`); canonicalization rule for the digest input (protobuf deterministic serialization for payloads, fixed field order for the envelope).
- `recovery-table.md` becomes a decision matrix over the six dispositions in `contracts/domain/core.proto`: `NORMAL`, `NEEDS_RECONCILIATION`, `RECOVERING`, `BLOCKED_UNKNOWN_EFFECT`, `BLOCKED_MISSING_RESOURCE`, `REQUIRES_HUMAN_DECISION`. Every reachable (run state, effect state) combination must have a row, including the cases the audit found missing: effect `Acknowledged` with the run transition uncommitted; effect `Claimed` with a live lease after a daemon epoch change; run `WaitingChild` at restart; run `WaitingHuman` with an expired approval; run `Cancelling` at restart; run `Suspended` at restart; effect `Prepared` with a terminal owning run. Unmapped combinations fail closed at startup (design D7). `ResolveBlockedRun` resolves or cancels `BLOCKED_MISSING_RESOURCE` (design D9).
- `RunPaused`/`RunResumed` no longer exist (design D8), so no disposition references them.

**Steps:**

- [ ] Rewrite the encodings sections of `types-and-ids.md`, including the digest field-coverage list and the cursor grammar
- [ ] Rewrite `recovery-table.md` as the complete matrix plus the fail-closed rule and the blocked-run exit
- [ ] Enumerate run states from `contracts/domain/core.proto` and confirm each appears: `python3 -c "import re; p=open('agent-os-microkernel-mvp-buildpack/specs/recovery-table.md').read(); e=open('agent-os-microkernel-mvp-buildpack/contracts/domain/core.proto').read(); states=re.findall(r'[A-Z_]{3,}', re.search(r'enum RunState \\{(.*?)\\}', e, re.S).group(1)); print([s for s in states if not s.endswith('UNSPECIFIED') and s not in p])"` must print `[]`
- [ ] Run `python3 tools/validate_repo.py` and confirm `OK`
- [ ] Commit: `docs(specs): pin encodings and complete recovery matrix [GC-4]`

**Acceptance criteria:**

- [ ] R3.1–R3.4 — digest algorithm, coverage, canonicalization, and encoding are unambiguous
- [ ] R4.1–R4.5 — ID, version, and cursor encodings are canonical and resumable
- [ ] R7.1 — every enumerated run state appears in the matrix
- [ ] R7.2 — the matrix states that unmapped combinations fail closed
- [ ] R7.3 — `ResolveBlockedRun` is the documented exit for blocked runs
- [ ] R7.4 — `Unknown` effects are never dispatched again without an explicit decision
- [ ] No file outside `files:` changed

**Verification:**

- [ ] The run-state enumeration check above prints `[]`
- [ ] `grep -n 'NEEDS_RECONCILIATION\|REQUIRES_HUMAN_DECISION' agent-os-microkernel-mvp-buildpack/specs/recovery-table.md` shows both dispositions assigned to rows
- [ ] `python3 tools/validate_repo.py` prints `OK`

---

### Task GC-5: Publish the normative limits

- status: done
- owner: agent-gc5
- depends_on: none
- files: `agent-os-microkernel-mvp-buildpack/specs/limits.yaml`, `agent-os-microkernel-mvp-buildpack/contracts/config/agent-os.schema.json`, `agent-os-microkernel-mvp-buildpack/examples/default-config.yaml`, `agent-os-microkernel-mvp-buildpack/specs/config-engine.md`, `agent-os-microkernel-mvp-buildpack/architecture/persistence.md`
- requirements: R10.1, R10.2, R10.3
- scope: medium
- model: standard

**Objective:** Every threshold asserted anywhere has one machine-readable value, validated by the config schema and mirrored in the example config.

**Context the implementer cannot infer:**

- Create `specs/limits.yaml` with exactly the keys and values in `design.md` Normative limits. No additional keys, no floating values.
- `contracts/config/agent-os.schema.json` currently declares `policies` as a bare object (`:83`). Add a typed `limits` object with the same key paths, integer types with `minimum` values (durations `minimum: 1`, capacities `minimum: 1`, counts `minimum: 0`), and string patterns for the file modes. Do not remove `policies`; mark it deprecated in a description.
- `examples/default-config.yaml` mirrors every value from `limits.yaml` exactly.
- `specs/config-engine.md` documents the limits section, references `limits.yaml` as the source, and documents `RollbackConfigGeneration`.
- `architecture/persistence.md` replaces the word "Recommended" with normative language and points at `limits.yaml` for values.

**Steps:**

- [ ] Create `specs/limits.yaml`
- [ ] Extend `agent-os.schema.json` and `examples/default-config.yaml`
- [ ] Update `config-engine.md` and `persistence.md`
- [ ] Verify mirror: `python3 -c "import yaml; l=yaml.safe_load(open('agent-os-microkernel-mvp-buildpack/specs/limits.yaml')); d=yaml.safe_load(open('agent-os-microkernel-mvp-buildpack/examples/default-config.yaml'))['limits']; flat=lambda o,p='':{k if not p else p+'.'+k:(flat(v,p+'.'+k) if isinstance(v,dict) else v) for k,v in o.items()}; fl=flat(l); fd=flat(d); fl.pop('schema_version',None); print('missing-in-default', sorted(set(fl)-set(fd))); print('mismatched', sorted(k for k in fl if fl[k]!=fd.get(k)))"` prints two empty lists
- [ ] Run `python3 tools/validate_repo.py` and confirm `OK`
- [ ] Commit: `docs(config): publish normative limits [GC-5]`

**Acceptance criteria:**

- [ ] R10.1 — every key from the design's limits list exists in `limits.yaml` with its exact value
- [ ] R10.2 — the file is machine-readable so the GC-7 validator can fail on a missing key
- [ ] R10.3 — the config schema types the limits object and rejects unknown keys
- [ ] No file outside `files:` changed

**Verification:**

- [ ] `python3 -c "import yaml; d=yaml.safe_load(open('agent-os-microkernel-mvp-buildpack/specs/limits.yaml')); assert d['adapters']['max_frame_bytes']==4194304 and d['effects']['lease_ms']==30000 and d['approvals']['ttl_ms']==900000 and d['shutdown']['drain_deadline_ms']==30000; print('limits ok')"`
- [ ] The mirror check above prints `missing-in-default []` and `mismatched []`
- [ ] `python3 -m json.tool agent-os-microkernel-mvp-buildpack/contracts/config/agent-os.schema.json > /dev/null` exits 0

---

### Task GC-6: Repair the DAG sources and add the sync script

- status: done
- owner: agent-gc6
- depends_on: GC-1, GC-3, GC-5
- files: `agent-os-microkernel-mvp-buildpack/dag.yaml`, `agent-os-microkernel-mvp-buildpack/dag.json`, `agent-os-microkernel-mvp-buildpack/DAG.md`, `agent-os-microkernel-mvp-buildpack/tasks.csv`, `agent-os-microkernel-mvp-buildpack/scripts/sync_dag_sources.py`, `agent-os-microkernel-mvp-buildpack/tasks/FND-001.md`, `agent-os-microkernel-mvp-buildpack/tasks/FND-002.md`, `agent-os-microkernel-mvp-buildpack/tasks/API-001.md`, `agent-os-microkernel-mvp-buildpack/tasks/RUN-002.md`, `agent-os-microkernel-mvp-buildpack/tasks/CFG-002.md`
- requirements: R11.1, R11.2, R11.3, R11.4, G2
- scope: large
- model: capable

**Objective:** All DAG sources agree with `dag.yaml`, task file lists are concrete and ownership-complete, and the derived sources are generated instead of hand-maintained.

**Context the implementer cannot infer:**

- Keep exactly the 59 task identifiers and all 140 existing dependency edges (G2). You may add edges to resolve ownership, never remove one.
- `dag.yaml` fixes: FND-001 gains `crates/*/src/lib.rs` as concrete per-crate paths plus `crates/agentd/src/main.rs` and the pre-declared foundation module stubs from design D13; FND-002's prose entry `build.rs or dedicated proto crate` becomes `agent-os/crates/domain/build.rs`, its `proto/**` becomes the concrete mirror paths, and `crates/domain/src/generated/**` becomes `agent-os/crates/domain/src/generated.rs`; FND-001's goal dependency sentence changes `sqlx/rusqlite choice` to `sqlx` (design D10). API-001 gains the missing fifth test `peer principal mismatch rejected` (restores `tasks/API-001.md:35`, which justified adding it via `specs/control-api.md:33`). RUN-002's goal becomes the task-file wording about deriving ancestry from the single `runs.parent_run_id` field. CFG-002's test list order matches its task file. Replace any remaining unbounded globs with concrete paths or an explicit generated-by reference.
- `scripts/sync_dag_sources.py` regenerates `dag.json` (byte-stable JSON matching current formatting style: two-space indent, `ensure_ascii=False`, trailing newline), `DAG.md` (mermaid graph plus numbered topological order in the current format), and `tasks.csv` (same columns as today) from `dag.yaml`. It supports `--check` (exit 2 if any output differs) and default write mode.
- `tasks/README.md` is not edited; it does not enumerate fields.
- Do not edit any other `tasks/*.md`.

**Steps:**

- [ ] Write `sync_dag_sources.py`; run it and confirm `dag.json` diff is empty against the current file
- [ ] Apply the `dag.yaml` metadata fixes and regenerate the three derived files
- [ ] Update the five task briefs listed in `files`
- [ ] Run `python3 agent-os-microkernel-mvp-buildpack/scripts/sync_dag_sources.py --check` and confirm exit 0
- [ ] Run `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` and confirm `BUILD PACK OK`
- [ ] Commit: `docs(dag): repair sources and add sync script [GC-6]`

**Acceptance criteria:**

- [ ] R11.1 — `dag.yaml`, `dag.json`, `DAG.md`, `tasks.csv`, and the task briefs agree on identifiers, edges, titles, and tests
- [ ] R11.2 — no task file list contains prose or an unbounded recursive glob
- [ ] R11.3 / R11.4 — every path, including module roots, has exactly one owning task; no same-wave overlap
- [ ] G2 — all 59 identifiers present and no existing edge removed
- [ ] No file outside `files:` changed

**Verification:**

- [ ] `python3 agent-os-microkernel-mvp-buildpack/scripts/sync_dag_sources.py --check` exits 0
- [ ] `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` prints `BUILD PACK OK`
- [ ] `python3 -c "import yaml; d=yaml.safe_load(open('agent-os-microkernel-mvp-buildpack/dag.yaml')); ids={t['id'] for t in d['tasks']}; missing={'FND-001','FND-002','FND-003','FND-004','FND-005','FND-006'} - ids; print('missing', missing)"` prints `missing set()`

---

### Task GC-7: Extend the validator, regenerate the lock and manifest

- status: done
- owner: agent-gc7
- depends_on: GC-1, GC-2, GC-3, GC-4, GC-5, GC-6, GC-8
- files: `agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py`, `agent-os-microkernel-mvp-buildpack/scripts/test_validator_mutations.sh`, `agent-os-microkernel-mvp-buildpack/contracts/contract-lock.sha256`, `agent-os-microkernel-mvp-buildpack/MANIFEST.json`
- requirements: R1.4, R12.1, R12.2, N3, G1, G2
- scope: large
- model: capable

**Objective:** The validator catches the defect classes this audit found, the lock and manifest are regenerated, and a mutation test proves the validator fails on each class.

**Context the implementer cannot infer:**

- Add checks to `validate_buildpack.py`: (a) module-root ownership — every path matching a crate source file has exactly one owning task and no same-wave pair writes the same path; (b) limits — every required key from `specs/limits.yaml` is present and numeric, with the required-key list hard-coded from design GC-5; (c) schema coverage — the `CREATE TABLE` set includes the six new tables from design GC-3 and the fifteen CHECK-constrained columns; (d) DAG-source equality — `dag.yaml`, `dag.json`, `DAG.md` mermaid edges, `tasks.csv`, and every `tasks/*.md` dependency line agree on ids, edges, titles, tests (extend the existing partial test equality check); (e) event-catalog equality — ids in `contracts/events/catalog.yaml` equal those in `specs/event-catalog.md`; (f) `MANIFEST.json` count and every hash; (g) duplicate proto symbols — messages, services, and top-level enum values within a package — plus the existing CLI flags `--update-lock` (rewrite `contracts/contract-lock.sha256`) and `--write-manifest` (rewrite `MANIFEST.json` with the true count including itself).
- `scripts/test_validator_mutations.sh` copies the pack to a temporary directory, applies one mutation per defect class (drop a table name, delete a limits key, break a DAG edge, corrupt a manifest hash, duplicate a proto message, add an unbounded glob), runs the validator against the copy with `SPECFLOW`-style path override if needed or from inside the copy's parent, and asserts exit 2 for each. The script must clean up and exit non-zero if any mutation is not caught.
- Regenerate the lock and manifest LAST, after the validator changes, using the new flags. Every contract file changed by GC-1, GC-2, and GC-5 must be reflected (R1.4, N3).
- Keep the existing `BUILD PACK OK` success line and exit codes.

**Steps:**

- [ ] Extend `validate_buildpack.py` with checks (a)–(h)
- [ ] Write `scripts/test_validator_mutations.sh`; run it and confirm every mutation is caught
- [ ] Run `python3 scripts/validate_buildpack.py --update-lock && python3 scripts/validate_buildpack.py --write-manifest`
- [ ] Run `python3 scripts/validate_buildpack.py` and confirm `BUILD PACK OK`
- [ ] Run `python3 tools/validate_repo.py` and confirm `OK`
- [ ] Commit: `docs(validation): extend build-pack validator and regenerate lock [GC-7]`

**Acceptance criteria:**

- [ ] R1.4 / N3 — every changed contract file hash is in the regenerated lock; a lock mismatch fails
- [ ] R12.1 — the validator fails on missing module roots, missing limit values, and manifest drift
- [ ] R12.2 — `MANIFEST.json` records the true file count and a verifying hash for every listed file
- [ ] G1 — the repo validator still passes
- [ ] G2 — the DAG equality check catches an added or removed task id or edge
- [ ] Mutation script exits 0 and prints one line per caught mutation
- [ ] No file outside `files:` changed

**Verification:**

- [ ] `bash agent-os-microkernel-mvp-buildpack/scripts/test_validator_mutations.sh` exits 0
- [ ] `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` prints `BUILD PACK OK`
- [ ] `python3 tools/validate_repo.py` prints `OK`
- [ ] `python3 -c "import json; m=json.load(open('agent-os-microkernel-mvp-buildpack/MANIFEST.json')); print(m['file_count']==len(m['files'])+1 or m['file_count']==len(m['files']))"` prints `True`

---

### Task GC-8: Resolve duplicate protobuf enum value symbols

- status: done
- owner: agent-gc8
- depends_on: GC-1, GC-3, GC-4
- files: `agent-os-microkernel-mvp-buildpack/contracts/domain/effects.proto`, `agent-os-microkernel-mvp-buildpack/contracts/catalog.yaml`, `agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql`, `agent-os-microkernel-mvp-buildpack/specs/effect-coordinator.md`, `agent-os-microkernel-mvp-buildpack/specs/recovery-table.md`, `agent-os-microkernel-mvp-buildpack/SOURCE_CORRECTIONS.md`
- requirements: R1.1, R1.2
- scope: small
- model: standard

**Objective:** The contract snapshot compiles: no two top-level enums in package `agentos.spec.v1` define the same value name.

**Context the implementer cannot infer:**

- protoc rejects three package-scope duplicates: `READ_ONLY` (`WorkspaceAccessMode` in `core.proto` vs `EffectClass` in `effects.proto`), `FAILED` and `CANCELLED` (`RunState` in `core.proto` vs `EffectState` in `effects.proto`). FND-002's scratch probe confirmed renaming the effects-side values makes the whole snapshot compile.
- Rename only the effects side: `EffectClass.READ_ONLY` becomes `EFFECT_CLASS_READ_ONLY`; `EffectState.FAILED` becomes `EFFECT_STATE_FAILED`; `EffectState.CANCELLED` becomes `EFFECT_STATE_CANCELLED`. Do not touch `core.proto` or the canonical repo-root `spec/` tree.
- Update every effect-side reference in the leased files: the `effect_classes` list in `contracts/catalog.yaml`; the enum comments at `specs/kernel-store-schema.sql:172` (EffectClass) and `:189` (EffectState); the `EffectClass` rendering at `specs/effect-coordinator.md:11`; the settled effect states at `specs/recovery-table.md:16`.
- Leave run-state and workspace-mode references unchanged: `specs/kernel-store-schema.sql:80-81` and `:149`, `specs/workspace.md:12`, the `workspace_modes` list in `contracts/catalog.yaml`, and the error code at `specs/error-model.md:43` are not effect-side.
- Document the correction in `SOURCE_CORRECTIONS.md` next to the version-const corrections: the canonical `spec/` tree keeps the unprefixed names; this pack snapshot diverges so protobuf code generation works.
- Do not touch `contract-lock.sha256` (task GC-7 regenerates it).

**Steps:**

- [ ] Rename the three enum values and update every effect-side reference in the leased files
- [ ] Run this duplicate scan and expect zero output:
      `python3 -c "import re,pathlib,collections; vals=collections.defaultdict(list); [ [vals[(m.group(1) if (m:=re.search(r'^package\\s+([\\w.]+)\\s*;', p.read_text(), re.M)) else '', v.group(1))].append(str(p)) for em in re.finditer(r'enum\\s+(\\w+)\\s*\\{(.*?)\\}', p.read_text(), re.S) for v in re.finditer(r'([A-Z][A-Z0-9_]*)\\s*=\\s*\\d+', em.group(2))] for p in pathlib.Path('agent-os-microkernel-mvp-buildpack/contracts').rglob('*.proto')]; print([k for k,l in vals.items() if len(l)>1])"`
- [ ] Run `python3 tools/validate_repo.py` and confirm `OK`
- [ ] Commit: `docs(contracts): rename colliding effect enum values [GC-8]`

**Acceptance criteria:**

- [ ] R1.1 — no duplicate message, service, or enum value symbol within a package
- [ ] R1.2 — effects-side values renamed; `core.proto` untouched
- [ ] effect-side references updated in all six leased files; run-state/workspace-mode references untouched
- [ ] No file outside `files:` changed

**Verification:**

- [ ] The duplicate scan prints `[]`
- [ ] `grep -n 'EFFECT_CLASS_READ_ONLY\|EFFECT_STATE_FAILED\|EFFECT_STATE_CANCELLED' agent-os-microkernel-mvp-buildpack/contracts/domain/effects.proto` shows all three
- [ ] `grep -c 'READ_ONLY' agent-os-microkernel-mvp-buildpack/contracts/domain/core.proto` is unchanged
- [ ] `python3 tools/validate_repo.py` prints `OK`

---

### Task FND-001: Bootstrap the Rust workspace, crates, and quality gates

- status: done
- owner: agent-fnd001
- depends_on: none
- files: `agent-os/Cargo.toml`, `agent-os/Cargo.lock`, `agent-os/rust-toolchain.toml`, `agent-os/.cargo/config.toml`, `agent-os/.github/workflows/ci.yml`, `agent-os/crates/agentd/Cargo.toml`, `agent-os/crates/agentd/src/main.rs`, `agent-os/crates/agentd/src/lock.rs`, `agent-os/crates/agentd/src/recovery.rs`, `agent-os/crates/agentd/src/api.rs`, `agent-os/crates/agentd/src/workers/mod.rs`, `agent-os/crates/agentd/src/workers/outbox.rs`, `agent-os/crates/agentd/src/workers/scheduler.rs`, `agent-os/crates/agentd/src/workers/loops.rs`, `agent-os/crates/domain/Cargo.toml`, `agent-os/crates/domain/src/lib.rs`, `agent-os/crates/domain/src/ids.rs`, `agent-os/crates/domain/src/provider.rs`, `agent-os/crates/domain/src/time.rs`, `agent-os/crates/domain/src/faults.rs`, `agent-os/crates/domain/src/run.rs`, `agent-os/crates/domain/src/effect.rs`, `agent-os/crates/domain/src/security.rs`, `agent-os/crates/domain/src/resource.rs`, `agent-os/crates/domain/src/generated.rs`, `agent-os/crates/errors/Cargo.toml`, `agent-os/crates/errors/src/lib.rs`, `agent-os/crates/errors/src/codes.rs`, `agent-os/crates/testkit/Cargo.toml`, `agent-os/crates/testkit/src/lib.rs`, `agent-os/crates/testkit/src/clock.rs`, `agent-os/crates/testkit/src/ids.rs`, `agent-os/crates/testkit/src/faults.rs`, `agent-os/crates/testkit/src/process.rs`, `agent-os/crates/observability/Cargo.toml`, `agent-os/crates/observability/src/lib.rs`, `agent-os/crates/observability/src/classification.rs`, `agent-os/crates/kernel-store/Cargo.toml`, `agent-os/crates/kernel-store/src/lib.rs`, `agent-os/crates/kernel-store-sqlite/Cargo.toml`, `agent-os/crates/kernel-store-sqlite/src/lib.rs`, `agent-os/crates/command-coordinator/Cargo.toml`, `agent-os/crates/command-coordinator/src/lib.rs`, `agent-os/crates/events/Cargo.toml`, `agent-os/crates/events/src/lib.rs`, `agent-os/crates/event-journal/Cargo.toml`, `agent-os/crates/event-journal/src/lib.rs`, `agent-os/crates/event-journal-sqlite/Cargo.toml`, `agent-os/crates/event-journal-sqlite/src/lib.rs`, `agent-os/crates/message-queue/Cargo.toml`, `agent-os/crates/message-queue/src/lib.rs`, `agent-os/crates/runtime/Cargo.toml`, `agent-os/crates/runtime/src/lib.rs`, `agent-os/crates/run-graph/Cargo.toml`, `agent-os/crates/run-graph/src/lib.rs`, `agent-os/crates/effects/Cargo.toml`, `agent-os/crates/effects/src/lib.rs`, `agent-os/crates/resources/Cargo.toml`, `agent-os/crates/resources/src/lib.rs`, `agent-os/crates/scheduler/Cargo.toml`, `agent-os/crates/scheduler/src/lib.rs`, `agent-os/crates/identity/Cargo.toml`, `agent-os/crates/identity/src/lib.rs`, `agent-os/crates/permissions/Cargo.toml`, `agent-os/crates/permissions/src/lib.rs`, `agent-os/crates/approvals/Cargo.toml`, `agent-os/crates/approvals/src/lib.rs`, `agent-os/crates/secrets/Cargo.toml`, `agent-os/crates/secrets/src/lib.rs`, `agent-os/crates/process-supervisor/Cargo.toml`, `agent-os/crates/process-supervisor/src/lib.rs`, `agent-os/crates/adapter-registry/Cargo.toml`, `agent-os/crates/adapter-registry/src/lib.rs`, `agent-os/crates/adapter-protocol/Cargo.toml`, `agent-os/crates/adapter-protocol/src/lib.rs`, `agent-os/crates/resource-uri/Cargo.toml`, `agent-os/crates/resource-uri/src/lib.rs`, `agent-os/crates/workspace/Cargo.toml`, `agent-os/crates/workspace/src/lib.rs`, `agent-os/crates/sandbox/Cargo.toml`, `agent-os/crates/sandbox/src/lib.rs`, `agent-os/crates/artifacts/Cargo.toml`, `agent-os/crates/artifacts/src/lib.rs`, `agent-os/crates/config-engine/Cargo.toml`, `agent-os/crates/config-engine/src/lib.rs`, `agent-os/crates/control-api/Cargo.toml`, `agent-os/crates/control-api/src/lib.rs`, `agent-os/crates/agentctl/Cargo.toml`, `agent-os/crates/agentctl/src/lib.rs`
- requirements: R13.1, R13.2, R13.3, N2, P2
- scope: large
- model: standard

**Objective:** A compilable 31-crate Rust workspace with pinned toolchain, CI quality gates, a placeholder `agentd` that exits cleanly, and pre-declared module roots so later tasks never edit a shared root.

**Context the implementer cannot infer:**

- Crate names and ownership come from `architecture/crate-map.md`. The workspace `Cargo.toml` declares all 31 members and `[workspace.dependencies]`: tokio, tracing, tracing-subscriber, serde, serde_json, serde_yaml, prost, prost-build, tonic, uuid (features `v7`, `serde`), sha2, sqlx (features `runtime-tokio`, `sqlite`, `macros`), thiserror, async-trait, tempfile, proptest, protoc-bin-vendored, anyhow. Use caret ranges; `Cargo.lock` is committed.
- Per-crate manifests declare only the dependencies that crate's scope needs now; foundation crates: `errors` (thiserror), `domain` (prost, uuid, serde; build-dependencies prost-build + protoc-bin-vendored), `testkit` (domain, uuid, tempfile; dev-dependencies proptest), `observability` (tracing, tracing-subscriber), `agentd` (tokio, observability). The other 26 declare their crate-map dependencies but implement nothing.
- All crates keep `#![forbid(unsafe_code)]` and a crate-level doc comment. Empty module stubs contain only a module doc comment, no deferred-work markers.
- `domain/src/lib.rs` declares `pub mod ids; pub mod provider; pub mod time; pub mod faults; pub mod run; pub mod effect; pub mod security; pub mod resource; pub mod generated;`. `errors/src/lib.rs` declares `pub mod codes;`. `testkit/src/lib.rs` declares its four modules. `observability/src/lib.rs` declares its modules. `agentd/src/main.rs` declares its modules and returns `std::process::ExitCode::SUCCESS` after doing nothing else — no database, no sockets (R13.2). The pre-declared agentd modules are empty placeholders so later tasks only fill bodies.
- The signatures these stubs will hold are in `design.md` Interfaces; do not implement them now.
- `rust-toolchain.toml` pins `1.94.0` with components `rustfmt`, `clippy`.
- `.cargo/config.toml` defines aliases `check-all`, `lint`, `test-all` wrapping the global constraint commands.
- `ci.yml` runs on macOS: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo check --workspace`, `python3 tools/validate_repo.py`, `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py`, a lock check (`python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` already covers it), and a hygiene grep that fails on `thread::sleep` or `tokio::time::sleep` inside `crates/**/tests/` and `src/**` test modules (N2). The workflow sets `working-directory` appropriately: cargo steps in `agent-os`, validator steps at the repository root.

**Steps:**

- [ ] Write the workspace manifest and toolchain files
- [ ] Create all 31 crate manifests and empty lib/main files
- [ ] Create the pre-declared module stubs
- [ ] Write `ci.yml`
- [ ] Run `cargo check --workspace` and confirm success
- [ ] Run `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` and confirm success
- [ ] Run `cargo run -p agentd` and confirm it exits 0 without opening anything
- [ ] Commit: `chore(workspace): bootstrap crates and quality gates [FND-001]`

**Acceptance criteria:**

- [ ] R13.1 — all four quality commands pass
- [ ] R13.2 — `agentd` exits cleanly; no database or socket code exists
- [ ] R13.3 / P2 — 31 crates, no dependency cycle (`cargo metadata` resolves)
- [ ] N2 — CI hygiene grep present
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo check --workspace && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` exits 0
- [ ] From `agent-os/`: `cargo run -q -p agentd; echo $?` prints `0`
- [ ] `python3 -c "import subprocess,json; m=json.loads(subprocess.check_output(['cargo','metadata','--format-version','1','--no-deps'],cwd='agent-os')); print(len(m['packages']))"` prints `31`

---

### Task FND-002: Install the contract mirror and hermetic code generation

- status: done
- owner: agent-fnd002
- depends_on: FND-001, GC-1, GC-3, GC-8
- files: `agent-os/proto/catalog.yaml`, `agent-os/proto/contract-lock.sha256`, `agent-os/proto/README.md`, `agent-os/proto/capabilities/adapter-capabilities.yaml`, `agent-os/proto/capabilities/security-capabilities.yaml`, `agent-os/proto/config/agent-os.schema.json`, `agent-os/proto/control-api/commands.proto`, `agent-os/proto/control-api/mvp_control.proto`, `agent-os/proto/domain/core.proto`, `agent-os/proto/domain/effects.proto`, `agent-os/proto/domain/entities.proto`, `agent-os/proto/domain/security.proto`, `agent-os/proto/events/catalog.yaml`, `agent-os/proto/events/event.proto`, `agent-os/proto/manifests/extension.schema.json`, `agent-os/proto/ports/agent_loop.proto`, `agent-os/proto/ports/artifact_store.proto`, `agent-os/proto/ports/common.proto`, `agent-os/proto/ports/context.proto`, `agent-os/proto/ports/event_journal.proto`, `agent-os/proto/ports/kernel_store.proto`, `agent-os/proto/ports/memory_store.proto`, `agent-os/proto/ports/message_queue.proto`, `agent-os/proto/ports/model.proto`, `agent-os/proto/ports/sandbox.proto`, `agent-os/proto/ports/secret_store.proto`, `agent-os/proto/ports/tool_runtime.proto`, `agent-os/proto/ports/transport.proto`, `agent-os/proto/ports/workspace.proto`, `agent-os/proto/protocols/adapter_frames.proto`, `agent-os/proto/protocols/agent_loop.proto`, `agent-os/proto/protocols/effect.proto`, `agent-os/proto/protocols/external_adapter.proto`, `agent-os/schema/kernel_store.sql`, `agent-os/schema/event_journal.sql`, `agent-os/crates/domain/build.rs`, `agent-os/crates/domain/src/generated.rs`, `agent-os/crates/domain/tests/contract_codegen.rs`
- requirements: R1.1, R14.1, R14.2, R14.3, N3, N4
- scope: large
- model: capable
- blocked_reason: need agent-os-microkernel-mvp-buildpack/contracts/domain/effects.proto: R1.1 unmet by dependency GC-1 snapshot; protoc 31.1 rejects duplicate package-scope enum values READ_ONLY (EffectClass vs WorkspaceAccessMode in core.proto) and FAILED/CANCELLED (EffectState vs RunState in core.proto); N3 forbids editing the mirrored copy; probed minimal fix (prefix the three values) makes all protos compile

**Objective:** The fixed contract snapshot is installed into the workspace and compiles deterministically with a vendored protoc, with a test that proves byte-identical regeneration.

**Context the implementer cannot infer:**

- Copy every file from `agent-os-microkernel-mvp-buildpack/contracts/` into `agent-os/proto/` preserving relative paths — including `catalog.yaml`, `contract-lock.sha256`, `README.md`, the capability YAMLs, JSON schemas, and all `.proto` files. Copy the two SQL schemas from the pack into `agent-os/schema/`: `specs/kernel-store-schema.sql` becomes `kernel_store.sql` and the new `specs/event-journal-schema.sql` becomes `event_journal.sql`. The mirror is verbatim; do not edit copied content.
- Mirror the pack only after GC-8's enum renames have landed, and take the mirror against the current pack (GC-2 finalized `events/catalog.yaml`).
- Imports inside the protos are relative to the contracts root (for example `import "domain/core.proto";`), so the prost include path is `agent-os/proto`.
- `domain/build.rs` sets `std::env::set_var("PROTOC", protoc_bin_vendored::protoc_bin_path().unwrap())` before invoking `prost_build::Config::new().out_dir(env::var("OUT_DIR")).compile_protos(&[...all proto files...], &["../proto"])` — use the API version matching the pinned prost-build, and re-run on proto changes via `cargo:rerun-if-changed=../proto`. Collect the proto file list explicitly by walking `../proto` at build time.
- Prost emits one file per protobuf package inside `OUT_DIR`. Inspect the actual filename during implementation and include it with `include!(concat!(env!("OUT_DIR"), "/FILE_NAME.rs"))` in `generated.rs`, re-exporting the package module as `pub mod contract`. If the snapshot compiles to multiple package files, include each.
- `crates/domain/tests/contract_codegen.rs` builds twice into two distinct temp dirs through a copy of the same compile routine and asserts the outputs are byte-identical; it also asserts `agentos::spec::v1::CommandRequest` and the six decision messages exist (path depends on the generated module layout; assert on the actual generated names).
- The test must not require network. Use the already-resolved vendored protoc.
- Do not edit anything copied (N3): the lock file inside the mirror is verified against the mirror by CI, not by this task.

**Steps:**

- [ ] Copy the contract mirror and SQL schemas
- [ ] Write `build.rs` and `generated.rs`; run `cargo check -p domain` and fix include names
- [ ] Write the determinism test with the failing assertion first, confirm it fails before the generator is correct
- [ ] Run `cargo test -p domain contract_codegen` and confirm pass
- [ ] Run `cargo check --workspace` and confirm the mirror did not break other crates
- [ ] Commit: `feat(domain): install contract mirror and hermetic codegen [FND-002]`

**Acceptance criteria:**

- [ ] R1.1 — the snapshot compiles with no duplicate symbols
- [ ] R14.1 — build succeeds with no system protoc (vendored path used)
- [ ] R14.2 / N4 — two generations are byte-identical
- [ ] R14.3 — no hand-written duplicate of a protobuf message struct exists
- [ ] N3 — mirror is verbatim; no copied file edited
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p domain contract_codegen` passes
- [ ] `diff -rq agent-os-microkernel-mvp-buildpack/contracts agent-os/proto` reports no differences
- [ ] `cargo check --workspace` exits 0

---

### Task FND-003: Implement domain IDs, mirror enums, and value types

- status: pending
- owner: -
- depends_on: FND-002
- files: `agent-os/crates/domain/src/ids.rs`, `agent-os/crates/domain/src/provider.rs`, `agent-os/crates/domain/src/time.rs`, `agent-os/crates/domain/src/faults.rs`, `agent-os/crates/domain/src/run.rs`, `agent-os/crates/domain/src/effect.rs`, `agent-os/crates/domain/src/security.rs`, `agent-os/crates/domain/src/resource.rs`, `agent-os/crates/domain/tests/prop_enums.rs`
- requirements: R4.2, R15.1, R15.2, R15.3, P1
- scope: large
- model: standard

**Objective:** Typed, validated domain primitives that never guess: UUIDv7 newtypes, canonical keys and cursor, time/provider/fault traits, and mirror enums whose wire conversion fails on unknown values.

**Context the implementer cannot infer:**

- Exact signatures and the full newtype list are in `design.md` Interfaces under `domain crate`. The `uuid_newtype!` macro must generate `FromStr` with `Err = InvalidId`, `Display`, `serde` support, and `as_uuid`; parsing rejects anything that is not a valid UUIDv7 (version nibble 7).
- `UuidV7::to_hyphenated` returns lowercase hyphenated form. `IdempotencyKey` rejects empty, control characters, and over 255 bytes. `EventCursor` parses and formats `v1:{stream_key}:{sequence}` where sequence is a decimal u64; malformed input returns `InvalidId`.
- Mirror enums: `run.rs` holds `RunState`, `RecoveryDisposition`; `effect.rs` holds `EffectState`, `EffectClass`, `IdempotencySemantics`, `ReconciliationSemantics`; `security.rs` holds `SensitivityClass`, `RetentionClass`, `TrustState`, `ConformanceState`, `ApprovalState`; `resource.rs` holds `WorkspaceAccessMode`, `LeaseEnforcementState`, `TimerState`, `ReservationState`, `DependencyCondition`. Each has `from_wire(i32) -> Result<Self, UnknownEnumValue>` and `to_wire(self) -> i32`, with values mapped from the integer constants in `agent-os/proto/domain/core.proto` and `effects.proto`. `UnknownEnumValue { value: i32, enum_name: &'static str }` lives in `run.rs` and is re-exported.
- Round-trip requirement: for any valid variant, `from_wire(to_wire(x)) == Ok(x)`.
- `provider.rs` defines the `IdProvider` trait and `SystemIdProvider`; `time.rs` defines `Clock` and `SystemClock`; `faults.rs` defines `FaultInjector` and `NoFaults`. The production implementations are trivial; tests inject doubles later.
- `prop_enums.rs` property test: for arbitrary `i32` values (including negatives and large), `from_wire` never returns a variant that does not correspond to the wire number — it either returns `Ok` with the exact variant or `Err` with the same value. Write one proptest per critical enum and assert `to_wire(from_wire(v)?) == v` when `Ok`.
- No `unwrap()` in library code; tests may use it.

**Steps:**

- [ ] Write the failing property test in `prop_enums.rs` first and confirm it fails to compile (types missing) — that is the RED state
- [ ] Implement IDs, traits, and enums minimally
- [ ] Run `cargo test -p domain prop_enums` and confirm pass
- [ ] Add unit tests inside `ids.rs` for `UuidV7` parse/format, invalid ID rejection, and cursor round-trip
- [ ] Run `cargo test -p domain` and confirm all pass
- [ ] Commit: `feat(domain): typed IDs, traits, and mirror enums [FND-003]`

**Acceptance criteria:**

- [ ] R4.2 — invalid identifiers rejected with `InvalidId`; typed newtypes used everywhere
- [ ] R15.1 / P1 — unknown wire values never map to a default variant, proven by the property test
- [ ] R15.2 — valid values round-trip exactly
- [ ] R15.3 — generated IDs are UUIDv7 and unique in a quick loop test
- [ ] No `unwrap()`/`expect()` in `src/`; no file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p domain` passes
- [ ] `cargo clippy -p domain --all-targets -- -D warnings` is clean
- [ ] `grep -n 'unwrap()\|expect(' agent-os/crates/domain/src/{ids,provider,time,faults,run,effect,security,resource}.rs` prints nothing

---

### Task FND-004: Implement the stable error model

- status: done
- owner: agent-fnd004
- depends_on: FND-001
- files: `agent-os/crates/errors/src/codes.rs`, `agent-os/crates/errors/src/lib.rs`, `agent-os/crates/errors/tests/codes.rs`
- requirements: R16.1, R16.2
- scope: small
- model: cheap

**Objective:** One typed error taxonomy with stable machine-readable codes and structural retry safety, used by every later kernel crate.

**Context the implementer cannot infer:**

- Exact interfaces are in `design.md` under `errors crate`. `ErrorCode::as_str` returns stable lowercase tokens: `invalid_argument`, `not_found`, `conflict`, `failed_precondition`, `resource_exhausted`, `unavailable`, `internal`.
- `RetryClass` is `Never`, `Safe`, `ReconciliationRequired`. `KernelError` carries code, retry class, message, and optional source; its `Display` prints `{code}: {message}` and never includes secret or payload content (N1).
- Integration test cases: same code produces the same string; `ReconciliationRequired` is structurally distinct from `Safe`; a `KernelError` built from an `std::io::Error` source preserves the source through `Error::source`; `Display` output contains the code token.
- No `unwrap()`/`expect()` in `src/`.

**Steps:**

- [ ] Write the failing test in `tests/codes.rs` covering the four integration cases
- [ ] Run `cargo test -p errors` and confirm it fails to compile because the API is missing
- [ ] Implement `codes.rs` and `lib.rs`
- [ ] Run `cargo test -p errors` and confirm pass
- [ ] Commit: `feat(errors): stable codes and retry classification [FND-004]`

**Acceptance criteria:**

- [ ] R16.1 — every failure carries a code and a retry class, demonstrated by `tests/codes.rs`
- [ ] R16.2 — the code is preserved across formatting and display
- [ ] N1 — `Display` never prints source content, only the message
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p errors` passes
- [ ] `cargo clippy -p errors --all-targets -- -D warnings` is clean

---

### Task FND-005: Implement the deterministic testkit

- status: pending
- owner: -
- depends_on: FND-003
- files: `agent-os/crates/testkit/src/clock.rs`, `agent-os/crates/testkit/src/ids.rs`, `agent-os/crates/testkit/src/faults.rs`, `agent-os/crates/testkit/src/process.rs`, `agent-os/crates/testkit/src/lib.rs`, `agent-os/crates/testkit/src/bin/fixture-daemon.rs`, `agent-os/crates/testkit/tests/daemon_host.rs`, `agent-os/crates/testkit/tests/prop_faults.rs`
- requirements: R17.1, R17.2, R17.3, N2, P3
- scope: large
- model: standard

**Objective:** Deterministic clock, ID factory, one-shot fault injection, and a temporary daemon host that every later crash and concurrency test uses instead of sleeps.

**Context the implementer cannot infer:**

- Exact signatures are in `design.md` under `testkit crate`. `TestClock` implements `domain::time::Clock`; `DeterministicIds` implements `domain::provider::IdProvider` producing UUIDv7 from a fixed seed timestamp plus a monotonic counter; `ArmedFaults` implements `domain::faults::FaultInjector` and each armed point triggers exactly once; `TempDaemonHost` owns a temp dir, spawns a daemon binary, exposes `home()` and `runtime_dir()`, and on Drop kills and reaps the child and removes the tree.
- `ArmedFaults` needs `assert_triggered(&self, point: &str)` that panics naming the point when it was armed but never fired (R17.2), and `is_triggered`.
- The property test `prop_faults.rs` generates arbitrary sequences of arm/trigger/re-arm over a small point set and asserts each triggered point fired exactly once per arm (P3).
- `TempDaemonHost` needs a real binary to launch in tests. Add `src/bin/fixture-daemon.rs`: a tiny binary that sleeps without touching the filesystem, accepts an optional `--exit-after-ms` argument, and exits 0. The integration test locates it via `env!("CARGO_BIN_EXE_fixture-daemon")` (available to integration tests of this crate), spawns it, and asserts clean exit; a second case kills it and asserts reaping.
- No wall-clock sleeps in tests; use the clock doubles and bounded waits on child exit with a polling loop that checks `try_wait` at short intervals is acceptable for child process reaping only.

**Steps:**

- [ ] Write the failing tests first: `test_clock_advances`, `test_ids_are_time_sorted`, `test_fault_fires_once`, `test_untriggered_fault_panics`, `test_temp_daemon_exits_cleanly`
- [ ] Run `cargo test -p testkit` and confirm RED with missing types
- [ ] Implement the four modules and the fixture binary
- [ ] Run `cargo test -p testkit` and confirm pass
- [ ] Add and run the property test `cargo test -p testkit prop_faults`
- [ ] Commit: `feat(testkit): deterministic doubles and fault injection [FND-005]`

**Acceptance criteria:**

- [ ] R17.1 — clock, IDs, and daemon host doubles exist and are deterministic, demonstrated by the named tests
- [ ] R17.2 / P3 — armed points fire exactly once; unreached armed points fail the test loudly
- [ ] R17.3 / N2 — no wall-clock sleep in test synchronization
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p testkit` passes
- [ ] `grep -rn 'thread::sleep\|tokio::time::sleep' agent-os/crates/testkit` prints nothing
- [ ] `cargo clippy -p testkit --all-targets -- -D warnings` is clean

---

### Task FND-006: Implement the classification and redaction substrate

- status: done
- owner: agent-fnd006
- depends_on: FND-001
- files: `agent-os/crates/observability/src/classification.rs`, `agent-os/crates/observability/src/lib.rs`, `agent-os/crates/observability/tests/redaction.rs`
- requirements: R18.1, R18.2, R18.3, N1
- scope: medium
- model: cheap

**Objective:** Classification, redacted wrappers, and span helpers that make secret leakage structurally impossible and attach correlation identifiers to every signal.

**Context the implementer cannot infer:**

- Exact signatures are in `design.md` under `observability crate`. `Classification` is ordered `Public < Internal < Confidential < Secret`; `Secret` is the only access path to wrapped content and `Debug`/`Display` always print `[REDACTED]`; `Redacted` prints `[REDACTED]` in `Debug` but allows access.
- `init_tracing(json: bool)` installs a `tracing_subscriber` sink; `kernel_span(&KernelFields)` returns a span carrying correlation, run, task, and effect ids.
- Integration tests `redaction.rs`: `format!("{:?}", Secret::new("hunter2"))` does not contain `hunter2`; `format!("{}", Secret::new("hunter2"))` does not contain it; `Secret::expose` returns it; `format!("{:?}", Redacted::new("x"))` prints `[REDACTED]`; ordering assertions `Public < Secret`.
- No derived `Debug` on `Secret`; implement it manually. No `unwrap()` outside tests.

**Steps:**

- [ ] Write the failing redaction tests
- [ ] Run `cargo test -p observability` and confirm RED with missing types
- [ ] Implement `classification.rs` and `lib.rs`
- [ ] Run `cargo test -p observability` and confirm pass
- [ ] Commit: `feat(observability): classification and redaction substrate [FND-006]`

**Acceptance criteria:**

- [ ] R18.1 — span helper attaches kernel field ids
- [ ] R18.2 / N1 — secret values never render through Debug or Display, demonstrated by `redaction.rs`
- [ ] R18.3 — the classification ordering allows redaction above a sink threshold
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p observability` passes
- [ ] `cargo clippy -p observability --all-targets -- -D warnings` is clean

---

## Checkpoints

| After wave | Check | Command |
|---|---|---|
| 1 | Pack edits valid; repo validator green; `agent-os` workspace compiles if FND-001 landed | `python3 tools/validate_repo.py` and, in `agent-os/`, `cargo check --workspace` |
| 2 | Derived DAG sources match; codegen deterministic; errors and observability suites pass | `python3 agent-os-microkernel-mvp-buildpack/scripts/sync_dag_sources.py --check` and, in `agent-os/`, `cargo test --workspace` |
| 3 | Full validator, lock, and manifest green | `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` and `python3 tools/validate_repo.py` |
| 4 | Foundation complete: all quality gates | In `agent-os/`: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` |

Note: `contract-lock.sha256` and `MANIFEST.json` are intentionally stale until GC-7 regenerates them in wave 3. A lock failure before then is expected and must not be "fixed" by hand.

## Rulings

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| 1 | Gap-closure work stays out of the 59-task pack DAG (spec-flow owns orchestration) | Preserves G2 and the pack's status file | Pack DAG does not mention gap closure; spec ledger carries it |
| 2 | testkit ships a `fixture-daemon` test binary so `TempDaemonHost` is self-testing | Avoids cross-crate binary-path fragility in tests | One extra tiny binary in the testkit crate |
| 3 | Lock and manifest regeneration deferred to GC-7 after all contract edits | Prevents a lock race across parallel wave-1 tasks | Contract lock is stale during waves 1–2; documented in checkpoints |
