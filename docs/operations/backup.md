> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Backup and Restore

Back up KernelStore, Event Journal where historical retention matters, config generations, AgentSpecs, extension bundles/digests, memory stores, artifact metadata/bodies according to policy, and secret-store metadata/material only through backend-specific secure mechanisms.

Restore validates schema/spec/kernel versions, daemon fencing state, extension bundle availability, and resolved-run binding references before resuming runs.
