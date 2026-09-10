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

## Inception version

`schema_version` is exactly `1`.
