# Task GC-3 — Complete the inception schema and durable records

- **Status:** DONE_WITH_CONCERNS
- **Agent:** agent-gc3
- **Commit:** `1cdb6c2` `docs(schema): complete inception schema and durable records [GC-3]`
- **Branch:** `feat/agentd-microkernel-mvp`

## What was implemented

1. **PRAGMA block (R6.3):** `kernel-store-schema.sql` header now carries the normative block from
   `architecture/persistence.md:12-19`:
   `foreign_keys=ON`, `journal_mode=WAL`, `synchronous=FULL`, `busy_timeout=5000`.
2. **Schema version seed (R5.2):** after `kernel_meta`, the schema inserts
   `('schema_version', '1')` (`INSERT OR IGNORE`, value stored as text `1`).
3. **Six new durable tables (R5.1):** `artifacts`, `workspaces`, `loop_turns`, `decisions`,
   `adapter_instances`, `conformance_reports` with exactly the fields listed in `design.md`
   Data model, including FKs to `runs`/`effects`, `workspaces` self-FK, and
   `adapter_registrations` identity FKs.
4. **Five new columns:** `runs.output_ref`, `runs.current_turn_id`, `runs.claim_daemon_epoch`,
   `effects.daemon_fencing_epoch`, `timers.claim_daemon_epoch` (R6.2).
5. **Fifteen CHECK constraints (R6.1):** `runs.state`, `runs.recovery_disposition`,
   `effects.state`, `effects.effect_class`, `effects.idempotency_semantics`,
   `effects.reconciliation_semantics`, `workspace_leases.mode`,
   `workspace_leases.enforcement_state`, `outbox_events.sensitivity`,
   `outbox_events.retention`, `adapter_registrations.trust_state`,
   `adapter_registrations.conformance_state`, `config_generations.validation_state`,
   `config_generations.test_state`, `run_dependencies.dependency_condition`. Integer ranges carry
   comment mappings from `contracts/domain/core.proto`, `contracts/domain/effects.proto`, and
   `contracts/events/event.proto` (sensitivity/retention); `UNSPECIFIED=0` is rejected.
6. **Immutability triggers (R5.4):** `BEFORE UPDATE` + `BEFORE DELETE` triggers raising
   `immutable record` on `resolved_run_environments`, `resolved_bindings`, `agent_specs`,
   `approval_requests`, `conformance_reports`; an `outbox_events` `BEFORE UPDATE` trigger blocks
   every column except `journal_published_at_ms` and `live_published_at_ms`.
7. **Event journal extraction:** new self-contained `specs/event-journal-schema.sql` carries the
   DDL from `specs/event-pipeline.md:24-35` verbatim (events.db). `event-pipeline.md` was not
   edited (not in the file lease); the relationship is documented in `kernel-store.md`.
8. **Companion specs updated:** `kernel-store.md` (table inventory + only-this-schema/version
   rule + event-journal relationship), `run-graph.md` (D16 head-row creation in the task-creation
   transaction), `artifacts.md` (metadata → `artifacts`), `workspace.md` (identity/base
   revision/fork lineage → `workspaces`), `process-supervisor.md` (durable `adapter_instances`
   record), `adapter-registry.md` (conformance report binds to adapter id/version/digest).

## Acceptance criteria

| Criterion | Status | Demonstration |
|---|---|---|
| R5.1 — six tables exist covering artifacts, workspaces, loop turns, decisions, adapter instances, conformance reports | **met** | `sqlite3 ":memory:" \".read .../kernel-store-schema.sql\" \"SELECT name FROM sqlite_master WHERE type='table' AND name IN ('artifacts','workspaces','loop_turns','decisions','adapter_instances','conformance_reports') ORDER BY name;\"` → all six rows (evidence below) |
| R5.2 / R5.3 — schema version row exists; inventory states only this schema creates tables | **met** | `... \"SELECT key, value, typeof(value) FROM kernel_meta;\"` → `schema_version\|1\|text`; `kernel-store.md` §Inception schema and version rule |
| R5.4 — immutability triggers block updates and deletes on the five record types | **met** | 11 triggers in `sqlite_master`; UPDATE probe raised `immutable record (19)`; DELETE on `agent_specs` raised `immutable record (19)`; outbox payload UPDATE blocked while publication-metadata UPDATE succeeded |
| R6.1 — out-of-domain state rejected by SQLite | **met** | runs `state=999` probe exits 19 with `CHECK constraint failed: state BETWEEN 1 AND 11`; negative probes for `artifacts.sensitivity`, `decisions.decision_type`, `workspace_leases.enforcement_state`, `config_generations.validation_state`, `outbox_events.sensitivity` all exit 19 |
| R6.2 — fencing-epoch columns on `runs`, `effects`, `timers` | **met** | `pragma_table_info` counts → `3` (`output_ref`,`current_turn_id`,`claim_daemon_epoch`), `1` (`effects.daemon_fencing_epoch`), `1` (`timers.claim_daemon_epoch`) |
| R6.3 — normative PRAGMA block present | **met** | `grep -n PRAGMA specs/kernel-store-schema.sql` → lines 3–6 |
| R6.4 — owner-only file modes declared in `limits.yaml` (GC-5) and referenced, not redefined | **met** | `specs/limits.yaml:33-35` (`db_file_mode "0600"`, `runtime_dir_mode "0700"`, `control_socket_mode "0600"`); `architecture/persistence.md:21` references `limits.yaml`; schema defines no file modes |
| No file outside `files:` changed | **met** | `git show --stat 1cdb6c2` lists only the eight leased paths |

