> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# RunGraph

## Relationship representation

- Parent/child ancestry is stored once as `runs.parent_run_id` and exposed by RunGraph queries. Do **not** duplicate it into a second edge table.
- Dependency relationships are stored in `run_dependencies`; a target cannot be claimed until its source satisfies the declared condition.

Dependency conditions in MVP:

- `completed_successfully` — default;
- `any_terminal`;
- `completed_or_cancelled`.

Do not add arbitrary expression DSLs in the MVP.

## Edge mutation rules

Dependency edges may target only runs in `Created` or unclaimed `Ready`. Once a run is claimed/running, dependency topology for that run is frozen.

Inside one `BEGIN IMMEDIATE` transaction:

1. load graph head/revision;
2. validate source/target exist in same task graph;
3. validate target state is mutable;
4. detect reachability from target to source using recursive CTE/graph read;
5. reject if cycle would form;
6. insert dependency;
7. increment graph revision;
8. emit outbox event.

## Ready claim

A run is claimable iff:

- state is `Ready`;
- not cancelled/current cancellation epoch valid;
- all dependencies satisfy their declared condition;
- required resources/profile bindings remain valid;
- no unexpired claim exists.

Eligibility + claim token + state transition must be one transaction.

## Cancellation epoch

Cancelling a parent increments its `cancellation_epoch`. Child creation includes `observed_parent_cancellation_epoch`; mismatch rejects creation. Subtree cancellation runs inside a transaction for the currently known graph; racing child creation cannot escape because it must observe the new epoch.
