> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# TransportPort

**Binding scope:** `generation-global`  
**Normative ID:** `transport@1`

## Purpose

Stable semantic contract for replaceable transport implementations.

## Required operations

connect/channel/send/stream/resume/health/close.

## Candidate adapters

Unix socket; localhost HTTP/WS; cloud relay/direct transport.

## Capabilities

E2E, resume, multiplex, backpressure, device identity.

## Common call context

Every external call carries operation/call ID, actor/run/delegation context, deadline/cancellation, correlation/causation IDs, and granted capabilities. Effectful operations additionally carry stable `effect_id`/`operation_id`, request hash, and fencing metadata where applicable.

## Versioning

Breaking semantic changes create `transport@2`; additive optional capabilities may remain compatible with `@1`.
