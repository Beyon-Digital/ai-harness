> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# ResolvedRunEnvironment

Created exactly once when a run is started/fully bound and then immutable.

Persist:

- AgentSpec ID/version/digest.
- AgentLoop ID/version/bundle digest.
- active config generation ID/digest.
- every run-scoped port → exact adapter ID/version/digest + negotiated capabilities.
- workspace URI/base revision/access mode/lease lineage.
- model provider/model ID/parameters when model exists.
- context/memory/router/tool/skill bundle identities when present.
- capability grant IDs and approval IDs.
- kernel version and protocol versions.

The run always references this record. Queries/audit never reconstruct historical bindings from current registry/config.

If a pre-resolved router/provider group performs failover internally, each actual operation stores the concrete target identity in its effect/operation metadata.
