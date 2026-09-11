# Task FND-002 report: Install the contract mirror and hermetic code generation

- Status: **in review** (specflow: `FND-002 -> review (@agent-fnd002)`) after being unblocked by GC-8
- Owner: @agent-fnd002
- Commit: `0e4b01f` `feat(domain): install contract mirror and hermetic codegen [FND-002]`
- Block history: blocked once on the duplicate enum values READ_ONLY/FAILED/CANCELLED in package `agentos.spec.v1`; GC-8 renamed the effects-side values to `EFFECT_CLASS_READ_ONLY`, `EFFECT_STATE_FAILED`, and `EFFECT_STATE_CANCELLED` and the task was resumed. Details kept below.

## What was implemented before blocking

All work is inside the leased paths; nothing is committed.

1. Contract mirror installed verbatim: every file from
   `agent-os-microkernel-mvp-buildpack/contracts/` copied preserving relative paths into
   `agent-os/proto/` (33 files, including `README.md`, which is present in the pack and its
   lock but omitted from the task's `files:` list — see Concerns).
2. SQL schemas copied: `agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql`
   -> `agent-os/schema/kernel_store.sql` and
   `agent-os-microkernel-mvp-buildpack/specs/event-journal-schema.sql`
   -> `agent-os/schema/event_journal.sql`.
3. `agent-os/crates/domain/build.rs` written: collects every `.proto` under
   `agent-os/proto` by walking the tree at build time, resolves the vendored protoc via
   `protoc_bin_vendored::protoc_bin_path()`, sets `PROTOC` before constructing
   `prost_build::Config`, compiles with include root `agent-os/proto`, and emits
   `cargo:rerun-if-changed` for the proto root and `build.rs`. The include root is resolved
   from `CARGO_MANIFEST_DIR/../../proto`; the task text's literal `../proto` resolves to
   `agent-os/crates/proto`, which does not exist (design.md says the root is `agent-os/proto`).
4. `agent-os/crates/domain/src/generated.rs` includes the single package artifact
   (`agentos.spec.v1.rs`) as `pub mod contract`.
5. `agent-os/crates/domain/tests/contract_codegen.rs` written first as the RED/GREEN
   anchor: it copies the compile routine, builds twice into two temp dirs, asserts the
   generated trees are byte-identical, and asserts `CommandRequest`, `LoopDecision`, and the
   six decision messages exist by generated path.

## Root-cause blocker (R1.1 not satisfiable as delivered)

`protoc` (vendored, libprotoc 31.1) rejects the snapshot because top-level enum values share
the package scope (`agentos.spec.v1`) under C++ scoping rules:

- `agent-os/proto/domain/core.proto:9` `WorkspaceAccessMode.READ_ONLY` vs
  `agent-os/proto/domain/effects.proto:4` `EffectClass.READ_ONLY`
- `agent-os/proto/domain/core.proto:6` `RunState.FAILED` vs
  `agent-os/proto/domain/effects.proto:7` `EffectState.FAILED`
- `agent-os/proto/domain/core.proto:6` `RunState.CANCELLED` vs
  `agent-os/proto/domain/effects.proto:7` `EffectState.CANCELLED`

A scan of all 56 top-level enum values finds exactly these three collisions. No protoc flag
relaxes the check. `control-api/commands.proto` and `control-api/mvp_control.proto` both
import `domain/core.proto` and `domain/effects.proto`, so the closure cannot be split into
compilable subsets.

Reproduction (from `agent-os/`):

```
$ cargo test -p domain contract_codegen
error: failed to run custom build command for `domain v0.1.0 (/Users/jainamshah/Documents/GitHub/ai-harness/agent-os/crates/domain)`
  thread 'main' panicked at crates/domain/build.rs:47:10:
  contract snapshot compiles: Custom { kind: Other, error: "protoc failed:
  .../proto/domain/effects.proto:4:48: \"agentos.spec.v1.READ_ONLY\" is already defined in file \"domain/core.proto\".
  ...:7:114: \"agentos.spec.v1.FAILED\" is already defined in file \"domain/core.proto\".
  ...:7:124: \"agentos.spec.v1.CANCELLED\" is already defined in file \"domain/core.proto\".
  .../proto/control-api/commands.proto:6:1: Import \"domain/effects.proto\" was not found or had errors." }
```

Independent reproduction with the vendored binary:

```
$ protoc -I agent-os/proto --descriptor_set_out=/dev/null control-api/commands.proto
agent-os/proto/domain/effects.proto:4:48: "agentos.spec.v1.READ_ONLY" is already defined ...
(3 collision pairs + the commands.proto import failure)
```

GC-1's acceptance test only checked duplicate **message/service** names
(`grep -rhoE '^(message|service) ...' | uniq -d`), so this enum-value class slipped through.
Fixing it requires an edit to `agent-os-microkernel-mvp-buildpack/contracts/domain/effects.proto`,
which is outside this task's file lease; editing only the mirror is forbidden by N3 and
would break `diff -rq`/lock verification.

## Probed minimal fix (verified in a scratch copy outside the repo)

Renaming the three colliding values in `domain/effects.proto` makes the entire snapshot
compile (probe in `/var/folders/.../T/opencode/fnd002-probe`, no repo files touched):

- `EffectClass.READ_ONLY` -> `EFFECT_CLASS_READ_ONLY`
- `EffectState.FAILED` -> `EFFECT_STATE_FAILED`
- `EffectState.CANCELLED` -> `EFFECT_STATE_CANCELLED`

(Preferred canonical form: prefix all values of both enums, matching the existing
`EFFECT_CLASS_UNSPECIFIED` / `EFFECT_STATE_UNSPECIFIED` convention and the design's
`EffectClass`/`EffectState` mirror-enum names in `crates/domain/src/effect.rs`.)

```
$ protoc -I <probe>/proto --descriptor_set_out=<probe>/out.pb $(find <probe>/proto -name '*.proto' | sort)
PROBE_COMPILES
```

## Acceptance criteria status

| Criterion | Status | Evidence |
|---|---|---|
| R1.1 snapshot compiles with no duplicate symbols | **NOT MET** | `cargo test -p domain contract_codegen` fails in `build.rs`; vendored protoc names the three collisions |
| R14.1 build succeeds with no system protoc | Blocked | vendored `PROTOC` is wired; build never reaches codegen |
| R14.2 / N4 two generations byte-identical | Blocked | determinism test exists but cannot compile until R1.1 is fixed |
| R14.3 no hand-written duplicate struct | Met | only `generated.rs` include of `agentos.spec.v1.rs`; no hand-written message structs added |
| N3 mirror verbatim; no copied file edited | Met (at copy time) | `diff -rq ...` was clean immediately after copy; no mirror file edited |
| No file outside `files:` changed | Met | working-tree changes are only the leased paths plus the pack README copy noted below |
| Mirror copy + SQL schemas | Met | `agent-os/proto/` (33 files), `agent-os/schema/{kernel_store,event_journal}.sql` |
| Test-first RED anchor | Met | first run: `error[E0432]: unresolved import domain::generated::contract` (no `contract` in `generated`) |

## Concurrent drift found while blocked (needs controller order)

`agent-os-microkernel-mvp-buildpack/contracts/events/catalog.yaml` was modified by the
in-flight GC-2 task after this task's mirror was taken. Current check:

```
$ diff -rq agent-os-microkernel-mvp-buildpack/contracts agent-os/proto
Files agent-os-microkernel-mvp-buildpack/contracts/events/catalog.yaml and agent-os/proto/events/catalog.yaml differ
```

The mirror must be re-synced once GC-2 completes, and the README/lock state regenerated by
GC-7, before the `diff -rq` verification can be green.

## Files changed (working tree, uncommitted)

- `agent-os/proto/**` (33 copied files, including pack `README.md`)
- `agent-os/schema/kernel_store.sql`, `agent-os/schema/event_journal.sql`
- `agent-os/crates/domain/build.rs` (new)
- `agent-os/crates/domain/src/generated.rs` (stub replaced with the `contract` include)
- `agent-os/crates/domain/tests/contract_codegen.rs` (new)

## Concerns

- The block is a dependency defect, not an implementation defect: the build.rs/generated.rs/
  test scaffolding is complete and can be reused unchanged once the snapshot is fixed.
- `README.md` exists in the pack (and in `contract-lock.sha256`) but is absent from the task's
  `files:` list, while the verification requires `diff -rq` to be empty; it was copied to
  `agent-os/proto/README.md` to satisfy the mirror-verbatim criterion.
- The task text's `../proto` include path is one directory short (`agent-os/crates/proto`);
  `build.rs` uses `CARGO_MANIFEST_DIR/../../proto`, matching design.md.
- `contract-lock.sha256` in the pack is already stale relative to the pack
  (`control.proto` listed but deleted, `README.md` hash changed, `commands.proto` absent);
  per the task this is GC-7's to regenerate, not FND-002's.

---

## Resume after GC-8 (final, in review)

### Re-sync

After GC-8 (enum rename), GC-2 (events catalog), and GC-7 (lock regeneration) landed, the
mirror was re-copied verbatim from the pack; the stale pre-GC-8 copies were replaced:

```
$ rm -rf agent-os/proto agent-os/schema && mkdir -p agent-os/proto agent-os/schema
$ cp -R agent-os-microkernel-mvp-buildpack/contracts/. agent-os/proto/
$ cp .../specs/kernel-store-schema.sql agent-os/schema/kernel_store.sql
$ cp .../specs/event-journal-schema.sql agent-os/schema/event_journal.sql
$ diff -rq agent-os-microkernel-mvp-buildpack/contracts agent-os/proto
MIRROR_IDENTICAL
$ find agent-os/proto agent-os/schema -type f | wc -l
35
```

`domain/effects.proto` now carries `EFFECT_CLASS_READ_ONLY`, `EFFECT_STATE_FAILED`,
`EFFECT_STATE_CANCELLED`; all 56 top-level enum values are unique under
`agentos.spec.v1`.

### Changes made during resume

- Replaced the stale mirror with the post-GC-8/post-GC-2/post-GC-7 tree; `README.md` is now
  explicitly in the task lease (`agent-os/proto/README.md`).
- Renamed the two test functions to contain `contract_codegen` so the exact acceptance
  command `cargo test -p domain contract_codegen` actually selects and runs them (cargo's
  filter matches test names, not the test binary filename).
- Added `#[allow(clippy::large_enum_variant)]` on the generated `contract` module in
  `generated.rs`: clippy flags prost's generated `EventStreamFrame::frame` oneof (272-byte
  enum). The generated file is not hand-editable, so the allow is scoped to the included
  module; no generated content was changed.

