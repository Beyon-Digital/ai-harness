> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Command Coordinator

The Command Coordinator is the linearization point for state-changing kernel commands.

## Input envelope

Actor/principal/device identity, idempotency key, command ID, correlation/causation IDs, expected run/config revision where relevant, deadline/cancellation metadata, and command payload.

## Responsibilities

1. Authenticate/authorize.
2. Resolve prior idempotency outcome.
3. Validate current revisions/fences.
4. Open KernelStore transaction.
5. Apply canonical mutation(s).
6. Create effects/reservations/outbox records as needed.
7. Commit.
8. Return the durable outcome.

A successful state-changing response must never describe an uncommitted canonical mutation.
