---
name: testing-agent-os
description: How to test the agent-os Rust microkernel workspace in ai-harness — build/test commands, the UDS gRPC control surface (agentd/agentctl/agentgw), adapter bundles, and the OPENROUTER_API_KEY LLM path.
---

# Testing agent-os (ai-harness)

Rust workspace lives at `agent-os/` under the repo root (not the root itself).
Toolchain is pinned by `agent-os/rust-toolchain.toml` (1.94.0); run all cargo
commands from `agent-os/`.

## Commands

- Workspace tests: `cargo test --workspace` (~450 tests, ~2-3 min warm).
- Lint/fmt gates: `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check`.
- Hygiene: no `thread::sleep`/`tokio::time::sleep` under `crates/*/tests/` (CI N2 greps
  it — poll with `std::thread::park_timeout` instead).
- Repo validators: `python3 tools/validate_repo.py`,
  `bash agent-os/scripts/check-contract-mirror.sh`,
  `python3 agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` — all from repo root.

## Control surface (real transport)

- `agentd --runtime-dir <d> --config <yaml> --adapter-bundle <dir>... --json` — daemon on
  `<d>/control.sock` (UDS gRPC), `--json` gives structured logs.
- `agentctl <cmd> --socket <sock>` — CLI: create-session, submit-spec, create-run,
  cancel-run, read-events, respond-approval, list-approvals, get-effect, resolve-effect,
  list-adapters, get-config, task-graph, health.
- `agentgw --socket <sock> --listen 127.0.0.1:7740` — HTTP+SSE gateway + embedded web GUI
  at `/`. Keeps a local id index at `<sock>.agentgw-index.json` (contract has no list RPCs).
- `agentctl::connect(socket)` returns `Daemon{control,events}` gRPC clients — used by
  agentgw and tests.

## E2E test pattern (copy from crates/agentd/tests/)

- Spawn `target/debug/agentd` with a tempdir runtime dir; wait for `control.sock` to exist.
- Build command envelopes via `agentctl` helpers or by hand: CommandId + IdempotencyKey +
  ActorId + SystemIdProvider principals; entity ids are **UUIDv7** (`uuid::Uuid::now_v7()`).
- Adapter bundles: `scripts/make-adapter-bundle.sh <binary> <manifest> <outdir>` builds
  manifest + `bundle.lock` (`sha256:<hex> <relpath>` sorted lines). Hello must echo the
  expected `adapter_id`/`adapter_version`/`bundle_digest`.
- Config binding `name@version` — version pin must equal the manifest version exactly.
- `fixtures/agent-loop` is the deterministic loop adapter; `Wait{timer_id:""}` mints a
  timer and fires immediately (yield-next-turn). `request_approval` needs a proto-encoded
  `CreateApprovalRequest` with `request_id`/`request_digest` present (empty ok) and
  `run_id` stamped = the run, else it never resumes.
- Fixed daemon ids: PrincipalId `00000000-0000-7000-8000-0000000000dd`,
  ActorId `00000000-0000-7000-8000-0000000000ae`.

## GUI/daemon gotchas

- `pkill -f <pattern>` matches the shell's own cmdline — use `pkill -x agentd` / `pkill -x agentgw`.
- A run needs an agent spec whose body sets `{"runtime_profile_name":"<profile>"}` and the
  run must carry `agent_spec_id`/`spec_version`/`spec_digest`/`requested_profile`, else it
  sits in `failed_precondition` (the run worker retries with exponential backoff).
- GUI clicks: the runs table re-renders every 3s — click handlers are delegated on `tbody`.
- `agent_spec_id` (and other entity ids) MUST be UUIDv7 — a v4 UUID is rejected with
  "not a valid identifier". Mint with python:
  `python3 -c "import time,secrets; ms=int(time.time()*1000)&0xFFFFFFFFFFFF; v=(ms<<80)|(0x7<<76)|(secrets.randbits(12)<<64)|(0b10<<62)|secrets.randbits(62); h=f'{v:032x}'; print(f'{h[:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:]}')"`.
