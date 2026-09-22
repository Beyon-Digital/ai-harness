# Task WRK-001 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** WRK-001 — Logical Resource URI parser/resolver
- **Status:** DONE
- **Commits:** `d5bb548` — `feat(resource-uri): canonical scheme parser + capability-gated resolver [WRK-001]`
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `parser.rs`: `ResourceUri` enum over the eight canonical schemes
  (`run`, `task`, `session`, `workspace`, `adapter`, `secret`,
  `capability`, `artifact`), strict percent-decoding (`%XX` only) then
  per-segment validation — rejects `.`/`..`, empty segments, backslashes,
  and encoded traversal forms; workspace paths normalize; `adapter` parses
  `id@version#digest`; `Display` round-trips the canonical form.
- `resolver.rs`: injectable `CapabilityGate` (`PermitSetGate`,
  `DenyAllGate`); per-scheme `required_capability`; resolution returns
  `ResolvedResource` logical handles — `secret://` resolves to an opaque
  `SecretRef` handle, never a path or value.

## Evidence

`cargo test -p resource-uri` — 8 tests incl. a proptest corpus (arbitrary
strings never panic; traversal segments always reject; canonical forms
round-trip; resolver checks capability before dispatch; secret resolution
exposes no path).
