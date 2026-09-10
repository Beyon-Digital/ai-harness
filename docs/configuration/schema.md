> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Configuration Schema

Strict versioned configuration covers daemon identity, bootstrap/global service bindings, runtime profiles, AgentSpecs/defaults, policies, remote access, observability, limits, and extension registrations.

Unknown fields fail in strict mode. Secrets are referenced by logical secret URIs rather than embedded plaintext. Binding scope violations are schema/semantic validation errors.
