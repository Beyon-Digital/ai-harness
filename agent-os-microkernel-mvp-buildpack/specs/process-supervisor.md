> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Process Supervisor

## Durable adapter-instance record

Every spawned process has an `adapter_instances` row in `kernel-store-schema.sql`: `adapter_instance_id`, exact `(adapter_id, adapter_version, bundle_digest)` (FK to `adapter_registrations`), `daemon_instance_id`, `pid`, `process_start_identity`, `state` (`starting`/`ready`/`exited`/`failed`), `exit_reason`, `last_heartbeat_ms`, `started_at_ms`, and `ended_at_ms`. A restart creates a new row and never mutates the prior instance; old in-flight protocol sessions are invalid.

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
