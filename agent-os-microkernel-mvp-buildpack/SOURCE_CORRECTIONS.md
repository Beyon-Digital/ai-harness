# Source Contract Corrections Applied in This Build Pack

## Inception version normalization

The canonical inception docs describe both the configuration schema and extension manifest as version 1, but the source JSON Schemas contained stale `const: 2` values.

For this inception implementation pack:

- `config.schema_version == 1`.
- `extension.manifest_version == 1`.

These are inception-contract consistency corrections only.

## Contract snapshot unification

- Removed `contracts/control-api/control.proto`. It declared the same `agentos.spec.v1` package and the same six message names (`CommandRequest`, `CommandResponse`, `GetRunRequest`, `GetRunResponse`, `GetEffectRequest`, `GetEffectResponse`, `ApprovalResponseRequest`, `ApprovalResponseResult`) as `contracts/control-api/mvp_control.proto`, so the snapshot could not compile. `mvp_control.proto` is the single normative Control API definition; the removal applies to this pack snapshot only.
- Added `contracts/control-api/commands.proto`: one payload message per command in `specs/command-catalog.md` (16) plus the two commands added by design D9 (`RollbackConfigGeneration`, `ResolveBlockedRun`), for 18 messages total. `CommandRequest.command_type` is the fully-qualified name of one of these messages (design D2).
- Replaced the free-string `decision_type` and `bytes payload` in `contracts/protocols/agent_loop.proto` with the fenced `LoopDecision` tuple plus a `oneof decision` over `Complete`, `Fail`, `Wait`, `SpawnAgent`, `InvokeEffect`, and `RequestApproval`.
- Added `AgentRun.output_ref = 13` and `AgentRun.current_turn_id = 14` in `contracts/domain/core.proto` so loop output can be persisted on the run.
- Replaced the `EventEnvelope` subscription stream element with `EventStreamFrame` (an `EventEnvelope` or a `LagNotice`) in `contracts/control-api/mvp_control.proto` so a lagging subscriber receives a resumable cursor (design D15).

The following snapshot files existed but were previously undocumented:

- `contracts/control-api/mvp_control.proto`;
- `contracts/domain/entities.proto`;
- `contracts/protocols/adapter_frames.proto`.

The repo-root canonical sources under `spec/` are unchanged.
