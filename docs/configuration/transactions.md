> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Transactional Configuration Activation

Agents/users propose immutable config generations; no component edits live configuration files in place.

Pipeline: schema validation → contract compatibility → capability negotiation → isolated instantiation → conformance/smoke workflow → health gate → atomic active-generation pointer update.

Existing runs continue with their persisted `ResolvedRunEnvironment`. If activation health checks fail, the previous healthy configuration generation may be reactivated. This rollback exists only as a safety mechanism for failed configuration activation.

A health failure in a pinned active run does not authorize silent cross-adapter rebinding. The run follows its persisted failure/recovery policy. If a frozen binding is itself a versioned router/provider-group, failover inside that pre-resolved policy is allowed and the actual target is recorded per operation.
