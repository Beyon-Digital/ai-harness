> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Capabilities and Delegation Chains

Every privileged request records principal, actor, run, and complete delegation chain.

Parent runs may delegate only a subset of their own authority/budget. Child-to-tool delegation is evaluated against both child grants and ancestor constraints. This prevents an untrusted child from invoking a more privileged parent-owned helper as a confused deputy.

Workspace authority is separately represented by WorkspaceLease and cannot be amplified by generic filesystem permission.
