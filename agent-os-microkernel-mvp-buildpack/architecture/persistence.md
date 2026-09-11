> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Persistence Model

## Single authority

`kernel.db` is authoritative for current execution correctness. `events.db` is a downstream durable projection.

## SQLite operating mode

These inception settings are normative:

```text
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
```

File modes for the database, runtime directory, and control socket are normative values from [`specs/limits.yaml`](../specs/limits.yaml).

All correctness-critical write sequences use explicit write transactions. For operations that require writer reservation before reading/modifying shared graph/head state, use `BEGIN IMMEDIATE` semantics.

## Transaction requirements

The `KernelStore` abstraction must support:

- explicit transaction lifetime;
- read + conditional write in the same transaction;
- unique constraints as correctness tools;
- monotonic stream-head allocation;
- compare expected revision/version;
- daemon fencing-epoch assertion;
- insertion of outbox and idempotency rows before commit.

## Event projection

Outbox rows are immutable except publication metadata. The dispatcher:

1. reads unpublished rows ordered by `(stream_key, sequence)`;
2. appends to Event Journal with expected sequence;
3. treats same event ID at same sequence as idempotent success;
4. marks journal publication;
5. emits to live subscribers;
6. marks live publication metadata if tracked.

A crash between 2 and 4 is safe because step 2 is idempotent.
