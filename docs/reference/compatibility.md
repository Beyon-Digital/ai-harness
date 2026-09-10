> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Compatibility and Versioning

Normative schemas/IDs are versioned in `spec/`. Port breaking semantics require a new major version. Event breaking payload changes create a new event version. Config/manifest schemas have explicit schema versions. Internal Rust crate APIs are not public compatibility contracts unless promoted into `spec/`.

CI should fail when generated SDK/docs fixtures are stale relative to `spec/`.
