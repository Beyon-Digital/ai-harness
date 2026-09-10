> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Binding Scopes and Immutable Run Environments

Not every adapter belongs in a per-run runtime profile.

## Bootstrap-global

Correctness/root identity services:
- KernelStore.
- daemon identity/lease/fencing backend.
- root crypto material.

They change only through controlled maintenance procedures.

## Generation-global

Daemon-wide active service generation:
- Event Journal.
- kernel Message Queue, if externalized.
- Secret Store.
- remote Transport.
- scheduler backing infrastructure.

Changes use transactional configuration activation and health-gated service replacement semantics.

## Run-scoped

Safe to resolve per `AgentRun`:
- AgentLoop.
- Sandbox.
- Workspace.
- Artifact Store.
- Model Provider/Router.
- Memory Store/Strategy.
- Context service/strategy.
- Tool Runtime/tools/skills.

## Immutable `ResolvedRunEnvironment`

At run start the kernel resolves and persists exact bindings. They do not change for that run when a later config generation activates.

Persist IDs, versions, bundle digests, negotiated capabilities, config/profile generation, model parameters, workspace base snapshot/commit and authority mode, security grants/approval references/delegation chain, and kernel/protocol versions.

This supports audit, historical reconstruction, and controlled re-run/testing without guessing which infrastructure was used.
