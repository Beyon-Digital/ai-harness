> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Remote Access Security

Phone/Web → authenticated relay/tunnel ← outbound Mac connection. No public inbound Mac port required by default.

Requirements: device identity/revocation, strong user auth, per-command authorization/idempotency, replay protection, channel binding, encrypted transport/application E2E where relay is not trusted with content, minimal relay metadata, and live revocation handling.
