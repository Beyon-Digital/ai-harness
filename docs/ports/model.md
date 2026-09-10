> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# ModelPort

**Binding scope:** `run-scoped`  
**Normative ID:** `model@1`

## Purpose

Stable semantic contract for replaceable model implementations.

## Required operations

models, invoke/stream, status/reconcile optional, cancel optional.

## Candidate adapters

OpenAI-compatible; Anthropic; local MLX/Ollama; custom.

## Capabilities

tools, structured output, reasoning controls, multimodal, status lookup, provider idempotency.

## Common call context

Every external call carries operation/call ID, actor/run/delegation context, deadline/cancellation, correlation/causation IDs, and granted capabilities. Effectful operations additionally carry stable `effect_id`/`operation_id`, request hash, and fencing metadata where applicable.

## Versioning

Breaking semantic changes create `model@2`; additive optional capabilities may remain compatible with `@1`.
