# Frozen MVP Contract Snapshot

This directory is the implementation-pack snapshot of the canonical inception contracts used by the microkernel/control-plane MVP.

## Authority

- `control-api/mvp_control.proto` is the single normative Control API definition in this snapshot. It is the only file that declares a Control API service; the duplicate `control-api/control.proto` was removed from this pack only (see `../SOURCE_CORRECTIONS.md`).
- `protocols/agent_loop.proto` is the normative loop contract: `LoopInput`, the fenced `LoopDecision`, and its six decision variants.
- `CommandRequest.command_type` is the fully-qualified protobuf message name of a payload message declared in `control-api/commands.proto` (design D2).

## Snapshot inventory

- `control-api/mvp_control.proto` — Control API and event API services, command envelope, and subscription frames.
- `control-api/commands.proto` — one payload message per catalogued command (18 total).
- `domain/core.proto`, `domain/entities.proto`, `domain/effects.proto`, `domain/security.proto` — canonical domain records and enums.
- `events/event.proto`, `events/catalog.yaml` — event envelope and event catalog.
- `protocols/agent_loop.proto`, `protocols/effect.proto`, `protocols/external_adapter.proto`, `protocols/adapter_frames.proto` — loop, effect, and adapter protocol frames.
- `ports/*.proto` — public port contracts.
- `capabilities/*.yaml` — capability declarations.
- `config/agent-os.schema.json` — configuration schema.
- `manifests/extension.schema.json` — extension manifest schema.
- `catalog.yaml` — port version catalog.
- `contract-lock.sha256` — snapshot hash lock.

## Rules

- Protocol/domain/public port fields come from here, not from ad-hoc hand-written duplicates.
- `config.schema_version` and `extension.manifest_version` are normalized to inception version `1` as documented in `../SOURCE_CORRECTIONS.md`.
- Implementation-specific semantics live in `../specs/` and must not contradict these contracts.
- Re-run `scripts/validate_buildpack.py` after any intentional contract edit and update `contract-lock.sha256`.