### Acceptance criteria (final)

| Criterion | Status | Evidence |
|---|---|---|
| R1.1 snapshot compiles with no duplicate symbols | Met | `cargo test -p domain contract_codegen` -> `test result: ok. 2 passed; 0 failed` |
| R14.1 build succeeds with no system protoc | Met | vendored protoc via `protoc_bin_vendored::protoc_bin_path()` + `PROTOC` in `build.rs`; `protoc` is not installed system-wide |
| R14.2 / N4 two generations byte-identical | Met | `contract_codegen_regenerates_the_snapshot_byte_identical` compares two temp-dir trees byte-for-byte |
| R14.3 no hand-written duplicate struct | Met | only `include!(concat!(env!("OUT_DIR"), "/agentos.spec.v1.rs"))`; no message structs hand-written |
| N3 mirror verbatim; no copied file edited | Met | `diff -rq agent-os-microkernel-mvp-buildpack/contracts agent-os/proto` prints nothing |
| No file outside `files:` changed | Met | commit `0e4b01f` stages only the leased paths |
| Steps 1-6 of the task | Met | copy -> build.rs/generated.rs -> test-first RED -> GREEN -> workspace check -> commit |

RED (before the generator existed, from the first attempt):

```
error[E0432]: unresolved import `domain::generated::contract`
 --> crates/domain/tests/contract_codegen.rs:6:5
  | use domain::generated::contract;
  |     ^^^^^^^^^^^^^^^^^^^^^^^^^^^ no `contract` in `generated`
```

