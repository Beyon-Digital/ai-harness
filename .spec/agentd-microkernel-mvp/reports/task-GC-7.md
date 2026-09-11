# Task GC-7 — Extend the validator, regenerate the lock and manifest

**Status:** DONE
**Owner:** agent-gc7
**Commit:** `809e8de` — `docs(validation): extend build-pack validator and regenerate lock [GC-7]`

## What was implemented

1. Extended `agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` with the
   audit defect classes:
   - (a) **Module-root ownership** — rejects prose/`*`/`?` entries in `files` lists; for every
     shared path requires all owning tasks to be dependency-comparable (no incomparable or
     same-wave writers); requires every declared `*/Cargo.toml` to have a declared sibling
     `src/lib.rs`/`src/main.rs` and every declared module root to have its crate manifest;
     requires every `**/mod.rs` to have exactly one owning task.
   - (b) **Limits** — the 24-key required list from design GC-5 is hard-coded; every key must
     be present with a non-bool integer (or a four-digit octal file-mode string for `fs.*`),
     and `schema_version == 1`.
   - (c) **Schema coverage** — all 30 tables (including the six new design-GC-3 tables
     `artifacts`, `workspaces`, `loop_turns`, `decisions`, `adapter_instances`,
     `conformance_reports`) and `CHECK` constraints on the fifteen design-GC-3 columns.
   - (d) **DAG-source equality** — `dag.yaml` == `dag.json`; `DAG.md` mermaid nodes, edges,
     and the min-heap topological list; `tasks.csv` header, rows, order, phases, titles,
     dependencies; and every `tasks/*.md` title line, `Depends on:` link list, and
     `## Required tests` list all agree with `dag.yaml`.
   - (e) **Event-catalog equality** — ids in `contracts/events/catalog.yaml` equal ids in
     `specs/event-catalog.md` (both 61, no duplicates on either side).
   - (f) **Manifest** — `file_count` includes the manifest itself; every listed path is
     unique, exists, and matches its byte count and sha256; every pack file is recorded.
   - (g) **Duplicate proto symbols** — messages, services, and top-level enum values
     detected per package across all 32 `contracts/**/*.proto` files.
   - CLI: `--update-lock` rewrites `contracts/contract-lock.sha256` from every contract file;
     `--write-manifest` rewrites `MANIFEST.json` with the true count including itself.
   - The `BUILD PACK OK: ...` success line and exit codes (0 success, 1 invalid) are
     unchanged from the pre-GC-7 validator.
2. Created `scripts/test_validator_mutations.sh`: it requires a green baseline, copies the
   pack per case, verifies the target mutation is applied, runs the copied validator, and
   requires a non-zero exit **and** the class-specific error signature. Six cases: dropped
   table, deleted limits key, broken DAG edge, corrupted manifest hash, duplicated proto
   message in the same package, unbounded glob in a task file list. It cleans up via trap
   and exits 0 only when 6/6 are caught.
3. Regenerated the stale lock and manifest **last**, after all validator changes, with the
   new flags: `contract-lock.sha256` now covers 32 files (drops `control-api/control.proto`,
   adds `control-api/commands.proto` and all GC-1/GC-2/GC-5 hashes); `MANIFEST.json` now
   records 159 files + itself = 160, including `specs/limits.yaml`,
   `specs/event-journal-schema.sql`, and `scripts/sync_dag_sources.py`.

## Acceptance criteria

| # | Criterion | Result | Evidence |
|---|---|---|---|
| R1.4 / N3 | every changed contract file hash is in the regenerated lock; a lock mismatch fails | met | `--update-lock` wrote 32 entries; mutated-manifest/hash checks fail; lock covers every `contracts/` file except itself and plain validation passes |
| R12.1 | validator fails on missing module roots, missing limit values, and manifest drift | met | mutation cases `add_unbounded_glob`, `drop_limits_key`, `corrupt_manifest_hash` (plus missing-root logic) all exit 1 with the expected error |
| R12.2 | `MANIFEST.json` records the true file count and a verifying hash for every listed file | met | `python3 -c "... m['file_count']==len(m['files'])+1"` prints `True` (160 / 159) and every entry's byte count + sha256 is verified on each plain run |
| G1 | the repo validator still passes | met | `python3 tools/validate_repo.py` prints `OK: 241 markdown, 13 canonical ports, ...` |
| G2 | the DAG equality check catches an added or removed task id or edge | met | `break_dag_edge` mutation exits 1 with a `DAG.md` error; task-doc id/title/test/dependency mismatches and `tasks.csv`/`dag.json` drift are checked as well |
| — | Mutation script exits 0 and prints one line per caught mutation | met | `6/6 caught`, script exit 0 |
| — | No file outside `files:` changed | met | commit `809e8de` touches only the four leased paths; re-check `git status` after commit is clean for the pack |

## Verification output

```text
$ bash agent-os-microkernel-mvp-buildpack/scripts/test_validator_mutations.sh
CAUGHT drop_table (exit 1)
CAUGHT drop_limits_key (exit 1)
CAUGHT break_dag_edge (exit 1)
CAUGHT corrupt_manifest_hash (exit 1)
CAUGHT duplicate_proto_message (exit 1)
CAUGHT add_unbounded_glob (exit 1)
mutations: 6/6 caught
mutation-script-exit=0

$ python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py
BUILD PACK OK: 59 tasks, 111 markdown files, contracts locked
validator-exit=0

$ python3 tools/validate_repo.py
OK: 241 markdown, 13 canonical ports, no link/schema/catalog errors
repo-exit=0

$ python3 -c "import json; m=json.load(open('agent-os-microkernel-mvp-buildpack/MANIFEST.json')); print(m['file_count'], len(m['files']), m['file_count']==len(m['files'])+1)"
160 159 True

$ python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py --update-lock
wrote contracts/contract-lock.sha256 (32 entries)

$ python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py --write-manifest
wrote MANIFEST.json (159 files + itself = 160)
```

## Files changed

- `agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` (checks a–g, update flags)
- `agent-os-microkernel-mvp-buildpack/scripts/test_validator_mutations.sh` (new, 134 lines)
- `agent-os-microkernel-mvp-buildpack/contracts/contract-lock.sha256` (regenerated)
- `agent-os-microkernel-mvp-buildpack/MANIFEST.json` (regenerated, 160)

## Concerns

- **Failure exit code is 1, not 2.** The task context mentioned asserting exit 2, but the
  controller's global constraint requires preserving the existing exit codes; the pre-GC-7
  validator exited 1 (as recorded in GC-6's report). The mutation script therefore asserts
  non-zero exit **plus** the expected error signature, which is strictly stronger than a
  bare exit-code check.
- **Ownership interpretation.** Intentional stub-then-body sharing exists in `dag.yaml`
  (51 paths, e.g. FND-001 + body task), so "exactly one owning task" is enforced as a chain:
  all owners must be dependency-comparable, no same-wave owners, and each `mod.rs`/crate-root
  module file keeps its declared introducer. This matches GC-6's R11.3/R11.4 scan.
- **Update flags do not validate.** `--update-lock` / `--write-manifest` rewrite and exit 0;
  the plain invocation is the enforcing one, matching the prescribed command sequence.
- **Limits values.** The validator checks required-key presence and numeric/mode shape, not
  the exact threshold values; exact-value parity with design GC-5 remains covered by GC-5's
  evidence (R10.1).
