# Task EFF-001 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** EFF-001 — Effect contract model + conservative policy resolver
- **Status:** DONE
- **Commit:** `b4c0b31` — `feat(effects): contract resolver, preparation, executor fencing [EFF-001][EFF-002][EFF-003]`
- **Branch:** `devin/1789940928-effects-wave`

## What was implemented

`agent-os/crates/effects/`:
- `contract.rs` — `CancellationSemantics` (`before_dispatch`, `cooperative`,
  `provider_specific`, `unsupported`), `EffectContract` with wire projection
  `to_contract`/`from_contract`, `EffectContract::unknown()` (fully conservative),
  `is_reconcilable`, `is_safely_redispatchable`, `can_cancel_before_dispatch`.
- `policy.rs` — `DeclaredSemantics` (all axes optional), `ResolveInputs`
  {kernel, claim, trust, conformance, policy}. Adapter claims contribute only when
  `(Trusted, Passed)`; each axis resolves via weakest-rank intersection over
  contributing sources; absent evidence maps to the most conservative default.
  `resolve_compensation` requires unanimous declarations (absent abstains;
  conflicting -> None).

## Evidence — task ACs

- "untrusted self-declared read-only cannot self-promote" — `untrusted_claims_do_not_promote` test.
- "trusted built-in known read-only remains read-only" — trusted+passed claim wins only within kernel bounds.
- Resolver never safer than evidence: absent axes resolve to Unspecified/most-conservative rank.

`cargo test -p effects` — 4 contract + 7 policy unit tests, all green.
