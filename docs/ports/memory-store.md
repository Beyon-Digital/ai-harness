> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# MemoryStorePort

**Binding scope:** `run-scoped`  
**Normative ID:** `memory-store@1`

## Purpose

Stable semantic contract for replaceable memorystore implementations.

## Required operations

put/get/search/delete/list namespace with provenance metadata.

## Candidate adapters

SQLite/FTS; Postgres/pgvector; graph/vector stores.

## Capabilities

full-text, vector, metadata filters, transactions, namespace isolation.

## Common call context

Every external call carries operation/call ID, actor/run/delegation context, deadline/cancellation, correlation/causation IDs, and granted capabilities. Effectful operations additionally carry stable `effect_id`/`operation_id`, request hash, and fencing metadata where applicable.

## Versioning

Breaking semantic changes create `memory-store@2`; additive optional capabilities may remain compatible with `@1`.
