# Task FND-006 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** FND-006 — Implement the classification and redaction substrate
- **Agent:** agent-fnd006
- **Status:** DONE (in review)
- **Commit:** `c43a6e6` — `feat(observability): classification and redaction substrate [FND-006]`
- **Branch:** `feat/agentd-microkernel-mvp`

## What was implemented

`agent-os/crates/observability/` now exposes the interface block from `design.md`
`### observability crate`, implemented exactly:

- `classification.rs`
  - `Classification` — `#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]`,
    discriminants `Public = 0 < Internal = 1 < Confidential = 2 < Secret = 3`.
  - `Classified` trait with `fn classification(&self) -> Classification`.
  - `Redacted<T>` — `new`, `get`, `into_inner`; manual `Debug` printing `[REDACTED]`.
  - `Secret<T>` — `new`, `expose` (the only access path); manual `Debug` and `Display`
    both printing `[REDACTED]`. No derived `Debug`, no `.unwrap()`/`.expect()`.
- `lib.rs`
  - `pub use classification::{Classification, Classified, Redacted, Secret};`
  - `KernelFields { correlation_id, run_id, task_id, effect_id }` with
    `#[derive(Clone, Debug, Default)]`.
  - `init_tracing(json: bool) -> Result<(), Box<dyn std::error::Error>>` — installs a
    `tracing_subscriber` sink via `try_init` (JSON layer when `json`, plain otherwise),
    defaulting the `EnvFilter` to `info` when `RUST_LOG` is absent; returns the error
    instead of panicking.
  - `kernel_span(&KernelFields) -> tracing::Span` — `kernel.span` with the four ids
    declared as `field::Empty` and recorded only when `Some`, so absent ids are omitted.
- `tests/redaction.rs` — 10 integration tests covering R18.1, R18.2, R18.3, N1.

## TDD evidence

### RED — tests written before implementation

```
$ cd agent-os && cargo test -p observability
error[E0432]: unresolved imports `observability::classification::Classification`, `observability::classification::Classified`, `observability::classification::Redacted`, `observability::classification::Secret`
 --> crates/observability/tests/redaction.rs:8:37
error[E0432]: unresolved imports `observability::init_tracing`, `observability::kernel_span`, `observability::KernelFields`
 --> crates/observability/tests/redaction.rs:9:21
error: could not compile `observability` (test "redaction") due to 2 previous errors
```

### GREEN — after implementation

```
$ cd agent-os && cargo test -p observability
running 10 tests
test classification_ordering_redacts_at_or_above_sink_threshold ... ok
test classification_orders_public_below_secret ... ok
test classified_reports_its_classification ... ok
test redacted_debug_hides_but_access_is_allowed ... ok
test secret_debug_never_renders_the_value ... ok
test secret_display_never_renders_the_value ... ok
test secret_expose_is_the_only_access_path ... ok
test kernel_span_omits_absent_kernel_field_ids ... ok
test kernel_span_attaches_kernel_field_ids ... ok
test init_tracing_installs_a_sink ... ok

test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

## Acceptance criteria

| # | Criterion | Met | Demonstrating command / evidence |
|---|---|---|---|
| 1 | R18.1 — span helper attaches kernel field ids | **MET** | `cargo test -p observability` → `kernel_span_attaches_kernel_field_ids` and `kernel_span_omits_absent_kernel_field_ids` pass; the test captures a real `fmt` subscriber writing to a buffer and asserts `correlation_id="corr-1"`, `run_id="run-1"`, `task_id="task-1"`, `effect_id="eff-1"` appear, and that none appear for `KernelFields::default()` |
| 2 | R18.2 / N1 — secret values never render through Debug or Display | **MET** | `cargo test -p observability` → `secret_debug_never_renders_the_value`, `secret_display_never_renders_the_value`, `secret_expose_is_the_only_access_path`, `redacted_debug_hides_but_access_is_allowed` pass; `grep -rnE '\.unwrap\(\)\|\.expect\(' crates/observability/src/` → none; only `Secret::expose` returns content |
| 3 | R18.3 — classification ordering allows redaction above a sink threshold | **MET** | `cargo test -p observability` → `classification_orders_public_below_secret` (`Public < Internal < Confidential < Secret`) and `classification_ordering_redacts_at_or_above_sink_threshold` pass |
| 4 | No file outside `files:` changed | **MET** | `git show --stat --format="%h %s" HEAD` → exactly 3 files: `src/classification.rs`, `src/lib.rs`, `tests/redaction.rs` |

Verification gates:

| Gate | Result | Output |
|---|---|---|
| `cargo test -p observability` | clean | 10 passed, 0 failed |
| `cargo clippy -p observability --all-targets -- -D warnings` | clean | `Finished dev profile` exit 0 |
| `cargo fmt -p observability -- --check` | clean | `fmt-exit:0` |

## Files changed

- `agent-os/crates/observability/src/classification.rs` (new content, 71 lines)
- `agent-os/crates/observability/src/lib.rs` (new content, 49 lines)
- `agent-os/crates/observability/tests/redaction.rs` (new, 168 lines)

## Concerns

1. `init_tracing` converts the `tracing_subscriber` `try_init` error into `io::Error`
   (`Box<dyn std::error::Error + Send + Sync>` cannot coerce directly into the design's
   `Box<dyn std::error::Error>`), preserving failure-by-value instead of panicking. The
   signature matches the design exactly.
2. `init_tracing` can only succeed once per process; the integration test binary calls it
   once, and callers in later crates must tolerate an `Err` when a global sink already
   exists (documented here; no API change).
3. `kernel_span` records ids via `Span::record` on `field::Empty`, so absent ids are
   omitted rather than emitted as empty strings. This satisfies "when present" in the
   task context.
