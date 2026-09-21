# Agent OS Desktop

Native desktop shell (Tauri v2) for the Agent OS web gateway. The app
opens the running `agentgw` UI in a native window — the shell ships no
frontend of its own, so the same React app serves browser and desktop.

## Run

```sh
# 1. Start the stack (from agent-os/):
agentd --runtime-dir /tmp/run --config config/openrouter.yaml
agentgw --control-sock /tmp/run/control.sock --listen 127.0.0.1:7740

# 2. Launch the shell (from apps/desktop/):
npm install          # once — fetches the Tauri CLI
npm run dev          # dev window → http://127.0.0.1:7740

# Or point at another gateway:
AGENTOS_GATEWAY=http://my-host:7740 npm run dev
```

## Installable builds

```sh
npm run build        # tauri build → src-tauri/target/release/bundle/
```

Produces platform installers: `.dmg`/`.app` (macOS), `.msi`/`.exe`
(Windows), `.AppImage`/`.deb` (Linux). Linux builds need webkit2gtk
(`apt install libwebkit2gtk-4.1-dev libappindicator3-dev patchelf`).

The shell is intentionally thin — it loads the gateway URL, so a release
build still talks to a running daemon+gateway. Bundling `agentd` as a
sidecar binary is a follow-up: add it to `bundle.externalBin` and spawn
it from `lib.rs::run()` before opening the window.
