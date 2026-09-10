> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Extension Integrity and Identity

Kernel computes immutable bundle/dependency/manifest digests. Approval/conformance/launch bind to those digests.

Process adapter identity is established through supervisor-created private IPC or authenticated peer/bootstrap mechanisms tied to expected digest and daemon/adapter instance identity. Self-reported `adapter_id`/digest is not sufficient proof.
