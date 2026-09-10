> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# External Process Adapters

Preferred launch uses a supervisor-created private socketpair inherited by the child. The kernel already knows the expected adapter instance and executable/bundle digest.

Handshake additionally proves a single-use bootstrap token/nonce bound to daemon instance, adapter instance, expected digest, and protocol version. Reconnectable endpoints must verify OS peer identity where supported and use authenticated per-instance credentials.

A process crash is isolated. Restart policy must respect effect reconciliation and never cause blind duplicate execution.
