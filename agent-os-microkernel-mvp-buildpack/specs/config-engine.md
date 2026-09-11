> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Config Engine

## Binding scopes

### Bootstrap-global

- KernelStore.
- daemon identity/root local paths.

These are startup deployment configuration, not ordinary runtime config.

### Generation-global

- Event Journal.
- message queue (in-memory in MVP).
- Secret Store.
- transport/control binding.

For the MVP these bindings are **frozen for the lifetime of one daemon instance**. A live config activation that would change any of them is rejected with `DAEMON_RESTART_REQUIRED`; there is no hot rebind. This avoids splitting event history or changing security/transport machinery underneath active control-plane workers.

### Run-scoped

- sandbox;
- workspace;
- artifact store;
- model/memory/context/tool/loop ports when available.

## Config generation pipeline

```text
proposed immutable document
 -> schema validation
 -> adapter existence/version validation
 -> capability negotiation
 -> isolated smoke validation where applicable
 -> mark tested
 -> atomic active-generation CAS
```

A failed activation can reactivate the prior known-good generation. This is limited to safe activation of immutable runtime configuration generations. Live activation is restricted to changes that do not alter daemon-instance-frozen generation-global service bindings.

`RollbackConfigGeneration` is the explicit Control API command for that path. It is capability-gated by the config rollback grant, CASes the active-generation pointer against `expected_active_revision` back to a prior known-good generation, and emits `ConfigRolledBack`. Like activation, it does not touch running runs; each run keeps the `ResolvedRunEnvironment` it captured.

## Normative limits

Every threshold in this pack has exactly one machine-readable value in [`limits.yaml`](limits.yaml). That file is the source of truth for tests, defaults, and validators; no threshold may be hard-coded inline elsewhere.

The matching `limits` object is typed in `contracts/config/agent-os.schema.json` with the same key paths and integer minima, and `examples/default-config.yaml` mirrors every value. The GC-7 validator fails when a normative key is missing.

## Inception version

`schema_version` is exactly `1`.
