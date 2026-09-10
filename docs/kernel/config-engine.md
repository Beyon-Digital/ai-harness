> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Configuration Engine

Configuration is immutable generations, not mutable live files.

## Pipeline

`Draft → schema validate → resolve ports → capability negotiate → isolated instantiate → conformance/smoke tests → health gate → atomic activation`.

Existing runs retain persisted resolved bindings. Activation affects new resolutions only unless a future explicit rebind protocol is used.

KernelStore is bootstrap-global and excluded from ordinary runtime config transactions; it is selected as part of daemon deployment configuration.
