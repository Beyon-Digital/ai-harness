> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Event Pipeline

## Stream keys

Canonical durable stream keys:

```text
run/<run-id>
task/<task-id>
session/<session-id>
effect/<effect-id>
config/global
adapter/<adapter-id>/<version>/<digest>
security/principal/<principal-id>
```

A single command may emit events to several streams. Each stream sequence is allocated inside the same KernelStore transaction.

## Event Journal SQLite schema

```sql
CREATE TABLE events (
  event_id TEXT PRIMARY KEY,
  stream_key TEXT NOT NULL,
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event_version INTEGER NOT NULL,
  occurred_at_ms INTEGER NOT NULL,
  envelope BLOB NOT NULL,
  UNIQUE(stream_key, sequence)
);
CREATE INDEX idx_events_stream ON events(stream_key, sequence);
```

Append semantics:

- `expected_sequence` means the caller expects journal head before append.
- If an identical `event_id` already exists at the requested stream sequence, return idempotent success.
- Same sequence/different event ID is conflict.
- Gaps are rejected.

## Live Bus

Use a bounded Tokio broadcast/watch architecture with explicit lag detection. For durable subscribers, a lag result means: terminate/reconnect and resume from Event Journal cursor. Do not fabricate skipped cursor advancement.

Ephemeral telemetry may use a separate lossy channel.