- `agent_specs` has `UNIQUE(digest)` — an identical spec body can only ever be stored once;
  a second id with the same body fails "conflict: storage constraint conflict". Vary the
  body (e.g. add a label field) when re-testing.
- Clicking a run row selects it and re-subscribes the SSE log to `run/<id>`; the detail
  panel auto-refreshes with the 3s index poll once selected.
- The same 3s poll rebuilds the run-detail innerHTML, so any expanded `<details>`
  (e.g. a decisions `payload`) collapses within ~3s while a run is selected — expand and
  screenshot immediately, or clear the interval to inspect payloads. (Newer builds skip
  the rebuild when the run fingerprint is unchanged, so payloads now survive the poll.
  An env-block "loading…" wedge after live transitions was found and fixed — env
  populates across waiting_tool→completed without re-clicking; if it regresses,
  re-clicking the run row repopulates it.)
- This box's display is 1600x1200 but the computer tool's coordinate space is 1024x768;
  clicks on small top-right targets (nav tabs) can land a few px off. Verify with
  `getBoundingClientRect`/`elementFromPoint` in the console; as a last resort
  `button.click()` works — the tab/summary handlers themselves are fine.
  Worked mapping this session: nav tabs ≈ (Runs 687, Sessions 783, Approvals 823,
  Adapters 888, Config 938, System 987) at y≈74; form inputs ≈ x=165.
- Decisions feature: `GET /api/runs/<id>/decisions` feeds a `decisions` block in run
  detail; `invoke_effect` rows show the operation + expandable payload JSON, other kinds
  render `key=value` pairs. The `environment` block comes from
  `GET /api/runs/<id>/environment` (profile env id, loop adapter, bindings per port —
  wasm adapter ids show the wasm-bound effect.execute).

## React SPA (rebuilt web UI, agentgw serves rust-embed from crates/agentgw/web/dist)

- Source is `crates/agentgw/web-app/` (React/Vite/Tailwind/shadcn). Left sidebar:
  Chat, Runs, Pipelines, Adapters, Config, Approvals, System. Health badge bottom-left.
- Standalone frontend-only testing needs no daemon: `cd agent-os/crates/agentgw/web-app
  && npm install && npm run dev` → http://localhost:5173 (fresh checkouts have no
  `node_modules`; node via nvm or, on this box, `export PATH=$HOME/node22/bin:$PATH`).
  Vite proxies `/api`→127.0.0.1:7740; with no daemon every page
  still renders but polls fail with repeating "Bad Gateway" toasts + red health dot —
  expected, NOT defects. Machine-local details below (`$HOME/node22`, vite port 5199,
  CDP 29229) are examples from the Devin test box — adapt to your own environment.
  Watch for stale vite servers/Chrome windows
  left by earlier sessions; CDP target list: `curl localhost:29229/json`.
- Coordinate math on this box: screenshots are real 1600x1200 px but input space is
  1024x768 (scale 1.5625). DOM `getBoundingClientRect` y is viewport-relative — add
  ~87px browser-chrome offset, then divide by 1.5625 for tool coords
  (`tool_y = (css_y + 87) / 1.5625`, `tool_x = css_x / 1.5625`). Verifying aria-labels
  against rects beats guessing icon positions.
- Forcing page overflow for scroll tests: `wmctrl -r "<window title>" -b
  remove,maximized_vert,maximized_horz` then `-e 0,0,0,1600,H` shrinks the window
  (real px); combine with Ctrl+= browser zoom for short pages whose content still
  fits. Note the icon rail itself does NOT scroll — below ~400 real px height the
  bottom rail buttons clip offscreen (pre-existing aside behavior, not a page-scroll
  bug); reset zoom with Ctrl+0 to reach lower rail icons again.
