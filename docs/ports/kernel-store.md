> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# KernelStorePort

**Binding scope:** `bootstrap-global`  
**Normative ID:** `kernel-store@1`

## Purpose

Stable semantic contract for replaceable kernelstore implementations.

## Required operations

begin/commit/rollback transaction; conditional read/write/CAS; unique insert; allocate stream sequence; lease/fence; transactional outbox.

## Candidate adapters

SQLite; Postgres.

## Capabilities

ACID, CAS, uniqueness, monotonic sequencing, fencing and outbox are mandatory; backup/replication optional.

## Common call context

Every external call carries operation/call ID, actor/run/delegation context, deadline/cancellation, correlation/causation IDs, and granted capabilities. Effectful operations additionally carry stable `effect_id`/`operation_id`, request hash, and fencing metadata where applicable.

## Versioning

Breaking semantic changes create `kernel-store@2`; additive optional capabilities may remain compatible with `@1`.
