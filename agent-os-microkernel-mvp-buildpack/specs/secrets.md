> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Secrets Broker

External code never receives the daemon's full environment.

## Broker operations

- resolve secret metadata;
- `sign_or_act` for supported providers/actions;
- issue short-lived scoped material where backend supports it;
- raw secret material only for explicitly authorized trusted paths;
- audit every use with run/actor/target.

## MVP backends

1. `InMemorySecretStore` — testkit only.
2. `MacOsKeychainSecretStore` — local production backend.

The process supervisor passes only specifically brokered secret material required for one operation. Secret values must not be serialized into ordinary logs/events.
