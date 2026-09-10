> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# ToolRuntimePort

**Binding scope:** `run-scoped`  
**Normative ID:** `tool-runtime@1`

## Purpose

Stable semantic contract for replaceable toolruntime implementations.

## Required operations

list/describe/invoke/status/reconcile/cancel tool operations.

## Candidate adapters

built-in Rust; MCP; Python/TS process; WASM.

## Capabilities

streaming, effect contract, status/reconciliation, cancellation, sandbox binding.

## Common call context

Every external call carries operation/call ID, actor/run/delegation context, deadline/cancellation, correlation/causation IDs, and granted capabilities. Effectful operations additionally carry stable `effect_id`/`operation_id`, request hash, and fencing metadata where applicable.

## Versioning

Breaking semantic changes create `tool-runtime@2`; additive optional capabilities may remain compatible with `@1`.
