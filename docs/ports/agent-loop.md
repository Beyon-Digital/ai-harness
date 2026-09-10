> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# AgentLoopPort

**Binding scope:** `run-scoped`  
**Normative ID:** `agent-loop@1`

## Purpose

Stable semantic contract for replaceable agentloop implementations.

## Required operations

next fenced decision; checkpoint/restore optional; shutdown.

## Candidate adapters

Hermes; Codex; ReAct; research; generated loop.

## Capabilities

checkpoint, streaming decisions, wait/human/subagent intents.

## Common call context

Every external call carries operation/call ID, actor/run/delegation context, deadline/cancellation, correlation/causation IDs, and granted capabilities. Effectful operations additionally carry stable `effect_id`/`operation_id`, request hash, and fencing metadata where applicable.

## Versioning

Breaking semantic changes create `agent-loop@2`; additive optional capabilities may remain compatible with `@1`.
