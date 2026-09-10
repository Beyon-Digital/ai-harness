> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Process Supervisor

## Spawn contract

Input includes:

- expected immutable bundle digest;
- executable path from installed bundle;
- argv;
- minimal environment allowlist;
- working directory/resource handles;
- sandbox association/trust tier;
- stdout/stderr capture policy;
- restart policy;
- protocol version.

## Private IPC

Use an `AF_UNIX SOCK_STREAM` socketpair created by the parent. Map the child end to a fixed inherited FD (e.g. 3) during spawn. The child does not listen on a discoverable socket.

Handshake:

1. kernel sends `AdapterBootstrap` containing daemon instance/fence, adapter instance, expected bundle digest, nonce, protocol version;
2. child replies `AdapterHello`;
3. supervisor verifies exact instance, protocol, adapter identity, and digest;
4. only then register the live process endpoint.

Stdout/stderr remain separate from protocol framing.

## Failure

Track process ID/start identity, exit reason, last health heartbeat, and current adapter instance ID. A process restart creates a new adapter instance and invalidates old in-flight protocol sessions.
