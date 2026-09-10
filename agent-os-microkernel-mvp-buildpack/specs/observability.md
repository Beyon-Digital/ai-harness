> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Observability

Use `tracing` spans with IDs but classify payloads before logging.

Required span fields where relevant:

```text
command_id correlation_id causation_id
principal_id actor_id run_id task_id session_id
effect_id adapter_id adapter_instance_id
run_revision loop_epoch step_sequence
fencing_epoch
```

Never log raw secret material. Prompt/tool payload logging is not needed in this MVP; event payloads may contain private test data and must carry sensitivity classification.

Required counters/gauges:

- command success/error/idempotent replay;
- active/nonterminal runs;
- outbox backlog;
- live subscriber lag/disconnects;
- effect states/unknown count;
- adapter process restarts;
- timer backlog;
- reservation counts;
- daemon fencing epoch.
