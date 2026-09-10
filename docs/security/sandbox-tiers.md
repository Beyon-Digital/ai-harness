> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Sandbox Security Tiers

## T0 — Trusted
Audited native/user-trusted code. Local process allowed. **Not a security boundary.**

## T1 — Constrained
User-trusted extension with scoped filesystem/process/resources; limited isolation.

## T2 — Untrusted
Generated/third-party code. Mandatory strong sandbox: default-deny host filesystem, explicit workspace mounts, default-deny network, no host/Docker/agent sockets, no arbitrary device access, process-tree containment, CPU/memory/PID/disk/time limits, descendant cleanup, scoped secret mechanism.

## T3 — Hostile
Arbitrary downloaded/generated code needing strongest available container/VM policy and stricter egress/input/output handling.

An adapter cannot advertise a tier it does not pass in conformance tests.

Strong workspace lease semantics for T2/T3 code require the sandbox/workspace stack to be able to revoke prior write authority; merely stopping new kernel tool calls is insufficient if a process retains writable mounts or file descriptors.
