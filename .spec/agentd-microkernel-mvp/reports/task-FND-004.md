# Task FND-004 report: Implement the stable error model

- Status: implementation complete, in review
- Owner: @agent-fnd004
- Commit: `2989b93` feat(errors): stable codes and retry classification [FND-004]
- Files changed (only leased paths):
  - `agent-os/crates/errors/src/codes.rs`
  - `agent-os/crates/errors/src/lib.rs`
  - `agent-os/crates/errors/tests/codes.rs` (new)

## What was implemented

`crates/errors` now exposes the exact public API from `design.md` under `errors crate`:

- `codes::ErrorCode` with variants `InvalidArgument`, `NotFound`, `Conflict`,
  `FailedPrecondition`, `ResourceExhausted`, `Unavailable`, `Internal`,
  `const fn as_str(self) -> &'static str` returning the seven stable lowercase
  tokens, and `Display` printing that token.
- `codes::RetryClass` with `Never`, `Safe`, `ReconciliationRequired`.
- `KernelError` with private fields `code`, `retry`, `message: Cow<'static, str>`,
  `source: Option<Box<dyn Error + Send + Sync + 'static>>`, plus `new`, `with_source`,
  `code`, `retry_class`, `message`, an `Error::source` impl that preserves and
  downcasts the attached source, and a `Display` impl printing `{code}: {message}`.
- `Result<T> = std::result::Result<T, KernelError>`.

No deferred-work markers. No `unwrap()`/`expect()` in `src/`.

## TDD evidence

### RED — `cargo test -p errors` before implementation (from `agent-os/`)

```
error[E0432]: unresolved imports `errors::codes::ErrorCode`, `errors::codes::RetryClass`
 --> crates/errors/tests/codes.rs:3:21
  |
3 | use errors::codes::{ErrorCode, RetryClass};
  |                     ^^^^^^^^^  ^^^^^^^^^^ no `RetryClass` in `codes`
  |                     |
  |                     no `ErrorCode` in `codes`

error[E0432]: unresolved imports `errors::KernelError`, `errors::Result`
 --> crates/errors/tests/codes.rs:4:14
```

(exit 101; full capture at `/var/folders/.../T/opencode/fnd004-red.txt`)

### GREEN — `cargo test -p errors` after implementation (from `agent-os/`, post-commit)

```
running 6 tests
test display_contains_code_token_and_never_the_source ... ok
test every_code_and_retry_class_survives_formatting ... ok
test result_alias_uses_kernel_error ... ok
test reconciliation_required_is_structurally_distinct_from_safe ... ok
test same_code_produces_the_same_string ... ok
test source_is_preserved_through_error_source ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

## Acceptance criteria

| Criterion | Status | Demonstrating command / evidence |
| --- | --- | --- |
| R16.1 — every failure carries a code and a retry class | met | `cargo test -p errors` → `reconciliation_required_is_structurally_distinct_from_safe`, `every_code_and_retry_class_survives_formatting`, `result_alias_uses_kernel_error` (constructs via `KernelError::new` and reads `code()` / `retry_class()`) |
| R16.2 — the code is preserved across formatting and display | met | `cargo test -p errors` → `same_code_produces_the_same_string`, `display_contains_code_token_and_never_the_source`, `every_code_and_retry_class_survives_formatting` (asserts `as_str == to_string == format!` and `Display` starts with the token) |
| N1 — `Display` never prints source content, only the message | met | `cargo test -p errors` → `display_contains_code_token_and_never_the_source` (attaches `std::io::Error::other("SECRET_PAYLOAD_DO_NOT_PRINT")`; asserts exact output `failed_precondition: guard rejected the command` and absence of the payload) |
| Source preserved through `Error::source` (task integration case) | met | `cargo test -p errors` → `source_is_preserved_through_error_source` (downcasts back to `std::io::Error` and checks `ErrorKind::PermissionDenied`) |
| No file outside `files:` changed | met | `git show --stat 2989b93` lists exactly the three leased paths |
| From `agent-os/`: `cargo test -p errors` passes | met | exit 0, 6/6 integration tests pass |
| `cargo clippy -p errors --all-targets -- -D warnings` is clean | met | `Finished dev profile` with no warnings (exit 0) |
| No `unwrap()`/`expect()` in `src/` | met | `rg 'unwrap\(|expect\(' agent-os/crates/errors/src` / grep tool → no files found |

## Concerns

- `thiserror` remains declared in `Cargo.toml` but is unused by this implementation
  (the design block specifies manual impls). Left untouched because `Cargo.toml`
  is outside the task's `files:` lease.
- Concurrent agents share the `agent-os` workspace target directory; the first
  post-commit GREEN capture was mistakenly invoked from the repo root and is
  discarded. The authoritative GREEN run above was executed from `agent-os/`
  against commit `2989b93`.

## Fix note: rustfmt quality-gate (2026-09-11)

- Defect: workspace-wide `cargo fmt --check` failed on `crates/errors/tests/codes.rs`
  (a `let err = ... .with_source(io);` chain was wrapped by hand where rustfmt
  wants it on one line).
- Fix: ran `cargo fmt -p errors`, re-ran `cargo test -p errors` (6/6 pass, exit 0)
  and `cargo fmt --check` from `agent-os/` (exit 0, workspace clean).
- Commit: `80ec174` style(errors): rustfmt test file [FND-004], scoped to
  `agent-os/crates/errors/tests/codes.rs` only.
