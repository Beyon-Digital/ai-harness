> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Log and Event Data Classification

Every event/log payload carries sensitivity and retention metadata.

Sensitivity: `Public`, `Internal`, `Private`, `Secret`.  
Retention: `Ephemeral`, `Session`, `Audit`, `Durable`.

Consumers receive projections appropriate to authority: local developer UI may view private details; cloud relay defaults to routing metadata; telemetry exporters receive redacted/internal projections; security audit logs record security metadata without raw secrets.

Classification occurs before export; regex redaction is defense-in-depth, not the primary control.
