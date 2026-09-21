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
find . -type f ! -name bundle.lock ! -name adapter.manifest.json -printf '%P\n' \
  | LC_ALL=C sort \
  | while IFS= read -r f; do
      printf 'sha256:%s %s\n' "$(sha256sum "$f" | cut -d' ' -f1)" "$f"
    done >> bundle.lock
