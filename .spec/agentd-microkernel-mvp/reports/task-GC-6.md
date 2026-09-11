# Task GC-6 — Repair the DAG sources and add the sync script

**Status:** DONE
**Owner:** agent-gc6
**Commits:**
- 0706510 — `docs(dag): repair sources and add sync script [GC-6]`
- 80e37ee — `docs(dag): align FND-002 mirror paths with snapshot layout [GC-6]`

## What was implemented

1. Created `agent-os-microkernel-mvp-buildpack/scripts/sync_dag_sources.py` (D12). It reads `dag.yaml` and regenerates the three derived sources byte-stably: `dag.json` (two-space indent, `ensure_ascii=False`, trailing newline), `DAG.md` (mermaid graph plus numbered topological order; Kahn's algorithm with a min-heap keyed by task ID), and `tasks.csv` (columns `id,phase,title,depends_on`, dependencies space-joined, CRLF line endings preserved). Supports `--check` (exit 2 on any drift) and default write mode. Before any metadata edit, `--check` returned 0 against the then-current files, proving the generator reproduces all three sources byte-for-byte.
2. Replaced all twelve unusable `files` entries with concrete paths:
   - FND-001: `crates/*/Cargo.toml` became the 31 concrete crate manifests, plus every crate module root (`src/lib.rs`, or `crates/agentd/src/main.rs`) and the D13 pre-declared foundation module stubs (`domain/src/{ids,provider,time,faults,run,effect,security,resource,generated}.rs`, `errors/src/codes.rs`, `testkit/src/{clock,ids,faults,process}.rs`, `observability/src/classification.rs`, `agentd/src/{lock,recovery,api}.rs`, `agentd/src/workers/{mod,outbox,scheduler,loops}.rs`).
   - FND-002: prose `build.rs or dedicated proto crate` became `agent-os/crates/domain/build.rs`; `proto/**` became the 32 concrete mirror paths with the snapshot layout preserved (`proto/control-api/...`, matching the concurrently implemented `agent-os/proto/` mirror); `crates/domain/src/generated/**` became `agent-os/crates/domain/src/generated.rs`.
   - PST-003: `.../repos/*.rs` became `repos/mod.rs` plus the eleven concrete repository modules.
   - ADP-005 / LOOP-001: `fixtures/**` became each fixture crate's `Cargo.toml` and `src/main.rs`.
   - API-002 / API-003: `proto/control/**` and `proto/events/**` became the concrete control and event contract files.
   - INT-004 / INT-005: `tests/*/*.rs` became the concrete `main.rs` harnesses that make `cargo test --test concurrency|property` valid.
3. Applied the remaining metadata fixes: API-001 gained its missing fifth test `peer principal mismatch rejected`; RUN-002's goal now uses the task-file wording that derives ancestry from the single `runs.parent_run_id` field; CFG-002's test list order now matches its brief.
4. Updated `tasks/FND-001.md` (concrete module-root outputs plus `sqlx/rusqlite choice` → `sqlx`, D10) and `tasks/FND-002.md` (concrete codegen and mirror outputs). `tasks/API-001.md`, `tasks/RUN-002.md`, and `tasks/CFG-002.md` already agreed with the corrected `dag.yaml`, so no bytes changed there; the repair landed in `dag.yaml`/`dag.json`.
5. Regenerated `dag.json`; `DAG.md` and `tasks.csv` were already byte-identical to a fresh generation because titles, identifiers, and edges are unchanged.

No file outside the lease was written. No task identifier was added or removed, and no dependency edge was added or removed (59 identifiers, 140 edges before and after).

## Acceptance criteria

| # | Criterion | Result | Evidence |
|---|---|---|---|
| R11.1 | `dag.yaml`, `dag.json`, `DAG.md`, `tasks.csv`, and task briefs agree on identifiers, edges, titles, tests | met | brief-vs-DAG parse check over all 59 tasks prints `issues: 0`; `sync_dag_sources.py --check` exits 0 |
| R11.2 | No task file list contains prose or an unbounded recursive glob | met | scan for `*`/` or ` in every `files` entry prints `prose/unbounded-glob entries: []` |
| R11.3/R11.4 | Every path, including module roots, has exactly one owning task; no same-wave overlap | met | module roots are introduced exactly once (FND-001 for crate roots and stubs, PST-003 for `repos/mod.rs`); longest-path wave scan reports no same-wave duplicate, and every pair of tasks sharing a path is dependency-comparable |
| G2 | All 59 identifiers present and no existing edge removed | met | `tasks: 59 ids unique: 59`, `edges: 140`; `git diff` shows only `files`, `goal`, and `tests` values changed |
| — | No file outside `files:` changed | met | commits 0706510 and 80e37ee touch only leased paths |

## Verification output

```text
$ python3 agent-os-microkernel-mvp-buildpack/scripts/sync_dag_sources.py --check
DAG sources up to date
sync check exit=0

$ python3 -c "import yaml; d=yaml.safe_load(open('agent-os-microkernel-mvp-buildpack/dag.yaml')); ids={t['id'] for t in d['tasks']}; missing={'FND-001','FND-002','FND-003','FND-004','FND-005','FND-006'} - ids; print('missing', missing)"
missing set()

$ python3 - <<'EOF'   # G2 + R11.2
import yaml, json
d=yaml.safe_load(open('agent-os-microkernel-mvp-buildpack/dag.yaml'))
j=json.load(open('agent-os-microkernel-mvp-buildpack/dag.json'))
print('yaml==json:', d==j)
print('tasks:', len(d['tasks']), 'ids unique:', len({t['id'] for t in d['tasks']}))
print('edges:', sum(len(t['depends_on']) for t in d['tasks']))
bad=[(t['id'],f) for t in d['tasks'] for f in t['files'] if '*' in f or ' or ' in f]
print('prose/unbounded-glob entries:', bad)
EOF
yaml==json: True
tasks: 59 ids unique: 59
edges: 140
prose/unbounded-glob entries: []

$ python3 <brief-consistency check over all 59 tasks/*.md>
issues: 0

$ python3 <longest-path wave + path-owner scan>
same-wave overlaps: none
incomparable tasks sharing a path: none

$ python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py
BUILD PACK INVALID
 - contract digest mismatch README.md
 - contract digest mismatch config/agent-os.schema.json
 - locked contract missing control-api/control.proto
 - contract digest mismatch control-api/mvp_control.proto
 - contract digest mismatch domain/core.proto
 - contract digest mismatch events/catalog.yaml
 - contract digest mismatch protocols/agent_loop.proto
validator exit=1   # pre-existing; see Concerns

$ diff <validator run at HEAD> <validator run after GC-6>
identical error set to HEAD baseline
```

The validator failure is byte-for-byte identical to the run against a pristine `HEAD` checkout extracted to a temp directory, so GC-6 introduces no validator regression. The affected contract files and `contracts/control-lock.sha256` are outside this task's lease and are explicitly owned by GC-7 (`contract-lock.sha256` and `MANIFEST.json` regeneration); see GC-1's report: "No lock regeneration attempted (N3; GC-7 owns `contract-lock.sha256`)".

## Files changed

- `agent-os-microkernel-mvp-buildpack/dag.yaml` (concrete file lists; API-001 test; RUN-002 goal; CFG-002 test order; `proto/control-api/` mirror paths)
- `agent-os-microkernel-mvp-buildpack/dag.json` (regenerated)
- `agent-os-microkernel-mvp-buildpack/scripts/sync_dag_sources.py` (created)
- `agent-os-microkernel-mvp-buildpack/tasks/FND-001.md` (module-root outputs; `sqlx`)
- `agent-os-microkernel-mvp-buildpack/tasks/FND-002.md` (codegen and snapshot-preserving mirror outputs)

`DAG.md` and `tasks.csv` were regenerated identically to their committed bytes (no identifiers, edges, or titles changed), so they do not appear in the commit.

## Concerns

- `validate_buildpack.py` cannot print `BUILD PACK OK` until GC-7 regenerates `contracts/contract-lock.sha256` (and `MANIFEST.json`). This failure exists at the wave-1 baseline and is unchanged by GC-6; the validator error set is identical before and after.
- Two spelling conventions now coexist in implementation `files` lists: `agent-os/crates/domain/build.rs` / `agent-os/crates/domain/src/generated.rs` for FND-002 (as mandated by the task text) while other implementation paths stay relative to `agent-os/`. No ownership check is affected because no other task claims those two strings.
- `fixtures/**` and `tests/*/**` had no file inventory anywhere in the pack, so their concrete paths (`Cargo.toml` + `src/main.rs`; `main.rs` harnesses) are the minimal files that satisfy the declared test commands (`cargo test --test concurrency|property`).
- `crates/kernel-store-sqlite/src/repos/*.rs` was expanded to `mod.rs` plus the union of repository modules named by downstream tasks (agent_specs, tasks, effects, effect_claims, resources, timers, security, approvals, workspace_leases, idempotency, outbox); each remains dependency-ordered after PST-003.
