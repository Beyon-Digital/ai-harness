> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Immutable Remote Approvals

A remote approval binds to an exact immutable request, not a boolean alone.

Approval request includes request ID/digest, principal/actor/run, operation, target resource, requested capability set, extension/config/bundle digest where applicable, expiry, nonce, and device/user authorization context.

If the underlying request, extension bundle, capability scope, or target changes, the approval is invalid and must be re-requested.
