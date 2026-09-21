> **Navigation:** [Home](../index.md) · [Guides](README.md)

# Web GUI + real LLM loop (post-MVP bring-up)

The MVP control surface (`agentctl` + UDS gRPC) is CLI-only. For browser
use, `agentgw` bridges the daemon's Unix-socket APIs to a localhost
JSON REST + SSE surface and serves an embedded dashboard.

## Pieces

- **`agentd`** — the kernel daemon (unchanged).
- **`agentgw`** — axum HTTP gateway: REST mirror of the control API, an
  SSE `Subscribe` bridge, a `GET /api/runs/{id}/decisions` endpoint that
  decodes `LoopDecisionAccepted` events into readable JSON, and a small
  JSON index (`<socket>.agentgw-index.json`) of sessions/tasks/runs/specs
  created through it (the frozen MVP contract has no list RPCs, so the
  gateway remembers ids).
- **`fixtures/openrouter-loop`** — the real-inference `agent_loop@1`
  adapter. It performs no network I/O: each model call is emitted as a
  durable `invoke_effect` decision (`model.chat`) so the kernel routes it
  through the Effect Coordinator — fenced, idempotent, reconciled.
- **`fixtures/openrouter-effect`** — the `effect.execute@1` adapter that
  actually calls OpenRouter chat completions. Results are recorded in a
  durable store keyed by `operation_id`, so a retried `execute` replays
  the recorded answer and `status` reconciles after a crash.
- **`fixtures/local-memory`** — an `effect.execute@1` adapter serving
  durable `memory.*` ops (`memory.put/get/delete/list/search`) on a JSON
  store under the runtime dir — the reachable Phase-10 memory path while
  the contract's `MemoryStorePort` has no kernel caller.
- **`scripts/make-adapter-bundle.sh`** — assembles a bundle dir
  (manifest + binary + `bundle.lock`) for `--adapter-bundle`.

## Bring-up

```sh
cargo build --workspace
scripts/make-adapter-bundle.sh \
  target/debug/openrouter-loop \
  fixtures/openrouter-loop/adapter.manifest.json \
  bundles/openrouter-loop
scripts/make-adapter-bundle.sh \
  target/debug/openrouter-effect \
  fixtures/openrouter-effect/adapter.manifest.json \
  bundles/openrouter-effect

OPENROUTER_API_KEY=sk-or-... \
  agentd --runtime-dir run \
         --config config/openrouter.yaml \
         --adapter-bundle bundles/openrouter-loop \
         --adapter-bundle bundles/openrouter-effect

agentgw --socket run/control.sock --listen 127.0.0.1:7740
# open http://127.0.0.1:7740/
```

