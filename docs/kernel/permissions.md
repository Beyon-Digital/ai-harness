> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Permission and Capability Engine

Capabilities are scoped authorities, not labels.

## Dimensions

Workspace/file scopes, network destinations, tool calls, secret use, agent spawning, config mutation, extension installation, external account actions, and resource budgets.

## Delegation

A child receives only an explicitly delegated subset. The full delegation chain is evaluated to prevent confused-deputy privilege amplification.

## Results

`Allow`, `Deny`, or `RequireApproval`, with reason, scope, expiry, actor/run bindings, and immutable approval request reference where applicable.
