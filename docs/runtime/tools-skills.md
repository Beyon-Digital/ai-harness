> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Tools, Skills, and Effect Contracts

A **Tool** is a callable typed operation. A **Skill** is a reusable behavioral package that can include instructions, prompts, tools, scripts, templates, or workflow logic.

Tools declare an effect contract, but the kernel computes the effective contract. Unknown/generated tools default to opaque/unknown side-effect semantics until trusted by policy/conformance.

Tool execution is always associated with actor/run/delegation identity and may be routed through sandbox, workspace, secrets broker, and Effect Coordinator.
