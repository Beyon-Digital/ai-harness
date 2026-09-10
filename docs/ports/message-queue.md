> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# MessageQueuePort

**Binding scope:** `generation-global`  
**Normative ID:** `message-queue@1`

## Purpose

Stable semantic contract for replaceable messagequeue implementations.

## Required operations

publish, consume, ack, nack; status/visibility where supported.

## Candidate adapters

In-memory; Redis Streams; SQS; NATS/Kafka adapters.

## Capabilities

durability, ordering, replay, delay, consumer groups, DLQ negotiated.

## Common call context

Every external call carries operation/call ID, actor/run/delegation context, deadline/cancellation, correlation/causation IDs, and granted capabilities. Effectful operations additionally carry stable `effect_id`/`operation_id`, request hash, and fencing metadata where applicable.

## Versioning

Breaking semantic changes create `message-queue@2`; additive optional capabilities may remain compatible with `@1`.
