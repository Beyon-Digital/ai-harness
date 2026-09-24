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

## Environment

Every variable the services read has a built-in default — nothing needs
to be set for the app to work. Two ways to customize:

- `agentos.env` in the app data dir — auto-created on first launch with
  a commented template (`OPENROUTER_API_KEY`, `OPENROUTER_MODEL`,
  `AGENTOS_GATEWAY`, `AGENTOS_CONFIG`, `MCP_SERVERS`, `ACP_*`, …). Edit
  it in any text editor and relaunch; no terminal needed.
- Real environment variables — always win over the file.

Key vars: `AGENTOS_GATEWAY=https://host:port` skips the embedded
services entirely (remote-gateway mode); `AGENTOS_CONFIG=<name>.yaml`
picks a bundled config from `share/`; `OPENROUTER_API_KEY` set → the
app auto-picks `openrouter.yaml` for the real-LLM path. The bundled
macOS/Linux runtime also registers a `computer` MCP server with
screenshot, click, type, key, and wait tools. Set `MCP_SERVERS`
yourself to replace that automatic server map.

```sh
AGENTOS_GATEWAY=https://my-host:7740 ./Agent\ OS.app/...  # remote gateway, no local services
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
  -p fixture-effect-adapter -p computer-mcp \
  && cargo build --release --target wasm32-wasip1 -p wasm-echo)
./scripts/stage-embedded-runtime.sh "$triple"
npm ci && npm run build:embedded

# Windows (no embedded runtime — the daemon speaks Unix sockets):
npm ci && npm run build
```

Produces platform installers: `.dmg`/`.app` (macOS), `.msi`/`.exe`
(Windows), and `.deb` (Linux). The Debian package declares the
`xdotool` and ImageMagick runtime dependencies used by computer tools.
Linux builds need webkit2gtk (`apt install libwebkit2gtk-4.1-dev
libappindicator3-dev patchelf xdotool imagemagick`).

## Releases

`.github/workflows/release.yml` publishes GitHub Releases automatically:

- **Merges to `main`** that touch the app, runtime, docs, or this
  workflow (see the workflow's path filter) rebuild the matrix and
  refresh the rolling **`dev-latest`** prerelease —
  `releases/tag/dev-latest` always carries the newest build. Asset names
  carry a `-dev.<sha>.<run>.<attempt>` suffix so a refresh never mutates
  the live set: the new files upload alongside the old, the tag moves
  once the full set is in place, and obsolete assets are pruned last.
- **`v*` tags publish a full release** directly (tag names containing
  `-`, e.g. `v1.0.0-rc1`, land as prereleases).
- **Manual `workflow_dispatch`** with `publish: true` does the same on
  demand.

Assets on every release:

- Desktop: `.dmg` (macOS arm64 + Intel), `.msi`/`.nsis` (Windows),
  `.deb` (Linux x86_64).
- CLI archives (`agentos-<name>-<platform>.tar.gz`): `agentd`,
  `agentctl`, `agentgw`, `agentos-wasm-host`, the adapter binaries,
  `wasm-echo.wasm`, `make-adapter-bundle.sh`, config manifests, and the
  gateway guide. Unix-only — the daemon speaks Unix sockets; on Windows
  the desktop app points at a remote `agentgw` via `AGENTOS_GATEWAY`.
  Linux CLI archive users must install `xdotool` and ImageMagick before
  using `computer-mcp`; the desktop `.deb` installs them automatically.

Cut a stable release:

```sh
git tag v0.1.0 && git push origin v0.1.0
# → release workflow → published release with all artifacts attached
```

PRs are covered by the `agent-os` workflow; the release matrix runs only
on merges to `main`, `v*` tags, and manual `workflow_dispatch`. To
validate the packaging path on demand, dispatch the workflow without
`publish`.

### Release signing

Stable `v*` tags read these GitHub Actions secrets:

- macOS (required — the release fails without them): `APPLE_CERTIFICATE`
  (base64 `.p12`), `APPLE_CERTIFICATE_PASSWORD`,
  `APPLE_SIGNING_IDENTITY`, `APPLE_API_KEY` (App Store Connect key ID),
  `APPLE_API_ISSUER`, `APPLE_API_PRIVATE_KEY` (base64 `AuthKey_<id>.p8`),
  and `KEYCHAIN_PASSWORD`.
- Windows (optional): `WINDOWS_CERTIFICATE` (base64 `.pfx`) and
  `WINDOWS_CERTIFICATE_PASSWORD`. When absent the job warns and the
  release ships unsigned Windows installers.

The workflow imports the platform certificates only for stable tags,
uses Tauri to sign macOS bundles and notarize them via the App Store
Connect API key, and Authenticode-signs Windows installers when the
certificate secrets are configured.
PR and rolling `dev-latest` builds remain certificate-free. Every
published installer and CLI archive is also signed keylessly with
Sigstore using GitHub OIDC; the matching `.sigstore.json` bundle is
attached beside the artifact.
