# Task CFG-001 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** CFG-001 — v1 config schema: parse, validate, profiles, security-sensitive deny-unknown
- **Status:** DONE
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `config-engine/src/schema.rs`: `schema_version = 1` enforced; every
  section `deny_unknown_fields` (kernel/services/profiles/limits/
  observability) so typo'd keys in security-sensitive sections are
  rejected at parse. All `limits.yaml` keys are modeled.
- `config-engine/src/model.rs`: `parse_document` (YAML → schema →
  profile-graph validation), `resolve` moved to `profile.rs`;
  `generation_services` extracts the generation-global services block;
  `document_digest` hashes exact document bytes.
- `agent-os/config/default.yaml` ships the default v1 document.

## Evidence

`cargo test -p config-engine`: default config parses as v1 with the
`local-trusted` profile resolving expected bindings;
`schema_version: 2` rejected; inheritance cycles and missing parents
rejected; unknown fields under kernel/services/limits rejected;
`kernel.store` is bootstrap-only (no profile key can rebind it).
