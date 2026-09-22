# Task Report: API-004

- **Spec**: agentctl local control CLI (specs/control-api.md)
- **Task**: API-004 — deterministic operator/dev client over the APIs
- **Status**: DONE
- **Commits**: see git log for `devin/1789944697-support-wave`
- **Branch**: devin/1789944697-support-wave

## What was implemented

`agentctl` (`lib.rs` for `run(args, socket) -> serde_json::Value`, thin
`main.rs`) connects only through the UDS Control/Event APIs. Commands:
`health`, `create-session`, `put-agent-spec`, `create-run`, `get-run`,
`get-task`, `graph`, `cancel-run`, `get-effect`, `resolve-effect`,
`adapters`, `config show|propose|test|activate|rollback`,
`approvals list|respond`, `events read|tail`. Output is always a single
JSON document; errors are JSON on stderr with non-zero exit.

Two support additions the CLI surface required:
- `ListApprovals` RPC (+ `ApprovalRequestView`) on the control API,
  backed by a new `SecurityRead::list_approvals` on kernel-store
  (sqlite + mock implementations).
- Config lifecycle command handlers (`config-engine::commands`):
  `ProposeConfigGeneration` (propose+validate), `MarkConfigTested`
  (kernel smoke test), `ActivateConfigGeneration`, `RollbackConfigGeneration` —
  each stages its catalogued `config/global` event in the same txn.

## Evidence

`cargo test -p agentctl --test cli` (3 tests) against a real daemon
socket wired to the sqlite store:
- `health_and_session_over_socket` — health fields + create-session
  returns a session id via SubmitCommand.
- `session_then_config_lifecycle_and_approvals` — propose → test →
  activate → show the default config, then approvals list.
- `not_found_and_json_parseable` — missing run errors cleanly; adapters
  and events read return parseable JSON.

Integration tests can therefore drive the full system without any
internal test-only DB mutation.
