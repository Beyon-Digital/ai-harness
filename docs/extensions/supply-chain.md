> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Extension Supply-Chain Integrity

Executable identity includes source + manifest + dependency lock/inventory + compiled assets.

Examples: `uv.lock`/locked Python environment, `package-lock.json`/`pnpm-lock.yaml`, compiled WASM/assets. Installation creates an immutable bundle and kernel-computed digest.

Approval, conformance reports, config generations, audit events, and every launch pin that exact digest. Any dependency/source change produces a new digest and requires new validation/approval according to policy.

## Runtime environment identity

For interpreted/native-dependent extensions, reproducible executable identity also includes the runtime environment: interpreter/runtime version, target OS/architecture, SDK/protocol version, and base image/environment digest where applicable. A source+lock digest alone is not assumed to produce the same executable behavior across arbitrary hosts.
