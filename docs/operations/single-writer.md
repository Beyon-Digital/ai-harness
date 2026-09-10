> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Single-Writer and Fencing Operations

Local mode: exclusive OS lock + durable daemon fencing epoch. Remote-store mode: durable lease/fencing token required.

Operational dashboards should expose current daemon instance/epoch, lease expiry, takeover history, stale-writer rejection counts, and effect executor fencing conflicts.