## Verification

### Bootstrap
```
$ sqlite3 ":memory:" ".read agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql"; echo "bootstrap exit=$?"
memory
5000
bootstrap exit=0
```

### Schema version, tables, fencing columns
```
$ sqlite3 ":memory:" ".read .../kernel-store-schema.sql" \
    "SELECT key, value, typeof(value) FROM kernel_meta;" \
    "SELECT name FROM sqlite_master WHERE type='table' AND name IN ('artifacts','workspaces','loop_turns','decisions','adapter_instances','conformance_reports') ORDER BY name;" \
    "SELECT count(*) FROM pragma_table_info('runs') WHERE name IN ('output_ref','current_turn_id','claim_daemon_epoch');" \
    "SELECT count(*) FROM pragma_table_info('effects') WHERE name='daemon_fencing_epoch';" \
    "SELECT count(*) FROM pragma_table_info('timers') WHERE name='claim_daemon_epoch';"
memory
5000
schema_version|1|text
adapter_instances
artifacts
conformance_reports
decisions
loop_turns
workspaces
3
1
1
```

### Out-of-domain CHECK probe (R6.1)
```
$ sqlite3 ":memory:" ".read .../kernel-store-schema.sql" \
    "INSERT INTO runs (run_id, task_id, state, recovery_disposition, created_at_ms, updated_at_ms) VALUES ('r','t',999,1,0,0);"
memory
5000
Error: stepping, CHECK constraint failed: state BETWEEN 1 AND 11 (19)
probe exit=19
```
The FK to `tasks` did not fire first, so the pure task-text probe fails on the CHECK as required.
Additional negative probes (same shape, exit 19 each): artifact `sensitivity=0`,
`decision_type='cancel'`, `enforcement_state='stale'`, `validation_state='bogus'`,
outbox `sensitivity=0`.

### Immutability trigger probe (R5.4)
```
$ sqlite3 ":memory:" ".read .../kernel-store-schema.sql" \
    "INSERT INTO resolved_run_environments (... 'e' ...); UPDATE resolved_run_environments SET kernel_version='x' WHERE environment_id='e';"
Error: stepping, immutable record (19)
$ sqlite3 ... "INSERT INTO agent_specs (...); DELETE FROM agent_specs WHERE agent_spec_id='s';"
Error: stepping, immutable record (19)
$ sqlite3 ... "INSERT INTO outbox_events (...); UPDATE outbox_events SET payload=x'01' WHERE event_id='ev';"
Error: stepping, immutable record (19)
$ sqlite3 ... "INSERT INTO outbox_events (...); UPDATE outbox_events SET journal_published_at_ms=5, live_published_at_ms=5 WHERE event_id='ev'; SELECT ...;"
ev|5|5
```

### Fifteen CHECK-constrained columns
```
$ for t in runs effects workspace_leases outbox_events adapter_registrations config_generations run_dependencies; do
    sqlite3 ":memory:" ".read .../kernel-store-schema.sql" "SELECT sql FROM sqlite_master WHERE type='table' AND name='$t';" | grep CHECK; done
== runs
  state INTEGER NOT NULL CHECK (state BETWEEN 1 AND 11),
  recovery_disposition INTEGER NOT NULL CHECK (recovery_disposition BETWEEN 1 AND 6),
== effects
  effect_class INTEGER NOT NULL CHECK (effect_class BETWEEN 1 AND 4),
  idempotency_semantics INTEGER NOT NULL CHECK (idempotency_semantics BETWEEN 1 AND 4),
  reconciliation_semantics INTEGER NOT NULL CHECK (reconciliation_semantics BETWEEN 1 AND 5),
  state INTEGER NOT NULL CHECK (state BETWEEN 1 AND 8),
== workspace_leases
  mode INTEGER NOT NULL CHECK (mode BETWEEN 1 AND 4),
  enforcement_state TEXT NOT NULL CHECK (enforcement_state IN ('active','revoked')),
== outbox_events
  sensitivity INTEGER NOT NULL CHECK (sensitivity BETWEEN 1 AND 4),
  retention INTEGER NOT NULL CHECK (retention BETWEEN 1 AND 4),
== adapter_registrations
  trust_state TEXT NOT NULL CHECK (trust_state IN ('trusted','untrusted')),
  conformance_state TEXT NOT NULL CHECK (conformance_state IN ('untested','passed','failed')),
== config_generations
  validation_state TEXT NOT NULL CHECK (validation_state IN ('proposed','validated','rejected')),
  test_state TEXT NOT NULL CHECK (test_state IN ('untested','passed','failed')),
== run_dependencies
  dependency_condition TEXT NOT NULL CHECK (dependency_condition IN ('completed_successfully','any_terminal','completed_or_cancelled')),
```

