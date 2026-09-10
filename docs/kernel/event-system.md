> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Event System

## Three parts

1. Transactional outbox entries inside KernelStore.
2. Durable Event Journal projection.
3. Live Event Bus for subscribers.

## Durable event ordering

Every durable stream has canonical `stream_key`, monotonic `sequence`, stable `event_id`, payload version, causation/correlation IDs, sensitivity label, and retention class.

## Publication order

For a durable event, the outbox dispatcher publishes **Event Journal first, Live Event Bus second**. A live cursor is exposed only after the journal has accepted the same `event_id`/stream sequence. If the journal is unavailable, canonical KernelStore commits continue and the outbox backs up, but resumable durable live delivery waits. This prevents a client from observing a cursor that cannot yet be resumed from durable history.

Outbox publishing is ordered per stream. The Event Journal must treat an exact re-append of the same `event_id` at the same sequence as idempotent success and reject a conflicting event at that sequence.

## Slow subscribers

Durable streams: disconnect/mark lagging subscriber and require resume from durable cursor. Never advance the cursor past an undelivered durable event. Ephemeral telemetry may drop with a dropped-event metric.
