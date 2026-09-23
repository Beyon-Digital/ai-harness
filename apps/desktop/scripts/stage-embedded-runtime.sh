#!/usr/bin/env bash
# Stage the embedded runtime the desktop app spawns at launch:
#   src-tauri/binaries/{agentd,agentgw}-<triple>[.exe]  (externalBin sidecars)
#   src-tauri/resources/agentos/share/**               (bundle resources:
#                                                       configs, manifests,
#                                                       adapter bundles)
# Expects the agent-os release binaries + wasm plugin already built for
# <triple> (see release.yml desktop job). No-op for Windows targets —
# the CLI stack is Unix-only there and the app runs in remote-gateway
# mode (AGENTOS_GATEWAY).
set -euo pipefail

triple="${1:?usage: stage-embedded-runtime.sh <target-triple>}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
tauri_dir="$repo_root/apps/desktop/src-tauri"
aos="$repo_root/agent-os"

if [[ "$triple" == *windows* ]]; then
  echo "windows target: no embedded runtime (Unix sockets only); app uses AGENTOS_GATEWAY"
  mkdir -p "$tauri_dir/resources/agentos/share"
  exit 0
fi

ext=""
[[ "$triple" == *windows* ]] && ext=".exe"

mkdir -p "$tauri_dir/binaries"
for b in agentd agentgw; do
  src="$aos/target/$triple/release/$b$ext"
  [[ -f "$src" ]] || src="$aos/target/release/$b$ext"   # native build dir
  [[ -f "$src" ]] || { echo "missing $src"; exit 1; }
  cp "$src" "$tauri_dir/binaries/$b-$triple$ext"
  chmod +x "$tauri_dir/binaries/$b-$triple$ext"
done

share="$tauri_dir/resources/agentos/share"
mkdir -p "$share/manifests" "$share/bundles"
cp "$aos"/config/*.yaml "$share/"
for m in "$aos"/fixtures/*/adapter.manifest.json; do
  name=$(basename "$(dirname "$m")")
  cp "$m" "$share/manifests/$name.manifest.json"
  entrypoint=$(sed -n 's/.*"entrypoint" *: *"\([^"]*\)".*/\1/p' "$m" | head -1)
  src="$aos/target/$triple/release/$entrypoint$ext"
  [[ -f "$src" ]] || src="$aos/target/release/$entrypoint$ext"
  [[ -f "$src" ]] || src="$aos/target/wasm32-wasip1/release/$entrypoint"
  [[ -f "$src" ]] || { echo "missing entrypoint for $name: $entrypoint"; exit 1; }
  "$aos/scripts/make-adapter-bundle.sh" "$src" "$m" "$share/bundles/$name"
done
cp "$repo_root/docs/guides/web-gui-and-llm-adapter.md" "$share/"

echo "staged: $(find "$tauri_dir/resources/agentos" -type f | wc -l) resource files, $(ls "$tauri_dir/binaries")"
