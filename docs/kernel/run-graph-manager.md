> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# RunGraph Manager

Owns transactional parent-child/dependency edges, graph revision, ready-run claims, cancellation epoch propagation, and graph queries.

## Safety

- Edge insertion validates state and cycle freedom atomically/serially.
- Ready eligibility + claim token are one transaction.
- Child spawn validates current parent cancellation epoch.
- Parent/child authority delegation is persisted with the child creation transaction.
