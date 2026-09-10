# Frozen MVP Contract Snapshot

This directory is the implementation-pack snapshot of the canonical inception contracts used by the microkernel/control-plane MVP.

Rules:

- Protocol/domain/public port fields come from here, not from ad-hoc hand-written duplicates.
- `config.schema_version` and `extension.manifest_version` are normalized to inception version `1` as documented in `../SOURCE_CORRECTIONS.md`.
- Implementation-specific semantics live in `../specs/` and must not contradict these contracts.
- Re-run `scripts/validate_buildpack.py` after any intentional contract edit and update `contract-lock.sha256`.
