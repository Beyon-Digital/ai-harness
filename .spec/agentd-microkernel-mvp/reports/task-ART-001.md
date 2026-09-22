# Task ART-001 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** ART-001 — Local artifact store: content-addressed atomic writes, metadata, retention
- **Status:** DONE
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `artifacts/src/local.rs`: `LocalArtifactStore` under an app-owned
  root. `put` hashes content to a `sha256:` digest, writes via
  temp-file + fsync + atomic rename (+ dir fsync), and persists the row
  (`artifact://<id>` URI, sensitivity, retention, origin run/effect) in
  the same transaction — duplicate URI conflicts. `get` re-hashes and
  fails closed on digest mismatch; `get_range`/`head`/`list_by_run`
  supported; `delete` is gated on `retention == Ephemeral` (row removal
  deferred to policy). Agent-facing APIs return only `artifact://` URIs
  and metadata — physical paths never leave the store.

## Evidence

`cargo test -p artifacts` (3 tests): put→get digest/metadata roundtrip
with range+head+list; on-disk tampering detected via digest mismatch;
non-ephemeral retention blocks delete.