### Event journal schema
```
$ sqlite3 ":memory:" ".read .../event-journal-schema.sql" \
    "INSERT INTO events (event_id,stream_key,sequence,event_type,event_version,occurred_at_ms,envelope) VALUES ('e','run/r',1,'T',1,0,x'00'); SELECT event_id,stream_key,sequence FROM events;"
e|run/r|1
journal-bootstrap exit=0
```

### Table-count verification
```
$ grep -c '^CREATE TABLE' agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql
30
```
**Not met as written:** the task says this must report `31 (25 existing plus 6 new)`. The committed
baseline (`aba14a5`) contains **24** `CREATE TABLE` statements, not 25; adding exactly the six tables
mandated by `design.md` Data model gives **30**. No seventh table exists in the design, so none was
invented. See concerns.

### Repo validator
```
$ python3 tools/validate_repo.py
OK: 232 markdown, 13 canonical ports, no link/schema/catalog errors
```

## Files changed (commit `1cdb6c2`)

| Path | Change |
|---|---|
| `agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql` | modify: PRAGMAs, meta seed, 6 tables, 5 columns, 15 CHECKs, 11 triggers |
| `agent-os-microkernel-mvp-buildpack/specs/event-journal-schema.sql` | create: events.db DDL extracted verbatim |
| `agent-os-microkernel-mvp-buildpack/specs/kernel-store.md` | modify: table inventory + version rule + journal relationship |
| `agent-os-microkernel-mvp-buildpack/specs/run-graph.md` | modify: D16 head-row creation |
| `agent-os-microkernel-mvp-buildpack/specs/artifacts.md` | modify: durable `artifacts` mapping |
| `agent-os-microkernel-mvp-buildpack/specs/workspace.md` | modify: durable `workspaces` record |
| `agent-os-microkernel-mvp-buildpack/specs/process-supervisor.md` | modify: durable `adapter_instances` record |
| `agent-os-microkernel-mvp-buildpack/specs/adapter-registry.md` | modify: conformance report binding |

## Concerns

1. **Table-count criterion is off by one in the task text.** Baseline is 24 `CREATE TABLE` statements
   (verified against `git show aba14a5:...kernel-store-schema.sql | grep -c "CREATE TABLE"` = 24), so
   24 + 6 = 30, not 31. If the intent was to count `kernel-store-schema.sql` plus the newly extracted
   `event-journal-schema.sql` (`events`), the combined count is 31 (30 + 1). No extra table was added
   because none is specified anywhere in the Data model and inventing one would violate the file lease's
   "exactly the fields listed" instruction.
2. **TEXT CHECK domains for state columns are not machine-defined anywhere in the pack.** `trust_state`,
   `conformance_state`, `validation_state`, `test_state`, `enforcement_state`, `loop_turns.state`,
   `adapter_instances.state`, `conformance_reports.result`, and `decisions.decision_type` have no proto
   enum. The literals chosen follow the vocabulary in the owning specs (`workspace.md`, `config-engine.md`,
   `adapter-registry.md`, `runtime-manager.md`, `command-catalog.md`). If a later task (e.g. GC-7 or the
   domain models) fixes different canonical strings, these CHECK lists must be aligned.
3. **Consequence for `approval_requests`:** the mandated immutable triggers block the
   `RespondApproval` state update on `approval_requests`; the response lives in `approval_responses`.
   This follows the design literally but means the request row's `state`/`resolved_at_ms` columns can
   never change after insert.
4. The pack-level `scripts/validate_buildpack.py` currently fails on contract-lock mismatches caused by
   GC-1's contract edits (lock regeneration is GC-1's/GC-7's file lease, not this task's). The required
   repo-level validator prints `OK`.

---

## Fix report (review follow-up)

- **Fix commit:** `b0e6ff3` `fix(schema): pin canonical decision types and text state literals [GC-3]`
- **Findings addressed:** 1 (decision_type literals), 2 (TEXT state vocabularies in companion specs),
  3 (table-count reconciliation in this report)

