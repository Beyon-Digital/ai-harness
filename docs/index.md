# AI Agent OS Documentation

This is the sole human-authored documentation tree. Normative IDs and schemas live in [`../spec/`](../spec/README.md).

## System at a glance

```mermaid
flowchart TB
    UI[Phone / Web / macOS / CLI] --> CP[Control API]
    CP --> CC[Command Coordinator]
    CC --> RT[Runtime Manager + RunGraph]
    CC --> EC[Effect Coordinator]
    RT --> KS[(Kernel Metadata Store)]
    EC --> KS
    KS --> OB[Transactional Outbox]
    OB --> EJ[(Event Journal)]
    OB --> LB[Live Event Bus]
    RT --> AR[Adapter Registry]
    EC --> AR
    AR --> PORTS[Versioned Ports]
    PORTS --> INFRA[Infrastructure Adapters]
    PORTS --> LOOPS[Agent Loops / Behavioral Services]
    INFRA --> SAND[Sandboxes / Workspaces / Storage / Models]
    LOOPS --> H[Hermes Loop]
    LOOPS --> C[Codex Loop]
    LOOPS --> G[Generated Loops]
```

## Browse

- [Architecture](architecture/README.md)
- [Kernel components](kernel/README.md)
- [Runtime semantics](runtime/README.md)
- [Stable ports](ports/README.md)
- [Adapter system](adapters/README.md)
- [Extension SDK](extensions/README.md)
- [Configuration/profiles](configuration/README.md)
- [Security](security/README.md)
- [Operations/recovery](operations/README.md)
- [Client/control APIs](api/README.md)
- [Developer guides](guides/README.md)
- [Reference](reference/README.md)
- [Implementation plan](../PLAN.md)
