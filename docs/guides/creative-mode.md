# Creative mode

Creative mode lets an agent **upgrade its own harness**: inspect and write files
under a confined harness root, scaffold new extensions (skills, workflows,
adapters, loops, MCP servers), validate them, and register them — plus spawn
child agent runs to parallelize work. Everything is delivered through the
existing effect plumbing, so every action stays durable, fenced, and
idempotent.

## Turning it on

Creative mode is a task-envelope flag — no profile changes needed. In the web
GUI, toggle **Creative** in the chat composer. Via the API, submit a run whose
task payload is:

```json
{
  "task": "Build me a JSON-echo MCP tool and use it",
  "creative": true,
  "tools": true,
  "spawn_defaults": {
    "session_id": "<session id>",
    "agent_spec_id": "<spec id>",
    "spec_version": "<spec version>",
    "spec_digest": "<spec digest>",
    "profile": "openrouter-wasm"
  },
  "max_children": 8
}
```

- `creative: true` — unlocks `harness_call` and `spawn_agent` replies.
- `spawn_defaults` — identity the kernel needs to create children. The chat UI
  fills it automatically; omit it and any `spawn_agent` fails with
  `spawn_not_configured`.
- `max_children` — optional child cap (default 8), enforced via the kernel
  event marker's `child_count`.

## The harness root

`harness.*` ops act on a single confined directory — the harness root —
resolved as `AGENTOS_HARNESS_ROOT` on the effect adapter, else
`~/.agentos/harness`. Every path is root-relative; absolute paths, `..`
segments, and symlink escapes are rejected. Layout created on demand:

```
<root>/
├── skills/<name>/SKILL.md      # model-visible skills
├── workflows/<name>.json       # ordered step plans
├── extensions/<name>/          # adapters, loops, MCP servers
├── mcp-servers.json            # live MCP server map (merged into MCP_SERVERS)
└── registry.json               # extension registry
```

## Model replies

On top of the standard four (`complete`, `fail`, `wait`, `request_approval`):

```json
{"harness_call": {"op": "harness.read", "path": "skills/echo/SKILL.md"}}
{"spawn_agent": {"task": "research X", "envelope": {"tools": true}, "profile": "local-trusted"}}
```

`harness_call` dispatches a `harness.*` op through the bound `effect.execute`
adapter exactly like `model.chat`/`mcp.call_tool` — the settled result is fed
back next turn. `spawn_agent` submits a `CreateTaskRun`; the child inherits the
parent's `creative`/`tools`/model/`spawn_defaults` unless its own `envelope`
overrides them, and the parent resumes automatically when every child is
terminal with outcomes fed as `kernel.child_settled` entries.

## Harness ops

| op | payload | result |
|----|---------|--------|
| `harness.catalog` | `{path?}` | file tree (≤6 deep, ≤2000 entries) |
| `harness.list` | `{path?}` | one directory listing |
| `harness.read` | `{path, max_bytes?}` | file contents (≤256KB) |
| `harness.write` | `{path, content, create_only?}` | atomic write (≤1MiB) |
| `harness.scaffold` | `{kind, name, description?}` | ready-made extension files |
| `harness.validate` | `{path}` | manifest/SKILL/workflow checks |
| `harness.register` | `{kind, name}` | install into the registry |
| `harness.registry` | `{}` | registered extensions |

`kind` is `skill` | `workflow` | `adapter` | `loop` | `mcp_server`.
Scaffolded `mcp_server` + `harness.register` go **live immediately**: the
server lands in `mcp-servers.json`, which `mcp.*` ops re-read per call — the
agent can then invoke it via `mcp.call_tool` in the same run.
`adapter`/`loop` register as `needs_build`: they still require compiling the
crate and binding it to a profile (an operator step outside the run).

## Skills and workflows

`skills/<name>/SKILL.md` files with `name:`/`description:` frontmatter are
indexed into the creative system prompt — the model sees what skills exist and
can read the full body with `harness.read`. `workflows/<name>.json` documents
ordered steps (`{name, description, steps: [{task, tools?}]}`) the agent can
run sequentially or fan out as children.

## Budgets and guards

- `MAX_HARNESS_HOPS = 16` `harness.*` ops per run (then the model must finish).
- `mcp.*` cap stays 4; `child_count` caps children (default 8).
- Duplicate request hashes (`latest_tool_request_hash` kernel marker) reject
  as `repeated_harness_call`; a child payload identical to an already-fed
  child's rejects as `duplicate_spawn`.

## Env vars (effect adapter)

- `AGENTOS_HARNESS_ROOT` — harness root (default `~/.agentos/harness`)
- `MCP_SERVERS_FILE` — override the merged server file (default
  `<root>/mcp-servers.json`); `MCP_SERVERS` env entries always win on
  collision.
