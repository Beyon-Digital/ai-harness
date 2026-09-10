> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Build a Custom Adapter

1. Select a normative port/version from `spec/ports`.
2. Implement mandatory semantics.
3. Declare optional capabilities and effect behavior honestly.
4. Build an immutable locked bundle.
5. Run port/effect/security conformance tests.
6. Register the exact digest.
7. Test through a non-active config generation/profile.
8. Activate only after health/smoke gates.
