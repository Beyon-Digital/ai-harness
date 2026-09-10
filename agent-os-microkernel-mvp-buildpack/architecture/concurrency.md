> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Concurrency and Linearization Points

| Operation | Linearization point |
|---|---|
| Command success | KernelStore transaction commit |
| Idempotent command replay | unique `(principal_id, idempotency_key)` record |
| Run transition | conditional update on `run_revision` |
| Loop decision acceptance | CAS on revision + epoch + step + cursor |
| RunGraph edge insertion | graph transaction after cycle check |
| Ready-run claim | conditional state/claim update in graph transaction |
| Parent cancellation | incremented cancellation epoch + subtree state mutation |
| Effect executor ownership | conditional claim with fencing token |
| Timer fire vs cancel | CAS from `Scheduled` |
| Workspace exclusive ownership | lease-epoch CAS + enforceable access handoff |
| Config activation | active generation pointer CAS |
| Daemon ownership | OS file lock + durable fencing epoch |

Tests must deliberately race these operations rather than only test serial behavior.
