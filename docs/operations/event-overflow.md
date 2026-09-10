> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Live Event Bus Overflow

Durable event subscribers must either receive events in cursor order or be disconnected/marked lagging and resume from the Event Journal. The server never silently advances a durable cursor past undelivered data.

Ephemeral telemetry may be dropped under bounded-buffer pressure; drops are measured and surfaced.

For durable events the cursor becomes live-visible only after Event Journal acknowledgement, so a disconnected client can always resume from a cursor it previously observed, subject to documented retention expiry.
