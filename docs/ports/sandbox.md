> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# SandboxPort

**Binding scope:** `run-scoped`  
**Normative ID:** `sandbox@1`

## Purpose

Stable semantic contract for replaceable sandbox implementations.

## Required operations

create, exec, status, terminate; checkpoint/restore where supported; network/resource policy.

## Candidate adapters

trusted local process; Apple Container/Docker; remote VM/cloud.

## Capabilities

trust tier, isolation, egress policy, persistent FS, checkpoint, GPU, suspend/resume.

## Common call context

Every external call carries operation/call ID, actor/run/delegation context, deadline/cancellation, correlation/causation IDs, and granted capabilities. Effectful operations additionally carry stable `effect_id`/`operation_id`, request hash, and fencing metadata where applicable.

## Versioning

Breaking semantic changes create `sandbox@2`; additive optional capabilities may remain compatible with `@1`.
