> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# WorkspacePort

**Binding scope:** `run-scoped`  
**Normative ID:** `workspace@1`

## Purpose

Stable semantic contract for replaceable workspace implementations.

## Required operations

create/open/read/write/list/remove; acquire/transfer lease; fork; snapshot; merge/apply.

## Candidate adapters

local directory; Git worktree; container/remote workspace.

## Capabilities

locking/versioning/conflict detection, git, snapshot, shared coordinated write.

## Common call context

Every external call carries operation/call ID, actor/run/delegation context, deadline/cancellation, correlation/causation IDs, and granted capabilities. Effectful operations additionally carry stable `effect_id`/`operation_id`, request hash, and fencing metadata where applicable.

## Versioning

Breaking semantic changes create `workspace@2`; additive optional capabilities may remain compatible with `@1`.

## Lease enforcement capabilities

Adapters declare whether exclusive write leases are `cooperative` or `enforced`. `SHARED_COORDINATED_WRITE` additionally requires locking/version/conflict capabilities. Strongly isolated T2/T3 workflows must not rely on cooperative-only writer revocation.
