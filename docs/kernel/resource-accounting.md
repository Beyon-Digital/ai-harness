> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Resource Accounting

Budgets and allocations use durable `ResourceReservation` records.

States: `Reserved`, `Allocated`, `Released`, `Expired`, `Unknown`.

Tracked resources may include model cost/tokens, wall time, CPU/memory, disk, network, sandbox slots, child-run concurrency/depth, cloud machines, and tool-call budgets.

Parent runs may delegate only bounded remaining budget to children. Reservation and run transition commit together where correctness requires it.
