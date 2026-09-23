# Agent OS Desktop

Native desktop app (Tauri v2) for the Agent OS web gateway. On macOS and
Linux the app is self-contained: it bundles `agentd`, `agentgw`, the
adapter bundles and configs, spawns them on launch, and opens the
gateway UI in a native window once it answers — no terminal needed.

## Run

Just launch the app. It stages the embedded runtime into the app data
dir, starts `agentd` + `agentgw` (port 7740, or a free port if taken),
and loads the dashboard when it's up. Service logs live under the app
data dir in `logs/`; runtime state (kernel.db, events.db) in `run/`.

Overrides for power users:

```sh
AGENTOS_GATEWAY=http://my-host:7740 ./Agent\ OS.app/...   # remote gateway, no local services
AGENTOS_CONFIG=openrouter.yaml                            # pick a bundled config
OPENROUTER_API_KEY=sk-or-...                              # auto-picks openrouter.yaml
```

```sh
npm install          # once — fetches the Tauri CLI
npm run dev          # dev window → http://127.0.0.1:7740 (external mode)
```

## Installable builds

```sh
# Unix (macOS/Linux): self-contained app — build agent-os release bins
# first, stage them, then bundle:
triple=$(rustc -vV | awk '/^host:/{print $2}')
(cd ../../agent-os && cargo build --release --target "$triple" \
  -p agentd -p agentgw -p wasm-host -p local-memory -p openrouter-loop \
  -p openrouter-effect -p acp-loop -p fixture-agent-loop \
  -p fixture-effect-adapter \
  && cargo build --release --target wasm32-wasip1 -p wasm-echo)
./scripts/stage-embedded-runtime.sh "$triple"
npm ci && npm run build:embedded

# Windows (no embedded runtime — the daemon speaks Unix sockets):
npm ci && npm run build
```

Produces platform installers: `.dmg`/`.app` (macOS), `.msi`/`.exe`
(Windows), `.AppImage`/`.deb` (Linux). Linux builds need webkit2gtk
(`apt install libwebkit2gtk-4.1-dev libappindicator3-dev patchelf`).

## Releases

`.github/workflows/release.yml` builds installers and CLI archives on
every `v*` tag push and publishes them to a draft GitHub Release:

- Desktop: `.dmg` (macOS arm64 + Intel), `.msi`/`.nsis` (Windows),
  `.AppImage`/`.deb` (Linux x86_64).
- CLI archives (`agentos-<tag>-<platform>.tar.gz`): `agentd`,
  `agentctl`, `agentgw`, `agentos-wasm-host`, the adapter binaries,
  `wasm-echo.wasm`, `make-adapter-bundle.sh`, config manifests, and the
  gateway guide. Unix-only — the daemon speaks Unix sockets; on Windows
  the desktop app points at a remote `agentgw` via `AGENTOS_GATEWAY`.

Cut a release:

```sh
git tag v0.1.0 && git push origin v0.1.0
# → release workflow → draft release with all artifacts attached
```

PRs touching `apps/desktop/**` and manual `workflow_dispatch` runs
execute the same matrix build-only (artifacts on the run, nothing
published), so the packaging path is always exercised before a tag.
