# Task ADP-004 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** ADP-004 — Port negotiation: capability checks + deterministic resolution
- **Status:** DONE
- **Commits:** `0eb071a` — `feat(adapter-registry): deterministic port resolver + sandbox-tier capability gate [ADP-004]`
- **Branch:** `devin-1789944697-support-wave`

## What was implemented

- `capabilities.rs`: `SandboxTier{T0..T3}` ordered; `hosts_tier` fails
  closed — T2/T3 require every capability in the canonical
  `adapter-capabilities.yaml` catalog (`REQUIRED_FOR_T2`, 10 caps); T0/T1
  carry no capability floor.
- `resolver.rs`: `resolve()` takes a `PortRequirement` (port_id@exact
  major, required capabilities, sandbox tier, optional adapter pin,
  conformance gate) and filters registered candidates: exact major match,
  capability satisfaction, `hosts_tier`, `trust_state==Trusted` for any
  tier > T0, pin equality, `conformance_state != Failed` (with optional
  `Passed` gate). Deterministic order: manifest priority position first,
  then `(adapter_id, version, bundle_digest)` lexicographic — the same
  candidate set always resolves identically regardless of persistence
  order. Empty set -> `FailedPrecondition("capability unsupported")`.
  Resolved `negotiated_capabilities` are returned/persisted with the
  resolution for enforcement downstream.

## Evidence

`cargo test -p adapter-registry` resolver tests: exact match/miss;
insertion-order-independent tie-break; priority beats lexicographic;
T2 rejected for T0 host (fail-closed); T2 gated on missing required caps;
port major mismatch rejected; conformance gate excludes unreviewed
adapters; `REQUIRED_FOR_T2` asserted equal to the YAML catalog via
`include_str!`.
