> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Cloud Runtime Profile Deployment

Cloud execution is a first-class deployment option from inception.

A cloud-oriented profile may bind run-scoped ports such as:

- `SandboxPort` → cloud compute/sandbox adapter.
- `WorkspacePort` → remote materialized workspace.
- `ArtifactStorePort` → S3/R2/Azure Blob.
- `MemoryStorePort` → Postgres/vector or another compatible backend.
- `ModelPort` → desired hosted or local-compatible provider.

The same agent loop and workflow semantics remain unchanged because they depend on port contracts, not vendor implementations. Before a profile can be activated, its adapters must satisfy capability negotiation, conformance tests, security policy, and smoke tests.

`KernelStorePort` remains bootstrap-global and is configured as part of the daemon deployment rather than as a run-scoped profile binding.
