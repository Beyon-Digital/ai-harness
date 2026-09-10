> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Adapter Registry

Tracks exact immutable adapter bundles and their port compatibility.

Registration contains adapter ID/version, kernel-computed bundle/dependency/manifest digests, implemented port versions, declared capabilities, effective conformance status, runtime type, required authorities, configuration schema digest, and health status.

Resolution filters by port version, required semantic capabilities, binding scope, environment/trust tier, config pin/priority, and health.
