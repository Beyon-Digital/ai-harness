> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Identity, Daemon Lease, and Fencing

## Identity layers

- Principal: authenticated user/system authority.
- Actor: actual agent/extension/tool/client making the request.
- Run identity: execution context.
- Delegation chain: principal → parent run → child run → tool/extension.

## Daemon single-writer authority

Local SQLite deployments use an OS exclusive lock plus a durable daemon epoch. Remote KernelStore deployments require lease/fencing primitives. A takeover obtains a higher fencing epoch; stale instances can no longer commit authoritative work.

## External adapter identity

Prefer an inherited private socketpair/FD from the supervisor. If reconnectable IPC is required, use OS peer credentials plus a single-use bootstrap secret bound to daemon instance, adapter instance, expected executable/bundle digest, and protocol version.
