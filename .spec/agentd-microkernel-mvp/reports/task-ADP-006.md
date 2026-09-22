# Task ADP-006 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** ADP-006 — Behavioral conformance harness + activation gate
- **Status:** DONE
- **Commits:** `c273829` + `0eb071a` gate wiring — `feat(fixtures+conformance) ... [ADP-006]`
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `adapter-registry/conformance.rs`: `ConformanceReport` bound to the
  exact `(adapter_id, version, bundle_digest)` triple with per-case
  results, `harness_version`, and a
  `sha256("agentos.conformance.v1\n" ++ id ++ version ++ digest ++ cases)`
  report digest. `persist_report` inserts the report and flips
  `conformance_state` (Passed/Failed) in the same `KernelTxn` —
  `AdapterRepo::set_conformance_state` added (sqlite + mock).
- `testkit/conformance.rs`: reusable packs — `process_protocol_pack`
  (handshake identity, ping/pong + call, deadline expiry) and
  `effect_adapter_pack` (execute + idempotent replay,
  crash-then-status-reconcile, fencing rejection). Every spawn/
  handshake/exec failure records a failed `CaseResult`, never panics.
- `fixtures/effect-adapter/tests/adapter_conformance.rs` — the
  spec's `tests/adapter_conformance.rs` placed under the fixture crate
  so `CARGO_BIN_EXE_*` resolves (workspace root is virtual and cannot
  hold integration tests).

## Evidence

`cargo test -p fixture-effect-adapter` (3 tests): full 6-case pass
-> report persists Passed -> resolver admits under
`require_conformance_passed`; a lying bundle (`/bin/true` entrypoint)
fails every case -> Failed -> resolver rejects even without the gate;
bundle mutation produces a new digest for which no report exists and
`verify_for_spawn` fails `NotFound` — reports never transfer across
digests.
