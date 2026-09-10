> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Component Responsibility Matrix

| Component | Trust | Scope | Replaceable? | Authority |
|---|---|---|---:|---|
| agentd / command coordinator | trusted | daemon | no | canonical command execution |
| KernelStore | trusted adapter | bootstrap-global | maintenance-only | authoritative runtime persistence |
| Event Journal | adapter | generation-global | yes | historical projection |
| Runtime/RunGraph | trusted | daemon | no | execution state/relations |
| Effect Coordinator | trusted | daemon | no | external-effect safety |
| Sandbox/Workspace/Artifact | adapter | run-scoped | yes | implementation of fixed ports |
| Model/Memory/Context/Tools | adapter/service | run-scoped | yes | mechanics/behavior under kernel mediation |
| AgentLoop | behavioral | run-scoped | yes | next-action policy |
| Secret Store | trusted-ish adapter | generation-global | yes | secret backend; broker controls use |
| Remote Transport | adapter | generation-global | yes | connectivity, not execution authority |
