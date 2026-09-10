> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Build a Custom Agent Loop

Implement `next(LoopInput) -> LoopDecision` using generated SDK types. Preserve all fencing fields exactly. Request model/tool/subagent/context/memory actions as kernel intents.

Test stale-decision rejection, crash/restart checkpointing, cancellation, child failures, effect unknowns, and event-cursor recovery. Never hold authoritative child/process state only inside loop memory.