### Finding 1 — canonical `decisions.decision_type` literals

`kernel-store-schema.sql` now checks the exact LoopDecision message names from
`contracts/protocols/agent_loop.proto`:

```
  -- Persisted literals are the canonical LoopDecision message names
  -- (contracts/protocols/agent_loop.proto; specs/command-catalog.md:94-101).
  decision_type TEXT NOT NULL CHECK (decision_type IN ('Complete','Fail','Wait','SpawnAgent','InvokeEffect','RequestApproval')),
```

Invalid-value probe (rejection):

```
$ sqlite3 ":memory:" ".read agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql" \
    "INSERT INTO tasks (...); INSERT INTO runs (...); INSERT INTO loop_turns (...,'issued',...); \
     INSERT INTO decisions (..., decision_type ...) VALUES ('d1','r','tn','cancel',...);"
memory
5000
Error: stepping, CHECK constraint failed: decision_type IN ('Complete','Fail','Wait','SpawnAgent','InvokeEffect','RequestApproval') (19)
invalid-decision-type exit=19
```

Canonical-value probe (accepted):

```
$ sqlite3 ... "INSERT ... VALUES ('d1','r','tn','InvokeEffect',...); SELECT decision_id, decision_type FROM decisions;"
memory
5000
d1|InvokeEffect
valid-decision-type exit=0
```

### Finding 2 — persisted TEXT state literals pinned in owning specs

`specs/kernel-store.md` gained a "Persisted TEXT state literals for the GC-3 tables" table covering
all four new TEXT-state columns; `specs/process-supervisor.md` and `specs/adapter-registry.md` now
name their literals as persisted values. Schema versus spec text:

```
$ grep -n "decision_type IN\|state IN ('issued'\|state IN ('starting'\|result IN ('pass'" specs/kernel-store-schema.sql
411:  state TEXT NOT NULL CHECK (state IN ('issued','accepted','stale')),
423:  decision_type TEXT NOT NULL CHECK (decision_type IN ('Complete','Fail','Wait','SpawnAgent','InvokeEffect','RequestApproval')),
445:  state TEXT NOT NULL CHECK (state IN ('starting','ready','exited','failed')),
461:  result TEXT NOT NULL CHECK (result IN ('pass','fail')),

$ grep -n "issued\|Complete\|starting\|pass" specs/kernel-store.md
50:| `loop_turns.state` (specs/runtime-manager.md) | `issued`, `accepted`, `stale` |
51:| `decisions.decision_type` (contracts/protocols/agent_loop.proto) | `Complete`, `Fail`, `Wait`, `SpawnAgent`, `InvokeEffect`, `RequestApproval` |
52:| `adapter_instances.state` (specs/process-supervisor.md) | `starting`, `ready`, `exited`, `failed` |
53:| `conformance_reports.result` (specs/adapter-registry.md) | `pass`, `fail` |

$ grep -n "starting" specs/process-supervisor.md
8:... `state` with persisted literals `starting`, `ready`, `exited`, `failed`, ...

$ grep -n "pass" specs/adapter-registry.md
29:... `result` with persisted literals `pass`, `fail`, ...
```

### Finding 3 — table-count reconciliation

The original report body above is unchanged. Its "Table-count verification" line ("**Not met as
written**" against `31`) was written before the task text was corrected. Reconciliation: the
committed baseline at `aba14a5` has **24** `CREATE TABLE` statements, GC-3 adds **6**, so the
correct total is **30**. With the corrected expectation, the count verification is **met** at 30;
the off-by-one concern is withdrawn.

```
$ grep -c '^CREATE TABLE' agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql
30
```

### Re-run verification after fixes

```
$ sqlite3 ":memory:" ".read agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql"; echo "bootstrap exit=$?"
memory
5000
bootstrap exit=0

$ python3 tools/validate_repo.py
OK: 234 markdown, 13 canonical ports, no link/schema/catalog errors
```

### Files changed in `b0e6ff3`

| Path | Change |
|---|---|
| `agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql` | `decisions.decision_type` CHECK → canonical message names |
| `agent-os-microkernel-mvp-buildpack/specs/kernel-store.md` | added persisted TEXT state literal table (four GC-3 columns) |
| `agent-os-microkernel-mvp-buildpack/specs/process-supervisor.md` | `adapter_instances.state` persisted literals documented |
| `agent-os-microkernel-mvp-buildpack/specs/adapter-registry.md` | `conformance_reports.result` persisted literals documented |

### Open concerns after fixes

- None new. The prior concerns about TEXT domains are resolved by finding 2. The `approval_requests`
  immutability observation still stands as a design consequence, not a defect.

