#!/usr/bin/env bash
# Verifies that the implementation contract mirror (agent-os/proto) is
# byte-identical to the build pack snapshot
# (agent-os-microkernel-mvp-buildpack/contracts) and that the mirror's
# contract lock still matches every mirrored file.
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
agent_os_dir=$(dirname "$script_dir")
repo_root=$(dirname "$agent_os_dir")

pack_contracts="$repo_root/agent-os-microkernel-mvp-buildpack/contracts"
proto_dir="$agent_os_dir/proto"

failed=0

if ! diff -rq "$pack_contracts" "$proto_dir"; then
  echo "contract mirror check: snapshot and implementation proto differ" >&2
  failed=1
fi

if command -v shasum >/dev/null 2>&1; then
  (cd "$proto_dir" && shasum -a 256 -c contract-lock.sha256) || failed=1
elif command -v sha256sum >/dev/null 2>&1; then
  (cd "$proto_dir" && sha256sum -c contract-lock.sha256) || failed=1
else
  echo "contract mirror check: neither shasum nor sha256sum is available" >&2
  failed=1
fi

if [ "$failed" -ne 0 ]; then
  echo "contract mirror check FAILED" >&2
  exit 1
fi

echo "contract mirror check OK"
