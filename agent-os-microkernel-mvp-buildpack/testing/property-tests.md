> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Property/Concurrency Tests

Required properties:

- Run state never transitions outside the allowed transition relation.
- `run_revision` strictly increases on authoritative run mutation.
- accepted loop decision tuple is unique for a run revision/epoch/step/turn.
- dependency graph remains acyclic.
- at most one active ready-run claim token is authoritative.
- child cancellation epoch observed at creation equals parent epoch at commit.
- at most one effect fencing token can commit an effect transition.
- committed effect states never return to pre-dispatch states.
- `Unknown` is never auto-redispatched without explicit safe policy.
- reservation descendants never exceed ancestor budget.
- capability delegation is monotonic non-increasing.
- exactly one of timer claim/cancel wins from a given version.
- at most one active exclusive workspace writer lease exists per workspace.
- frozen run bindings never change after environment creation.
- outbox sequence is strictly contiguous per durable stream.
