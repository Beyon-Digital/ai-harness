> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Architecture Overview

## Thesis

The platform is a **trusted Rust control plane with a polyglot extension plane**.

- Kernel = law/mechanism/invariants.
- Adapters = machinery implementing fixed semantics.
- Agent loops/strategies = policy deciding what to do next.

## Trusted kernel

`agentd` owns identity, fencing, command linearization, run state, RunGraph, effect coordination, resource reservations, permissions, secrets mediation, process/sandbox control, adapter registry, scheduler, configuration generations, KernelStore, outbox, live event dispatch, and recovery.

## Replaceable run behavior

Each `AgentRun` may independently bind Hermes, Codex, ReAct, research, custom, or generated loops and its own model/context/memory/tool adapters. These bindings are resolved once and persisted as an immutable `ResolvedRunEnvironment`.

## Deployment

The authoritative runtime defaults to the Mac. Remote phone/web access uses an authenticated outbound relay/tunnel path; the cloud relay is not the default owner of conversations, memories, workspaces, or tool execution.

## Critical invariants

1. One transactional KernelStore mutation owns authoritative state changes.
2. Durable events originate from the same transaction via outbox.
3. External side effects are prepared durably before dispatch.
4. Stale loop/effect executors are fenced.
5. Untrusted extension code cannot bypass the capability/sandbox boundary.
6. Active runs never silently rebind when config changes.
