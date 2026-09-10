> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# RunGraph

RunGraph is a small kernel primitive for execution relationships, not a workflow engine.

## Nodes and edges

Nodes are `AgentRun`s. Edges are:
- `ParentChild` for delegation/ancestry.
- `Dependency` with an explicit satisfaction condition.

Default dependency condition: `CompletedSuccessfully`. Other supported conditions must be explicit (`AnyTerminal`, `SpecificOutcome`).

## Transactional rules

- Dependency validation/insertion is serialized or otherwise transactionally protected against concurrent cycle creation.
- Dependency edges can be mutated only before the dependent run is execution-claimed unless a future explicit graph-rewrite protocol is used.
- Ready-run eligibility and claim happen atomically.
- Parent cancellation increments a cancellation epoch; child creation must present the observed parent epoch, preventing a racing child from escaping cancellation.

## Kernel operations

`spawn_run`, `add_dependency`, `claim_ready_run`, `wait_for`, `wait_for_any`, `wait_for_all`, `cancel_run`, `get_parent`, `list_children`, `collect_outputs`.

The loop decides *why* and *when* to spawn/delegate; the kernel owns process/state truth.
