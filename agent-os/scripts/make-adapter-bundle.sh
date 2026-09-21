#!/usr/bin/env bash
# Assemble a process adapter bundle directory:
#   make-adapter-bundle.sh <adapter-binary> <adapter.manifest.json> <out-dir>
#
# Copies the manifest + binary into <out-dir> and writes bundle.lock
# (sha256:<digest> <relpath> per non-manifest, non-lock file) in exactly
# the format adapter_registry::write_lock produces.
set -euo pipefail

bin="$1"
manifest="$2"
out="$3"

mkdir -p "$out"
cp "$manifest" "$out/adapter.manifest.json"
cp "$bin" "$out/$(basename "$bin")"
chmod +x "$out/$(basename "$bin")"

cd "$out"
: > bundle.lock
# Portable across GNU/macOS: no `find -printf`, no `sha256sum` on BSD.
if command -v sha256sum >/dev/null 2>&1; then
  digest() { sha256sum "$1" | cut -d' ' -f1; }
else
  digest() { shasum -a 256 "$1" | cut -d' ' -f1; }
fi
find . -type f ! -name bundle.lock ! -name adapter.manifest.json \
  | LC_ALL=C sort \
  | while IFS= read -r f; do
      f="${f#./}"
      printf 'sha256:%s %s\n' "$(digest "$f")" "$f"
    done >> bundle.lock