`config/openrouter.yaml` binds `agent_loop` to the OpenRouter loop and
`effect.execute` to `openrouter-effect` — a run's model call is a
durable `model.chat` EffectRecord (visible via `agentctl get-effect` or
the run's decision list), not a side channel.
`config/default.yaml` keeps the deterministic `fixture-loop` +
`fixture-effect` for tests; `config/memory.yaml` adds a `local-memory`
profile whose `effect.execute` is the memory adapter.

The daemon propagates `OPENROUTER_API_KEY`, `OPENROUTER_MODEL`
(default `openrouter/free`), `OPENROUTER_BASE_URL`, `OPENROUTER_SITE`
and `OPENROUTER_APP_NAME` into loop- and effect-adapter children only —
nothing else crosses the env boundary.

## Turn flow (model via effects)

1. Turn N: loop sees no settled `model.chat` effect → emits
   `invoke_effect` → run parks in `waiting_tool`; the coordinator
   prepares, claims, dispatches to `openrouter-effect`, commits.
2. Turn N+1: the kernel feeds the run's settled effect outcomes into
   `LoopInput.events` (JSON array) → the loop decodes the committed
   `result_ref` data URI and parses the assistant's reply as the next
   decision — `complete` / `fail` / `wait` / `request_approval`.

## Run lifecycle in the GUI

1. Sessions tab: create a session (id optional — uuidv7 minted), put an
   agent-spec revision whose body names the profile:
   `{"runtime_profile_name":"local-trusted"}`.
2. Runs tab: create a run — pick the submitted spec from the dropdown
   (fills id/version/digest), keep `local-trusted` profile, write the
   task prompt.
3. Watch the event stream; the run detail panel shows the decisions
   list (`invoke_effect model.chat` → `complete`) and the decoded
   answer under **output**.
4. If the model answers `{"request_approval": ...}`, the run parks in
   `waiting_human` and the request appears under Approvals —
   approve/deny resumes or fails the run.

## Model decision vocabulary

The loop adapter parses the committed model reply as exactly one JSON
object:

```json
{"complete": {"output": "<final answer>"}}
{"fail": {"reason_code": "<snake_case>"}}
{"wait": {"reason": "<what it waits on>"}}
{"request_approval": {"operation": "<op>", "reason": "<why>"}}
```

Prose replies are treated as a `complete` whose output is the raw text.

## Memory ops

`config/memory.yaml` adds a `local-memory` profile (extends
`local-trusted`) whose `effect.execute` binding is the `local-memory`
adapter. Any loop that emits `invoke_effect` with a `memory.*` payload —
e.g. `{"op":"memory.put","namespace":"n","memory_id":"id","record":{...}}`
— gets durable, fenced memory writes; `memory.get/list/search` read
backs return `data:application/json;base64,` result refs. The store is
`run/fixture-store-<adapter-id>.json`.

## Backup + drain

- `agentd backup --runtime-dir <dir> --out <dir>` — consistent
  `VACUUM INTO` snapshots of `kernel.db` + `events.db` (safe while the
  daemon runs).
- SIGINT/SIGTERM drains first: workers stop claiming, in-flight turns
  finish, the outbox flushes, the lock releases
  (`limits.shutdown.drain_deadline_ms`).

## Adapter lifecycle

Bundles live under `<runtime-dir>/installed-adapters/` once installed —
`agentd` auto-registers every enabled bundle at boot.

- `agentd adapter install --runtime-dir <dir> --bundle <dir> [--check]` —
  verifies the bundle lock, copies it into `installed-adapters/<id>-<v>-<digest>`,
  optionally spawns a handshake+ping smoke check, and indexes it.
- `agentd adapter check --runtime-dir <dir> --bundle <dir>` — the same
  conformance smoke without installing.
- `agentd adapter list|enable|disable|remove --runtime-dir <dir>` — manage
  the `index.json` enable flags and on-disk copies.

## Observability surface

- `GET /api/metrics` — read-only counters over `kernel.db`/`events.db`
  (runs/effects/approvals/timers by state, decisions, adapter instances,
  outbox + journal depth, active generation). Rendered in the GUI's
  **System** tab.
- `GET /api/config/generations` — the generation pipeline
  (proposed → validated → tested → active); rendered under **Config**.
- `GET /api/runs/{id}/environment` — the run's frozen resolved
  environment + adapter bindings; rendered in run detail. 404 for runs
  started without a spec.

Memory writes record provenance internally: each stored record is an
envelope `{record, sensitivity, provenance:{effect_id, operation_id,
fencing_token, written_at_ms}}`. `memory.put` accepts `sensitivity` of
`public|internal|confidential|secret` (default `internal`); an existing
record's sensitivity can only be raised — downgrades and cross-namespace
writes with mismatched class are rejected (`sensitivity_downgrade`).
Namespaces must match `[a-z0-9][a-z0-9._-]{0,63}`.

## Context strategy (per-generation `limits.context`)

The run's frozen generation caps what each turn's `LoopInput` carries:

```yaml
limits:
  context:
    max_state_bytes: 65536      # run-state snapshot fed to the loop
    max_fed_events: 64          # settled-effect entries per turn
    max_fed_event_bytes: 65536  # serialized events batch
```

Oversized state is truncated on a UTF-8 boundary; an oversized events
batch keeps the most recent entries (dropping all if none fit). Absent
`context:` the defaults above apply; existing documents keep parsing.

## Adapter sandbox tiers (`runtime.isolation`)

A bundle manifest may request kernel-level isolation:

```json
"runtime": {"type": "process", "entrypoint": "...", "isolation": "user-ns-no-net"}
```

`none` (default) | `user-ns` (fresh user+IPC namespaces, network kept) |
`user-ns-no-net` (also isolates the net namespace — no sockets). Spawn
runs through `unshare --user --map-root-user --ipc [--net]`; if userns
creation is unavailable the daemon logs a warning and runs unsandboxed.
`agentd adapter check` exercises the same isolation at smoke time.

## Device registry (per-device gateway auth)

```bash
agentgw device add    --devices-file devices.json --label laptop   # prints a gwdev_… token once
agentgw device list   --devices-file devices.json
agentgw device revoke --devices-file devices.json <id-prefix>
agentgw --socket … --listen … --auth-token MASTER --devices-file devices.json
```

The file stores only `sha256:` token hashes. `Authorization: Bearer
<token>` accepts the master token (`admin`) or any enabled device token;
`revoke` applies to the very next request — no restart. A
device-authenticated `POST /api/approvals/respond` binds that device id
into the durable `device_id` field.

## Historical replay

`agentctl replay RUN_ID` pages the `run/<id>` event stream and decodes
every `LoopDecisionAccepted` payload into `{kind, decision_id, turn_id,
step_sequence}` — reconstruct what the loop decided without it running.
