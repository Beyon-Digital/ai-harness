#!/usr/bin/env bash
# Mutation test for scripts/validate_buildpack.py.
#
# Copies the build pack to a temporary directory, applies exactly one mutation
# per defect class, and requires the validator to reject every mutated copy
# with an error naming that class. The validator's established failure exit
# code is 1 (preserved); the script treats any non-zero exit as caught and
# additionally checks the expected error signature is present.
#
# Exits 0 only when every mutation is caught.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/buildpack-mutations.XXXXXX")"
trap 'rm -rf "${TMP}"' EXIT

BASELINE_LOG="${TMP}/baseline.log"
if ! python3 "${ROOT}/scripts/validate_buildpack.py" >"${BASELINE_LOG}" 2>&1; then
  echo "baseline: validator must pass before mutations can be attributed" >&2
  cat "${BASELINE_LOG}" >&2
  exit 1
fi

CASE_NAME=""
CASE_DIR=""
TOTAL=0
CAUGHT=0

prepare_case() {
  CASE_NAME="$1"
  CASE_DIR="${TMP}/${CASE_NAME}"
  rm -rf "${CASE_DIR}"
  cp -R "${ROOT}" "${CASE_DIR}"
  find "${CASE_DIR}" -type d -name __pycache__ -exec rm -rf {} + 2>/dev/null || true
}

assert_caught() {
  local signature="$1"
  local log="${TMP}/${CASE_NAME}.log"
  local code
  TOTAL=$((TOTAL + 1))
  python3 "${CASE_DIR}/scripts/validate_buildpack.py" >"${log}" 2>&1
  code=$?
  if [ "${code}" -ne 0 ] && grep -qF -- "${signature}" "${log}"; then
    echo "CAUGHT ${CASE_NAME} (exit ${code})"
    CAUGHT=$((CAUGHT + 1))
  else
    echo "MISSED ${CASE_NAME} (exit ${code}, expected error containing: ${signature})"
    sed 's/^/    /' "${log}"
  fi
}

# 1. schema coverage: drop a required table name
prepare_case drop_table
python3 - "${CASE_DIR}" <<'PY'
import re, sys
from pathlib import Path
p=Path(sys.argv[1])/'specs/kernel-store-schema.sql'
text=p.read_text()
text,count=re.subn(r'(?i)CREATE TABLE(?: IF NOT EXISTS)?\s+artifacts\b',
                   'CREATE TABLE IF NOT EXISTS artifacts_dropped', text, count=1)
assert count==1, 'artifacts table not found'
p.write_text(text)
PY
assert_caught "missing SQL table artifacts"

# 2. limits: delete a required key
prepare_case drop_limits_key
python3 - "${CASE_DIR}" <<'PY'
import sys
from pathlib import Path
p=Path(sys.argv[1])/'specs/limits.yaml'
lines=[line for line in p.read_text().splitlines(True) if 'capacity_messages:' not in line]
p.write_text(''.join(lines))
PY
assert_caught "missing limits key queue.capacity_messages"

# 3. DAG sources: break one edge in the generated mermaid graph
prepare_case break_dag_edge
python3 - "${CASE_DIR}" <<'PY'
import sys
from pathlib import Path
p=Path(sys.argv[1])/'DAG.md'
text=p.read_text()
mutated=text.replace('FND_001 --> FND_002','FND_001 --> FND_003',1)
assert mutated!=text, 'edge not found'
p.write_text(mutated)
PY
assert_caught "DAG.md"

# 4. manifest: corrupt one recorded hash
prepare_case corrupt_manifest_hash
python3 - "${CASE_DIR}" <<'PY'
import json, sys
from pathlib import Path
p=Path(sys.argv[1])/'MANIFEST.json'
manifest=json.loads(p.read_text())
manifest['files'][0]['sha256']='0'*64
p.write_text(json.dumps(manifest, indent=2)+'\n')
PY
assert_caught "MANIFEST.json digest mismatch for"

# 5. protobuf: duplicate a message across files of the same package
prepare_case duplicate_proto_message
python3 - "${CASE_DIR}" <<'PY'
import sys
from pathlib import Path
p=Path(sys.argv[1])/'contracts/control-api/commands.proto'
p.write_text(p.read_text()+'\nmessage GetRunRequest { string duplicate = 99; }\n')
PY
assert_caught "duplicate proto message GetRunRequest"

# 6. ownership: add an unbounded glob to a task file list
prepare_case add_unbounded_glob
python3 - "${CASE_DIR}" <<'PY'
import json, sys, yaml
from pathlib import Path
root=Path(sys.argv[1])
for name in ('dag.yaml','dag.json'):
    p=root/name
    graph=yaml.safe_load(p.read_text()) if name.endswith('.yaml') else json.loads(p.read_text())
    for task in graph['tasks']:
        if task['id']=='FND-001':
            task['files'].append('crates/agentd/src/*.rs')
    if name.endswith('.yaml'):
        p.write_text(yaml.safe_dump(graph, sort_keys=False))
    else:
        p.write_text(json.dumps(graph, indent=2, ensure_ascii=False)+'\n')
PY
assert_caught "unbounded glob"

echo "mutations: ${CAUGHT}/${TOTAL} caught"
[ "${TOTAL}" -gt 0 ] && [ "${CAUGHT}" -eq "${TOTAL}" ]
