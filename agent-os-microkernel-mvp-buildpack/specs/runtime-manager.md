> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Runtime Manager

## State machine

Allowed transitions:

```text
Created -> Ready
Ready -> Running | Cancelled
Running -> WaitingTool | WaitingChild | WaitingHuman | Suspended | Cancelling | Completed | Failed
WaitingTool -> Running | Cancelling | Failed
WaitingChild -> Running | Cancelling | Failed
WaitingHuman -> Running | Cancelling | Failed
Suspended -> Running | Cancelling | Failed
Cancelling -> Cancelled | Failed
```

System recovery may set `RecoveryDisposition` independently without inventing an illegal user-level state transition.

## Run fencing

Every accepted loop decision atomically compares:

```text
run_revision
loop_epoch
step_sequence
input_event_cursor
turn_id/issued turn record
```

If any mismatch, reject as stale with no mutation.

On accepted decision:

- persist decision ID for duplicate detection;
- apply resulting transition/effect/child/wait action;
- advance cursor/step as specified by decision type;
- increment run revision;
- emit events;
- commit atomically.

## Loop epoch

Increment `loop_epoch` whenever a loop process is rebound/restarted in a way that invalidates prior outstanding decisions. A response from a prior epoch is always stale.
