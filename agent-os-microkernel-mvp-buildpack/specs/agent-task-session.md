> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)

# AgentSpec, Session, Task, and Run Creation

## AgentSpec

An AgentSpec is immutable per `(agent_spec_id, version)` and content-addressed by digest.

MVP body must be able to express:

```text
agent_spec_id
version
instructions/opaque behavior configuration
agent_loop_ref
runtime_profile_name
requested capabilities
resource budget defaults
optional pre-provisioned workspace policy/reference
metadata
```

The kernel stores the opaque specification body plus extracts/validates the fields required for binding. Editing an agent creates another version; it never mutates an existing version used by a historical run.

## Session

A Session is an interaction/history grouping owned by one principal. It does not itself grant capabilities. Session creation is a simple idempotent command.

## Task

A Task is logical requested work. The MVP does not give Task a complex mutable status machine; task outcome is derived from its Run attempts. Task payload is immutable after creation.

## AgentRun creation

`CreateTaskRun` atomically creates:

- a Task when the command requests a new task;
- an AgentRun in `Created`;
- parent relationship through `parent_run_id` if applicable;
- initial delegation/budget references;
- graph revision update for child creation;
- outbox events;
- idempotency outcome.

A run cannot transition `Ready` until an exact `ResolvedRunEnvironment` is persisted. The MVP may bind an existing/pre-provisioned workspace URI. Automatic generic workspace provisioning is not required for root runs; child workspace forks are handled by Workspace Coordinator.

## AgentSpec resolution

Binding resolves an exact AgentSpec `(id, version, digest)`. Requests that specify only an ID use an explicitly configured default version at binding time, and the resolved version/digest is frozen into the run environment.
