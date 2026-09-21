# Task Report: API-002

- **Spec**: control-api command/query surface (specs/control-api.md)
- **Task**: API-002 — typed command dispatch + read-side query RPCs
- **Status**: DONE
- **Commits**: see git log for `devin/1789944697-support-wave`
- **Branch**: devin/1789944697-support-wave

## What was implemented

`ControlApiService` now carries `store: Arc<dyn KernelStore>` +
`ids: Arc<dyn IdProvider>` alongside the coordinator. Every query RPC
(`GetRun`, `GetTask`, `GetEffect`, `GetRunGraph`, `GetActiveConfig`,
`ListAdapters`) opens `begin_read()` only — no query ever mutates.
`SubmitCommand` maps the request envelope to a typed `CommandEnvelope`
and dispatches through the Command Coordinator; `RespondApproval` builds
`agentos.spec.v1.RespondApproval` payloads and dispatches them through
the same coordinator path, so no public state-changing method performs
direct repository mutation.

`views.rs` projects store rows → protobuf: `run_view`, `task_view`,
`effect_view`, `environment_view`, `dependency_view`,
`config_generation_view`, `adapter_view`. Kernel errors map to stable
gRPC codes + `agentos.code` metadata (NotFound→NOT_FOUND,
Conflict→ALREADY_EXISTS, FailedPrecondition→FAILED_PRECONDITION,
PermissionDenied→PERMISSION_DENIED, Unavailable→UNAVAILABLE,
else UNKNOWN).

## Evidence

`cargo test -p control-api --test queries` (4 tests):
- `submit_command_replays_idempotently` — same idempotency key replays
  the recorded result without re-executing the handler (counter=1).
- `query_not_found_maps_to_not_found_status` — GetRun/GetTask/GetEffect/
  GetActiveConfig on missing ids → NOT_FOUND.
- `approval_digest_mismatch_over_api` — RespondApproval with wrong digest
  → ALREADY_EXISTS over the wire; correct digest succeeds.
- `health_reports_fencing_config_and_outbox` — health exposes daemon
  epoch, active config generation, outbox backlog.

`cargo test -p control-api --test uds_api` — 7 existing RPC tests still
pass with the new constructor.
