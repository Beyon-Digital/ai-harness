> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Adapter Registry and Capability Negotiation

## Registration identity

```text
(adapter_id, version, bundle_digest)
```

Name/version without digest is insufficient.

For process bundles, use a deterministic `bundle.lock` containing sorted relative file paths plus SHA-256 for every executable/source/dependency-lock asset included in the installed bundle. The kernel verifies every listed file and computes the bundle digest over the canonical lock content plus manifest digest. Symlinks escaping the bundle root are rejected.

## Registry record

- manifest digest;
- bundle digest;
- runtime type;
- implemented port IDs/versions;
- declared capabilities;
- trust state;
- conformance state;
- enabled state/config references.

## Conformance report binding

A conformance report is a `conformance_reports` row in `kernel-store-schema.sql`, keyed by the exact identity `(adapter_id, adapter_version, bundle_digest)` and FK-bound to `adapter_registrations`. It records `report_digest`, `harness_version`, `result` with persisted literals `pass`, `fail`, `run_at_ms`, and `details`. Reports are immutable (`BEFORE UPDATE`/`BEFORE DELETE` triggers raise `immutable record`); `adapter_registrations.conformance_state` reflects the report outcome.

## Resolution

Given a port requirement:

1. require exact compatible port major version;
2. require all mandatory capabilities;
3. filter by trust/sandbox policy;
4. filter by config/profile pin if present;
5. deterministic tie-break by configured priority then immutable identity;
6. persist resolved identity/capabilities in `ResolvedRunEnvironment`.

The runtime never branches on vendor name such as `docker`/`redis`; it branches only on port semantics/capabilities.

## MVP external process protocol

Length-delimited protobuf messages over the supervisor-owned private socket. Implement request IDs, deadlines, cancellation frames, health ping/pong, and graceful shutdown.
