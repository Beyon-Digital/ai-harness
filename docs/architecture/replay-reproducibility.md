> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Replay, Audit, and Reproducibility

The architecture guarantees **state reconstruction and historical replay**, not deterministic re-execution of nondeterministic systems.

## Historical replay

Read previously recorded events/results in their original stream order.

## State reconstruction

Reconstruct authoritative run/task/graph/effect state from KernelStore records and historical data.

## Reproducibility metadata

Where retention policy permits, persist exact AgentSpec/loop/tool/adapter digests, resolved capabilities, model/provider/model ID/parameters, input context references, workspace base snapshot, tool/model requests and results, config generation, and protocol/kernel versions.

A “re-run” means attempt the same declared logical environment and inputs. External models/web/APIs may have changed and are not guaranteed to return identical data.
