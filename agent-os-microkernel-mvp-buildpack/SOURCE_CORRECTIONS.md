# Source Contract Corrections Applied in This Build Pack

## Inception version normalization

The canonical inception docs describe both the configuration schema and extension manifest as version 1, but the source JSON Schemas contained stale `const: 2` values.

For this inception implementation pack:

- `config.schema_version == 1`.
- `extension.manifest_version == 1`.

These are inception-contract consistency corrections only.

## Effect enum value prefixing

protoc rejects duplicate value names within one package. The `agentos.spec.v1` snapshot declared `READ_ONLY` in both `WorkspaceAccessMode` (`contracts/domain/core.proto`) and `EffectClass` (`contracts/domain/effects.proto`), and `FAILED` / `CANCELLED` in both `RunState` (`contracts/domain/core.proto`) and `EffectState` (`contracts/domain/effects.proto`). This pack renames only the effects side:

- `EffectClass.READ_ONLY` -> `EFFECT_CLASS_READ_ONLY`;
- `EffectState.FAILED` -> `EFFECT_STATE_FAILED`;
- `EffectState.CANCELLED` -> `EFFECT_STATE_CANCELLED`.

`contracts/domain/core.proto` is unchanged. The canonical repo-root `spec/` tree keeps the unprefixed names; this pack snapshot diverges so protobuf code generation works.

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

## Classification vocabulary reconciliation

The inception event contract used a sensitivity value named `PRIVATE` and four
retention values (`EPHEMERAL`, `SESSION`, `AUDIT`, `DURABLE`). This pack
reconciles the classification vocabulary to one spelling across the proto,
domain mirror, catalog, schema, and every Rust usage:

- `Sensitivity.PRIVATE` is renamed `Sensitivity.CONFIDENTIAL`.
- Retention narrows to three values, `EPHEMERAL` (1), `STANDARD` (2), and
  `AUDIT` (3), removing `SESSION` and `DURABLE`.

The canonical repo-root `spec/events/event.proto` intentionally keeps the old
names for now; a future canonical-tree update should adopt this correction.

## Transition table correction

`specs/runtime-manager.md` allowed `Created -> Ready` as the only edge out of
`Created`, but cancellation of a never-started run has no cleanup to drain and
no persisted `ResolvedRunEnvironment`, so it must reach `Cancelled` without
fabricating a `Ready`. The pack's state table now reads
`Created -> Ready | Cancelled`, where `Created -> Cancelled` is
cancellation-only.

The runtime implementation and its state-machine suite mirror this correction
(`runtime/src/state.rs`, `runtime/tests/state_machine.rs`); the repo-root
canonical runtime-manager doc is unchanged.
