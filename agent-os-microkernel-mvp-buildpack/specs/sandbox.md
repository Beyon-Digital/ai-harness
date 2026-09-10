> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Sandbox Manager

## Trust tiers

- T0 trusted: local process permitted; not a security boundary.
- T1 constrained: not required for MVP.
- T2 untrusted: kernel contract enforced, but no qualifying adapter ships in this microkernel MVP.
- T3 hostile: out of scope.

## Required behavior

If a run/extension requests T2/T3 and registry has no adapter passing required capability negotiation, return `CAPABILITY_UNSUPPORTED`. **Do not silently fall back to T0.**

## T0 adapter

Provides process execution with:

- explicit cwd/workspace;
- environment allowlist;
- deadline/cancellation;
- process-tree tracking/cleanup where possible;
- stdout/stderr capture;
- no claim of filesystem/network isolation.

The sandbox manager records the negotiated trust tier in the frozen run environment.
