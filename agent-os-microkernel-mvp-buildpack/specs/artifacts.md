> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Local Artifact Store

The MVP artifact store persists immutable/versioned outputs under an application-owned directory.

Operations:

- put bytes/stream → `artifact://id`;
- get/range read;
- metadata/head;
- list by run/task;
- delete only when retention/policy allows.

Artifact metadata stores content digest, media type, size, originating run/effect, sensitivity, retention class, and physical locator known only to the adapter.

Agent/runtime code never persists absolute artifact paths in domain records.

## Durable record

Artifact metadata maps to the `artifacts` table in `kernel-store-schema.sql`: `artifact_id` (identity), `uri` (`artifact://id`, UNIQUE), `digest`, `media_type`, `size_bytes`, `origin_run_id`/`origin_effect_id` (FKs to `runs`/`effects`), `sensitivity` and `retention` (integer `Sensitivity`/`RetentionClass` from `contracts/events/event.proto`), `locator` (adapter-private physical location), and `created_at_ms`. Content bytes are immutable; the row is deleted only when retention/policy allows.
