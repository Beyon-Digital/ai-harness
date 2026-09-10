> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Sandbox Manager

Maps authorized execution requests to a `SandboxPort` implementation and enforces the requested trust tier.

Responsibilities: select compatible adapter, create sandbox, materialize/mount workspace, apply filesystem/network/process/resource policies, inject only authorized capabilities/secrets, execute, checkpoint/restore where supported, reconcile status, and destroy/reclaim resources.

`local-process` is T0 trusted execution only and is not a security boundary.
