> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Adapter Contract

Every adapter registration is bound to an immutable extension bundle.

Required metadata: adapter ID/version, bundle digest, dependency digest/inventory, manifest digest, implemented port/version(s), binding scope, runtime type, config schema digest, declared semantic capabilities, required security authorities, conformance result digest, and supported effect operations.

Generated infrastructure adapters use supervised process or WASM isolation; they do not load arbitrary native dynamic code into `agentd`.

## Privileged port policy

Not every port is equally open to generated adapters. `KernelStorePort` is trusted-native-only in the initial implementation. Other privileged daemon-global ports may require reviewed trust levels. The catalog defines extension policy in addition to binding scope.
