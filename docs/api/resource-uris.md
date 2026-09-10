> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Logical Resource URI API

Canonical schemes include `workspace://`, `artifact://`, `secret://`, `sandbox://`, `run://`, `task://`, `session://`, `extension://`.

Resolution is adapter-aware and capability-checked. Extensions/clients must not transform logical URIs into guessed host paths or cloud bucket keys.
