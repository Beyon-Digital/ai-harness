> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Identity and Delegation

## Identity layers

- `principal`: authenticated owner/system authority.
- `actor`: actual client/agent/adapter/tool making request.
- `run`: execution context.
- `delegation_chain`: ordered authority path.

## Delegation invariant

Each child hop may receive only a subset of capabilities and resource budget represented by its parent hop. Store explicit grant IDs; do not infer authority solely from ancestry.

## Confused deputy protection

When an untrusted child invokes a helper/tool owned by a more privileged parent, effective authority is the intersection of:

- child's delegated grants;
- tool's allowed capability set;
- ancestor constraints;
- target/resource policy.

The helper never inherits the parent's full privilege merely because the parent registered it.
