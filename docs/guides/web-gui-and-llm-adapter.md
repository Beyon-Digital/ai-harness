> **Navigation:** [Home](../index.md) · [Guides](README.md)

# Web GUI + real LLM loop (post-MVP bring-up)

The MVP control surface (`agentctl` + UDS gRPC) is CLI-only. For browser
use, `agentgw` bridges the daemon's Unix-socket APIs to a localhost
JSON REST + SSE surface and serves an embedded dashboard.

## Pieces

- **`agentd`** — the kernel daemon (unchanged).
- **`agentgw`** — axum HTTP gateway: REST mirror of the control API, an
  SSE `Subscribe` bridge, and a small JSON index (`<socket>.agentgw-
  index.json`) of sessions/tasks/runs/specs created through it (the
  frozen MVP contract has no list RPCs, so the gateway remembers ids).
- **`fixtures/openrouter-loop`** — a real-inference `agent_loop@1`
  adapter: each turn posts the task payload (`LoopInput.state`) to
  OpenRouter chat completions and maps the model's JSON reply onto
  `complete` / `fail` / `wait` / `request_approval` decisions.
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
  target/debug/fixture-effect-adapter \
  fixtures/effect-adapter/adapter.manifest.json \
  bundles/fixture-effect

OPENROUTER_API_KEY=sk-or-... \
  agentd --runtime-dir run \
         --config config/openrouter.yaml \
         --adapter-bundle bundles/openrouter-loop \
         --adapter-bundle bundles/fixture-effect

agentgw --socket run/control.sock --listen 127.0.0.1:7740
# open http://127.0.0.1:7740/
```

`config/openrouter.yaml` binds `profiles.local-trusted.agent_loop` to
the OpenRouter adapter (`01905c5e-0000-7000-8000-11a7c3d90001@0.1.0`).
`config/default.yaml` keeps the deterministic `fixture-loop` for tests.

The daemon propagates `OPENROUTER_API_KEY`, `OPENROUTER_MODEL`
(default `openrouter/free`), `OPENROUTER_BASE_URL`, `OPENROUTER_SITE`
and `OPENROUTER_APP_NAME` into loop-adapter children only — nothing
else crosses the env boundary.

## Run lifecycle in the GUI

1. Sessions tab: create a session (id optional — uuidv7 minted), put an
   agent-spec revision whose body names the profile:
   `{"runtime_profile_name":"local-trusted"}`.
2. Runs tab: create a run — pick the submitted spec from the dropdown
   (fills id/version/digest), keep `local-trusted` profile, write the
   task prompt.
3. Watch the event stream; click a completed run to read the model's
   answer (returned as a `data:text/plain;base64,` `output_ref`, decoded
   inline).
4. If the model answers `{"request_approval": ...}`, the run parks in
   `waiting_human` and the request appears under Approvals —
   approve/deny resumes or fails the run.

## Model decision vocabulary

The loop adapter accepts exactly one JSON object per turn:

```json
{"complete": {"output": "<final answer>"}}
{"fail": {"reason_code": "<snake_case>"}}
{"wait": {"reason": "<what it waits on>"}}
{"request_approval": {"operation": "<op>", "reason": "<why>"}}
```

Prose replies are treated as a `complete` whose output is the raw text.