GREEN (final mirror):

```
$ cargo test -p domain contract_codegen
running 2 tests
test contract_codegen_exports_command_request_and_the_six_decisions ... ok
test contract_codegen_regenerates_the_snapshot_byte_identical ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.04s
```

### Quality gates

- `cargo check --workspace` -> `Finished dev profile ... in 2m 25s` (exit 0).
- `cargo clippy --workspace --all-targets -- -D warnings` -> `Finished ... in 1m 47s` (exit 0).
- `cargo test --workspace` -> 64 `test result: ok` targets, no failures, no errors.
- `cargo fmt --check` -> `cargo fmt -p domain --check` clean; the only workspace diff is
  `agent-os/crates/errors/tests/codes.rs:58`, which belongs to FND-004's lease and was left
  untouched.

### Commit

```
0e4b01f feat(domain): install contract mirror and hermetic codegen [FND-002]
38 files changed, 2310 insertions(+)
```

### Final concerns

- The workspace-wide `cargo fmt --check` is red solely because of
  `crates/errors/tests/codes.rs` (FND-004). FND-002's own files pass `cargo fmt --check`.
- The `#[allow(clippy::large_enum_variant)]` on the generated module is required because
  clippy lints `include!`-expanded code; if the contract later avoids the large oneof, the
  allow can be removed.

