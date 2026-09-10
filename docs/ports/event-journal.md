> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# EventJournalPort

**Binding scope:** `generation-global`  
**Normative ID:** `event-journal@1`

## Purpose

Stable semantic contract for replaceable eventjournal implementations.

## Required operations

append expected-position event batch; read stream/range; resolve cursor; retention metadata.

## Candidate adapters

SQLite journal; Postgres; durable log adapter.

## Capabilities

per-stream ordering and dedupe mandatory; global ordering optional.

## Common call context

Every external call carries operation/call ID, actor/run/delegation context, deadline/cancellation, correlation/causation IDs, and granted capabilities. Effectful operations additionally carry stable `effect_id`/`operation_id`, request hash, and fencing metadata where applicable.

## Versioning

Breaking semantic changes create `event-journal@2`; additive optional capabilities may remain compatible with `@1`.

## Idempotent publication semantics

The journal must support safe outbox retry. Re-appending the identical `event_id` at its already-committed stream sequence succeeds idempotently. A different event attempting to occupy the same stream sequence is a conflict/invariant error. Publishers preserve per-stream order; cross-stream global order is not required by the initial contract.
