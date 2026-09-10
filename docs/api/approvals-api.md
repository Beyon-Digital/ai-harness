> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Approval API

Approval requests are immutable records with request digest, operation/target, capability scope, actor/delegation chain, relevant config/extension/bundle digests, nonce, expiry, and human-readable projection.

Approval responses reference the exact request ID/digest and authenticated approving device/user. Any material mutation creates a new approval request.
