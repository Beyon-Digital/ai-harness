# Task GC-1 — Unify the contract snapshot

**Status:** DONE
**Owner:** agent-gc1
**Commit:** a79e9b0 — `docs(contracts): unify snapshot, add command payloads and typed decisions [GC-1]`

## What was implemented

1. Deleted `agent-os-microkernel-mvp-buildpack/contracts/control-api/control.proto` (duplicate `agentos.spec.v1` package and symbol set; D1). Repo-root `spec/control-api/control.proto` untouched.
2. Created `agent-os-microkernel-mvp-buildpack/contracts/control-api/commands.proto` with exactly 18 payload messages: the 16 catalogued commands in `specs/command-catalog.md` plus `RollbackConfigGeneration` and `ResolveBlockedRun` from D9. `command_type` is the fully-qualified protobuf message name (D2). `agent_spec_ref` uses `VersionedRef`, `condition` uses `DependencyCondition`, `resolved_environment` uses `ResolvedRunEnvironment`, `expected_effect_state` uses `EffectState`; optional ids/digests are `string`, payloads are `bytes`.
3. Rewrote the decision section of `contracts/protocols/agent_loop.proto`: `LoopDecision` now carries the fencing tuple (`run_id`, `run_revision`, `loop_epoch`, `step_sequence`, `input_event_cursor`, `turn_id`, `decision_id`) and a `oneof decision` over six typed variant messages: `Complete`, `Fail`, `Wait`, `SpawnAgent`, `InvokeEffect`, `RequestApproval`.
4. Added `AgentRun.output_ref = 13` and `AgentRun.current_turn_id = 14` in `contracts/domain/core.proto`.
5. Updated `contracts/control-api/mvp_control.proto`: `Subscribe` now streams `EventStreamFrame` with `oneof frame { EventEnvelope event = 1; LagNotice lag = 2; }` plus `LagNotice { stream_key, resume_sequence }` (D15); `stream_key`/`after_sequence` kept on `SubscribeEventsRequest`.
6. Rewrote `contracts/README.md` to state that `mvp_control.proto` is the single normative Control API, document the D2 `command_type` naming rule, and list the snapshot inventory.
7. Rewrote `SOURCE_CORRECTIONS.md` to document the `control.proto` removal, the added/updated contract files, and the previously undocumented `mvp_control.proto`, `entities.proto`, and `adapter_frames.proto`.

No prior partial GC-1 work existed in the leased files at start (all seven were pristine relative to HEAD). No lock regeneration attempted (N3; GC-7 owns `contract-lock.sha256`).

## Acceptance criteria

| # | Criterion | Result | Evidence |
|---|---|---|---|
| R1.1 | No duplicate message/service names across snapshot protos | met | duplicate `uniq -d` command below prints nothing |
| R1.2 | `control.proto` removed from pack; `mvp_control.proto` remains | met | `ls contracts/control-api` -> `commands.proto mvp_control.proto` |
| R1.3 | `mvp_control.proto` only Control API service definition | met | `grep -rl "service MvpControlApi\|service ControlApi" contracts --include='*.proto'` -> only `mvp_control.proto` |
| R2.1 | `commands.proto` defines exactly 18 messages | met | `grep -c '^message' .../commands.proto` -> `18` |
| R2.3 | `agent_loop.proto` defines the six decision messages | met | `grep -cE '^message (Complete\|Fail\|Wait\|SpawnAgent\|InvokeEffect\|RequestApproval) '` -> `6` |
| N3 | No lock regeneration attempted here | met | `contract-lock.sha256` unmodified in commit |
| — | No file outside `files:` changed | met | commit touches only the 7 leased paths |

## Verification output

```text
$ grep -rhoE '^(message|service) [A-Za-z0-9_]+' agent-os-microkernel-mvp-buildpack/contracts --include='*.proto' | sort | uniq -d
(no output)

$ python3 tools/validate_repo.py
OK: 231 markdown, 13 canonical ports, no link/schema/catalog errors

$ for n in CreateSession CreateTaskRun SubmitLoopDecision RollbackConfigGeneration ResolveBlockedRun; do grep -rq "message $n" agent-os-microkernel-mvp-buildpack/contracts/control-api/commands.proto || echo "missing $n"; done
(no output)

$ grep -c '^message' agent-os-microkernel-mvp-buildpack/contracts/control-api/commands.proto
18

$ grep -cE '^message (Complete|Fail|Wait|SpawnAgent|InvokeEffect|RequestApproval) ' agent-os-microkernel-mvp-buildpack/contracts/protocols/agent_loop.proto
6

$ ls agent-os-microkernel-mvp-buildpack/contracts/control-api
commands.proto  mvp_control.proto
```

Additional self-check: brace-balanced proto parse with per-message duplicate field-number scan -> `proto-selfcheck OK`.

## Files changed

- `agent-os-microkernel-mvp-buildpack/contracts/control-api/control.proto` (deleted)
- `agent-os-microkernel-mvp-buildpack/contracts/control-api/commands.proto` (created, 18 messages)
- `agent-os-microkernel-mvp-buildpack/contracts/control-api/mvp_control.proto` (subscription frames)
- `agent-os-microkernel-mvp-buildpack/contracts/protocols/agent_loop.proto` (typed decisions)
- `agent-os-microkernel-mvp-buildpack/contracts/domain/core.proto` (`output_ref`, `current_turn_id`)
- `agent-os-microkernel-mvp-buildpack/contracts/README.md` (authority + inventory)
- `agent-os-microkernel-mvp-buildpack/SOURCE_CORRECTIONS.md` (documented changes)

## Concerns

- `CreateApprovalRequest` and `MarkConfigTested` have no field shapes in `specs/command-catalog.md`; fields were mirrored from the durable `approval_requests` table (plus `approvals.md` digest inputs) and from the catalog's "generation ID/digest + test report digest/result" wording respectively. `requested_budget`, `child_request`, `effect_claim`, and `approval_draft` have no canonical shape and are typed `bytes`.
- `contract-lock.sha256` now intentionally mismatches the snapshot until GC-7 regenerates it; the buildpack validator will report mismatches until then.
