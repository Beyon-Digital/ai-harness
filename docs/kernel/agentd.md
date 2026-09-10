> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# `agentd` — Trusted Rust Daemon

## Responsibilities

Boot the active kernel configuration; acquire exclusive writer authority; expose local/remote control surfaces; host runtime/graph/effect/security/config/event components; supervise external adapters/loops; reconcile external resources; and shut down safely.

## Reliability requirements

- External/untrusted code does not execute in-process.
- Privileged commands are idempotency-keyed.
- Authoritative state transitions occur through the Command Coordinator and KernelStore transaction.
- Every spawned child process/sandbox/resource is owned by a durable runtime identity/reservation where applicable.
- Daemon instance authority is fenced.

## Non-responsibilities

Prompt design, planning strategy, memory extraction policy, workflow-specific branching, or provider-specific business logic.
