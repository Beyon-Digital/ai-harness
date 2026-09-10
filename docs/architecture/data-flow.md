> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# End-to-End Execution Flow

```text
Client command
→ authenticate/authorize
→ idempotency lookup
→ transactional command coordinator
→ create/update Task/Run/Graph/Reservation/Outbox
→ dispatch loop turn with fenced snapshot
→ receive loop decision
→ CAS decision against run revision/epoch/step/cursor
→ prepare EffectRecord or child run transactionally
→ effect executor claims/fences
→ adapter execution
→ acknowledge/reconcile outcome
→ commit run/effect/outbox mutation
→ publish historical/live events
→ repeat until terminal
```

No agent loop directly spawns authoritative OS processes, accesses raw kernel secrets, or mutates the RunGraph outside kernel commands.
