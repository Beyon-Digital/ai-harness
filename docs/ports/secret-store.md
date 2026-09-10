> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# SecretStorePort

**Binding scope:** `generation-global`  
**Normative ID:** `secret-store@1`

## Purpose

Stable semantic contract for replaceable secretstore implementations.

## Required operations

metadata/resolve; put/rotate/revoke where authorized.

## Candidate adapters

macOS Keychain; cloud secret manager.

## Capabilities

versioning, rotation, scoped credentials, audit metadata.

## Common call context

Every external call carries operation/call ID, actor/run/delegation context, deadline/cancellation, correlation/causation IDs, and granted capabilities. Effectful operations additionally carry stable `effect_id`/`operation_id`, request hash, and fencing metadata where applicable.

## Versioning

Breaking semantic changes create `secret-store@2`; additive optional capabilities may remain compatible with `@1`.
