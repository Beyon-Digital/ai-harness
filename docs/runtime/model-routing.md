> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Model Providers and Routing

`ModelPort` abstracts provider mechanics. `ModelRouter` is behavioral/product policy.

Provider concerns: requests/streaming, tool-call encoding, structured output, reasoning controls, multimodal, usage/cost metadata, provider errors, status/reconciliation if offered.

Router concerns: quality/cost/latency tiers, task specialization, local/cloud preference, fallback.

All billable/nondeterministic model invocations are represented as effects with stable operation IDs; retries after uncertainty follow explicit effect policy.
