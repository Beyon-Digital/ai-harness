# Task ADP-005 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** ADP-005 — Effect fixture adapter with idempotent durable counter
- **Status:** DONE
- **Commits:** `c273829` — `feat(fixtures+conformance): effect/loop fixture bundles, behavioral conformance packs, activation gate [ADP-005][ADP-006][LOOP-001]`
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `fixtures/effect-adapter` (`fixture-effect-adapter`): private protocol
  over the fd-0 socketpair — reads `AdapterBootstrap`, echoes identity in
  `AdapterHello`, then serves `PortCallRequest`s. `execute` applies
  `EffectExecutionRequest` to a JSON fixture store at `FIXTURE_STORE`
  keyed by `operation_id` — duplicates replay the stored outcome
  (provider idempotency). `status` resolves prior effects by operation id
  (`succeeded`/`not_found`) for post-crash reconciliation.
- Fault flags: `FIXTURE_CRASH_BEFORE_RESPONSE` (side effect applied, then
  exit(2) — produces the unknown-effect case), `FIXTURE_DELAY_MS`,
  `FIXTURE_MIN_FENCE` (requests below the floor return
  `fencing_rejected`).
- Shipped as an immutable bundle: `adapter.manifest.json` +
  generated `bundle.lock`; digest computed via the registry's own
  `compute_bundle_digest`.

## Evidence

`cargo test -p fixture-effect-adapter` (conformance tests) plus
`testkit::conformance::effect_adapter_pack`: normal execute increments;
duplicate operation id replays instead of double-applying; crash before
response leaves the effect applied and `status` reconciles it on
re-spawn; stale fencing token rejected; wrong handshake digest refused.
