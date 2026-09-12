# Task CMD-001A Report

- **Spec:** agentd-command-core
- **Task:** CMD-001A — Additive fault-injection seam
- **Agent:** agent-cmd001a
- **Status:** DONE
- **Commit:** `0dc0580` — `feat(faults): add defaulted inject seam [CMD-001A]`
- **Branch:** `feat/agentd-microkernel-mvp`

## What changed

- `agent-os/crates/domain/src/faults.rs`: `FaultInjector` gains the additive,
  defaulted `fn inject(&self, point: &str) -> bool` returning `false`. `NoFaults`
  keeps its existing `trigger` override and inherits the default `inject`, so every
  existing implementor keeps compiling unchanged.
- `agent-os/crates/testkit/src/faults.rs`: extracted the once-per-arm state
  transition into a private `ArmedFaults::fire(&self, point) -> bool` helper.
  `trigger` delegates to it (discarding the result, semantics unchanged) and
  `inject` returns it. An armed, previously unfired point flips to `triggered` and
  returns `true`; an unarmed or already-fired point returns `false`; `arm` resets
  the state as before.
- `agent-os/crates/testkit/tests/prop_faults.rs`: added the failing inject tests
  (RED first): four explicit tests plus a new model-based `prop_inject_once_per_arm`
  property over mixed `Arm`/`Inject` sequences. The pre-existing `prop_faults`
  property and all `trigger` unit tests are untouched.

## Acceptance criteria

| # | Criterion | Met | Evidence |
|---|---|---|---|
| 1 | R4.3 — armed points fire exactly once per arm; unarmed points are no-ops | **MET** | `inject_armed_point_fires_exactly_once`, `inject_unarmed_point_returns_false`, `prop_inject_once_per_arm` pass |
| 2 | R4.4 — `NoFaults` and the default never fire | **MET** | default body is `false`; `NoFaults` has no `inject` override (inherits it); `cargo test -p domain` passes |
| 3 | N2 — no sleeps; existing once-per-arm property test still passes | **MET** | no sleep/timing code added; `prop_faults` passes unmodified |
| 4 | P2 — seam usable for pre-commit failure injection | **MET** | `inject` returns `bool` at a named point with once-per-arm semantics; RED test `inject_marks_point_triggered_for_assertions` proves `assert_triggered` works after an injected fire |
| 5 | No file outside `files:` changed | **MET** | commit touches exactly the 3 leased paths (`git show --stat 0dc0580`) |
| 6 | `cargo test -p domain -p testkit` passes | **MET** | exit 0; all suites green (below) |
| 7 | `cargo clippy -p domain -p testkit --all-targets -- -D warnings` clean | **MET** | exit 0, no diagnostics |
| 8 | `cargo fmt --check` passes | **MET** | exit 0, no output |
| 9 | Commit scoped with task id, retry on `index.lock` | **MET** | `0dc0580`; retry loop implemented (no retry needed) |

## RED output (real, before implementation)

```
$ cargo test -p testkit --test prop_faults
error[E0599]: no method named `inject` found for struct `ArmedFaults` in the current scope
   --> crates/testkit/tests/prop_faults.rs:25:20
    |
 25 |     assert!(!faults.inject("io"), "an unarmed point must not fire");
    |                     ^^^^^^ method not found in `ArmedFaults`
...
error[E0599]: no method named `inject` found for struct `ArmedFaults` in the current scope
   --> crates/testkit/tests/prop_faults.rs:86:44
    |
 86 |                     let fired_now = faults.inject(POINTS[index]);
    |                                            ^^^^^^ method not found in `ArmedFaults`

For more information about this error, try `rustc --explain E0599`.
error: could not compile `testkit` (test "prop_faults") due to 7 previous errors
```

The 7 E0599 sites are the four explicit tests plus the property, confirming the
tests fail for the intended reason (missing method, not a test bug).

## GREEN output (real, after implementation)

```
$ cargo test -p domain -p testkit
     Running unittests src/lib.rs (target/debug/deps/domain-658b0240d1311e5e)
running 20 tests ... test result: ok. 20 passed; 0 failed
     Running tests/contract_codegen.rs (target/debug/deps/contract_codegen-baff3befd5d6e26b)
running 2 tests ... test result: ok. 2 passed; 0 failed
     Running tests/prop_enums.rs (target/debug/deps/prop_enums-0b6a9d7bda327958)
running 19 tests ... test result: ok. 19 passed; 0 failed
     Running unittests src/lib.rs (target/debug/deps/testkit-8862728b6a11b641)
running 8 tests ... test result: ok. 8 passed; 0 failed
     Running unittests src/bin/fixture-daemon.rs (target/debug/deps/fixture_daemon-898a609e2182add3)
running 0 tests ... test result: ok. 0 passed; 0 failed
     Running tests/daemon_host.rs (target/debug/deps/daemon_host-29a619b98bca5417)
running 4 tests ... test result: ok. 4 passed; 0 failed
     Running tests/prop_faults.rs (target/debug/deps/prop_faults-47fb262311dc02ed)
running 6 tests ... test result: ok. 6 passed; 0 failed
     Running tests/store_mock.rs (target/debug/deps/store_mock-cab12b57ca193baf)
running 16 tests ... test result: ok. 16 passed; 0 failed
   Doc-tests domain: test result: ok. 0 passed; 0 failed
   Doc-tests testkit: test result: ok. 0 passed; 0 failed
```

`prop_faults` reports 6 tests: 2 properties (the untouched `prop_faults` plus the new
`prop_inject_once_per_arm`) and 4 explicit inject tests.

```
$ cargo clippy -p domain -p testkit --all-targets -- -D warnings
    Checking domain v0.1.0 (/Users/jainamshah/Documents/GitHub/ai-harness/agent-os/crates/domain)
    Checking kernel-store v0.1.0 (/Users/jainamshah/Documents/GitHub/ai-harness/agent-os/crates/kernel-store)
    Checking testkit v0.1.0 (/Users/jainamshah/Documents/GitHub/ai-harness/agent-os/crates/testkit)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 48.37s
clippy_exit=0

$ cargo fmt --check
fmt_exit=0
```

## Files changed

- `agent-os/crates/domain/src/faults.rs` (leased)
- `agent-os/crates/testkit/src/faults.rs` (leased)
- `agent-os/crates/testkit/tests/prop_faults.rs` (leased)

`git show --stat 0dc0580` = 3 files changed, 110 insertions(+), 3 deletions(-).

## Concerns

1. `ArmedFaults::trigger` now delegates to the shared `fire` helper and discards
   the boolean. Semantics are provably identical (same state transition, same
   guards), and the unchanged unit tests plus the untouched `prop_faults` property
   confirm it.
2. Tests were placed in `prop_faults.rs` per the task; testkit's in-crate unit tests
   for `trigger` were intentionally left unmodified so "existing semantics unchanged"
   is easy to audit from the diff.
3. No deferred-work markers (`TODO`/`FIXME`/`unimplemented!`) were introduced.
