# Codex-parity gap analysis

What ai-harness already has vs. what a full-fledged AI harness like Codex
desktop ships. Checked against the current tree (`agentd` kernel, `agentgw` +
React SPA, adapters, fixtures).

## Already strong

- **Durable core**: SQLite kernel-store, transactional outbox, durable
  `EffectRecord`s with fencing and idempotent replay — stronger than Codex's
  ad-hoc session model.
- **Contract-first adapters**: framed protobuf protocol, bundle digests,
  profile-bound ports (`agent_loop`, `effect.execute`, `sandbox`, `workspace`,
  `artifact_store`) — Codex hardwires its tool loop.
- **Governance**: approvals API + UI, capability model, deterministic replay.
- **Surfaces**: control gRPC (agentctl), HTTP/SSE gateway, React SPA, Tauri
  shell.
- **LLM + tools**: OpenRouter loop/effect fixtures, MCP stdio+HTTP client ops,
  `computer` server (screenshot/click/type), ACP connectors.

## Missing or thin (gap → where it lives → suggested path)

| Area | Gap | Notes |
|------|-----|-------|
| **Self-extension** | No way for the agent to add tools/skills/plugins | **Shipped in this change**: `harness.*` ops + creative envelope + `spawn_agent` + skills index + live MCP registration. |
| **Multi-agent** | `spawn_agent` decision existed but deadlocked: children under a different task were invisible to `children_terminal`, and outcomes never reached the loop | **Fixed**: `RunRead::list_children` + `kernel.child_settled` feed + kernel-owned `task_id`/`parent_run_id` fill. |
| **File editing UX** | No diff/patch tool, no undo, no review surface | `harness.write` is whole-file; a `harness.patch` (unified diff) + diff preview in the GUI is the natural next op. |
| **Shell/exec tool** | No `exec`/`shell` MCP for the agent | Codex's core loop is shell-first. An `agent-shell` MCP server (confined cwd, timeout, output cap) closes most of this gap; scaffolded `mcp_server` + register already works. |
| **Persistent memory** | No cross-session memory/notes | `AGENTOS_HARNESS_ROOT` is already a persistent scratch space; a `memory/` convention + prompt injection would formalize it. |
| **Skills/plugins registry** | Marketplace-style discovery | `skills/` + `registry.json` are local-only; no remote index, no versioned install/update flow. |
| **Code intelligence** | No grep/AST search op for the model | `harness.read` is single-file; `harness.search {pattern, glob?}` (rg-backed) is cheap to add on the same surface. |
| **Worktree isolation** | Parallel workspaces exist, but child runs don't get isolated git worktrees | `workspace` port + `spawn_agent` could mint a worktree per child; Codex does cloud-task isolation. |
| **Scheduling/daemon tasks** | No "run this agent on a cron/webhook" | The scheduler crate fires `Wait` timers only; a `schedule.run` command kind would unlock automation. |
| **Auth/notifications** | No user accounts, SSO, push, or Slack surface | agentgw is single-user local; Codex cloud tasks need an identity/notification layer. |
| **Usage/metering** | No token/cost accounting per run | Effect records could carry `usage` from providers; surfaces exist (System page) to display it. |
| **Replay UI** | Deterministic replay exists internally but isn't exposed | A "replay this run" GUI action + event-sourced debugging view. |
| **Model routing** | One bound loop per profile | Model-pick-per-message exists via `model` envelope; a routing policy (cheap→strong escalation) is a loop concern. |
| **Sandboxed code exec** | `sandbox` port exists; no shipped sandbox adapter | Docker/gVisor/firecracker adapter bundle is the big remaining parity item. |
| **Checkpointing UX** | Snapshots by crash recovery only | Named run checkpoints + "restore" in the GUI. |

## Priority for parity

1. **exec/shell MCP tool** — biggest functional gap vs. Codex.
2. **Sandbox adapter** — safe untrusted code execution.
3. **patch/diff tool + review UX** — code-edit quality of life.
4. **Search op** — lets the agent navigate the root without enumerating.
5. **Scheduling** — unattended automation.
6. **Memory conventions** — cross-session continuity.

Creative mode (this change) covers the *extensibility* row end-to-end: tools
(`mcp_server`), skills (`skills/`), workflows (`workflows/`), and plugin
skeletons (`adapter`/`loop`) are all scaffold→validate→register able from
inside a run.
