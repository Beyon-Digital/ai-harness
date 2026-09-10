> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Sessions and Tasks

`Session` groups interaction/history. `Task` is logical requested work. `AgentRun` is one attempt.

```text
Session
├─ Task A
│  ├─ Run A1 failed
│  └─ Run A2 completed
└─ Task B
   └─ Run B1
```

This separation supports retries, audit, and multi-agent execution without conflating conversation identity with process attempts.
