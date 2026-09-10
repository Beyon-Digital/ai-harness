> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Extension Manifest

The manifest is declarative and never grants authority by itself.

It declares identity/version/kind, runtime/entrypoint, implemented ports/exports, requested filesystem/network/secret/spawn authorities, effect claims, config schema, and dependencies/lock metadata.

The kernel computes and records the immutable bundle digest. The adapter/extension cannot self-assert the digest used for approval.
