> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Local Base Control API

## Transport

Tonic gRPC over a Unix-domain socket owned by the user, e.g. application support runtime directory with mode `0600`. The daemon refuses world/group-writable socket directories.

## MVP services

### `ControlApi`

- `SubmitCommand(CommandRequest) -> CommandResponse`
- `GetRun(GetRunRequest) -> GetRunResponse`
- `GetTask(GetTaskRequest) -> GetTaskResponse`
- `GetEffect(GetEffectRequest) -> GetEffectResponse`
- `GetRunGraph(GetRunGraphRequest) -> GetRunGraphResponse`
- `GetActiveConfig(...)`
- `ListAdapters(...)`
- `RespondApproval(...)`
- `Health(...)`

### `EventApi`

- `ReadStream(stream_key, from_sequence, limit)`.
- `Subscribe(stream_key/filter, after_cursor)` server-streaming.

On durable subscription lag, server ends the stream with a resumable cursor/error and client reconnects using Event Journal.

## Authentication for local MVP

Unix socket filesystem ownership is the first transport boundary. The server derives the local principal from the peer OS identity (macOS peer credentials such as `getpeereid`) and configured daemon owner; it must not trust an arbitrary `principal_id` claim from request bytes. The request actor ID is validated/bound under that principal. Do not design command semantics such that future remote authentication requires changing the command model.

## State changes

API handlers never open DB transactions directly. They construct typed `KernelCommand`s and invoke Command Coordinator.
