> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Agent-Generated Extensions

Pipeline:

`need → generate source/manifest/tests → dependency-resolving build sandbox → immutable bundle → kernel digest → static/schema validation → conformance/security tests → smoke workflow → approval tier → install exact digest → enable`.

Low-risk tools may auto-enable only under explicit policy and a suitable restricted sandbox. Behavioral services require sandbox/capability review. Infrastructure adapters and privileged config changes require explicit approval by default.
