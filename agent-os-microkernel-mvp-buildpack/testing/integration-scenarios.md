> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# End-to-End Integration Scenarios

## Scenario A — Basic run

Create session → register fixture AgentSpec/loop → create task/run → resolve local profile → persist frozen environment → claim run → fixture loop returns `Complete` → run completes → query run/events.

## Scenario B — Child delegation

Parent fixture loop requests two children. Kernel creates graph edges and isolated workspace forks. Children complete with artifact/workspace outputs. Parent receives child completion events and completes.

## Scenario C — Effect safety

Loop requests fixture external mutation. Effect is prepared, claimed, dispatched, adapter simulates success. Crash daemon before acknowledgement commit. Restart and adapter status lookup reconciles same operation ID to success without second side effect.

Repeat with a non-reconcilable adapter: restart results in `Unknown` and `BlockedUnknownEffect`.

## Scenario D — Stale loop response

Issue loop turn, transition run/advance epoch before response, deliver old decision, assert `STALE_LOOP_DECISION` and no effects/children created.

## Scenario E — Config freeze

Start Run A under generation G1. Activate G2. Assert A retains G1 resolved bindings; Run B resolves G2.

## Scenario F — Sandbox fail-closed

Request T2 execution with only T0 adapter. Assert capability resolution failure and no process spawn.
