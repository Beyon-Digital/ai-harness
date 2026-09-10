> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Daemon Startup, Shutdown, and Recovery

## Startup

1. Resolve application directories.
2. Acquire OS exclusive lock.
3. Open KernelStore and validate inception schema.
4. Acquire/increment daemon fencing epoch.
5. Load active config generation.
6. Start Event Journal/outbox dispatcher.
7. Start recovery reconciliation for non-terminal effects/timers/resources/runs.
8. Start adapter/process supervisors.
9. Bind local Control API socket.
10. Mark daemon healthy/ready.

## Shutdown

1. stop accepting new external commands;
2. mark draining;
3. stop scheduling/claiming new runs/effects/timers;
4. stop issuing new loop turns;
5. request cooperative cancellation/drain of in-flight work;
6. terminate supervised processes as policy requires;
7. reconcile final responses that are still valid under fences;
8. commit resulting state;
9. drain outbox journal publishing to configured deadline;
10. close live subscribers;
11. invalidate/release daemon authority and OS lock;
12. close stores.

If shutdown deadline expires, correctness is preserved by durable state/recovery even if all projections are not flushed.