- The app is state-based with NO client router — server fallback serves index.html for
  any path (`/runs` → 200, no 404) but the SPA always boots to Chat; deep links are
  cosmetic only.
- `GET /api/index` returns BARE REFS — runs `{run_id,session_id,task_id}`, specs
  `{agent_spec_id,digest,version}`; hydrated rows come from `GET /api/runs`
  (state/state_name/epoch/step/output) added later. Specs carry display_name+profile
  in newer index entries — specs created before that build render as "agent".
- (Fixed) `ChatPage.send()` used to omit `requested_profile` → daemon retried
  `not_found: profile '' not defined` forever at `thinking…`/`created`. Now send()
  passes the spec's profile AND the gateway backfills `requested_profile` from the
  spec index when callers omit it. If a chat ever wedges at `created` again, check
  `agentd.log` for `profile ''` retries — that's this regression.
- `POST /api/config/proposals {document}` → `{proposal_id,path,valid,error}` — writes
  `<runtime>/proposed/*.yaml` even when `valid:false`. Specs are POST-only (PUT → 405).
- React Flow drag for pipeline edges: source handles are ~10px dots; tool presses off
  by ≥1px drag the node instead. Calibrate with a pointermove listener
  (`window.__hit`) — this session's mapping was `css_x = tool_x*1.5625 - 17`,
  `css_y = tool_y*1.5625 - 97` (differs per window size; verify before dragging).
  `left_mouse_down`/`left_mouse_up` take no coordinate — mouse_move to the handle
  first, verify `elementFromPoint`/`__hit` says `react-flow__handle`, press, move in
  ~100px steps to the target handle, release. Synthetic PointerEvent dispatch does NOT
  connect (edges=0); real drags only.
- System page metrics: map-valued metrics (`runs_by_state`, `effects_by_state`, …)
  render `state: count` pairs ("9: 4  10: 1"); an earlier build rendered literal
  `[object Object]` — if that regresses, flattenMetrics is not formatting maps.
- Custom providers (System → Add provider) persist in localStorage `agentos.providers`
  and the Chat dropdown shows the provider name after the label fix (it previously
  showed the raw UUID — if that regresses, the select is binding the id as label).
  The effect adapter enforces `base_url must be https on openrouter.ai` unless the
  daemon allows non-default base URLs — a custom-provider run failing with that
  reason is the payload flowing correctly, not a bug.
- Staged config proposals are listed by `GET /api/config/proposals` (proposal_id,
  path, valid, error) AND on Config → "Advanced (raw files)" tab's staged-proposals
  table (earlier builds had no proposals listing).
- Theme select sits in the sidebar footer; options open UPWARD in a scrollable list —
  items past ~4 visible are offscreen; scroll inside the list or click the visible
  rows. Choice persists via localStorage; collapsed label shows the theme name.
- Chat composer has an "MCP tools" switch (span[aria-checked] + hidden checkbox,
  default ON) — click the knob itself; the text label is not wired to toggle. When
  ON, the task envelope carries `tools:true` and the loop adapter's system prompt
  gains an `mcp_call` clause (grep the decision payload for "mcp_call" to confirm).
- `provider_unreachable` from openrouter-effect can be transient free-tier routing —
  retry before calling it a failure (key auth was 200 while the model call failed).
- Health badge text is `running · epoch N · outbox N`; `ok`/`healthy`/`running` map to
  badge-ok, anything else warns.
- Read the kernel sqlite read-only with python (`sqlite3` CLI may be absent):
  `python3 -c "import sqlite3; c=sqlite3.connect('file:<db>?mode=ro',uri=True); ..."`.

## Devin Secrets Needed

`secret:org:OPEN_ROUTER` → bind as env `OPENROUTER_API_KEY` when launching agentd with
`config/openrouter.yaml` + the `fixtures/openrouter-loop` bundle (free models only,
default `openrouter/free`). Not needed for the default fixture-loop config.
