#!/usr/bin/env bash
# REL-001 — Agent OS microkernel MVP release gate.
#
# One command that proves the MVP satisfies MVP_EXIT_CRITERIA.md:
#   1. fmt / clippy(-D warnings) / build / workspace tests
#   2. contract mirror + hash lock + schema/buildpack/repository validators
#   3. unit, store, concurrency, security, property, and process-level e2e suites
#   4. BLOCKING_MVP marker sweep
#   5. machine-readable JSON + Markdown verification report with the git SHA,
#      toolchain, and platform, linking every exit criterion to the evidence
#      that just passed.
#
# Usage: scripts/release_gate.sh [--fast]
#   --fast   skip `cargo build` (test compilation already proves it) and run
#            the property suite at PROPTEST_CASES=64 (default gate run uses
#            the suites' checked-in defaults).
#
# Exit code is non-zero when any check fails. Reports land in
# artifacts/release-gate/ as verification-report.{json,md}.
set -u -o pipefail

repo_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
agent_os="$repo_root/agent-os"
out_dir="$repo_root/artifacts/release-gate"
mkdir -p "$out_dir"

FAST=0
for arg in "$@"; do
  case "$arg" in
    --fast) FAST=1 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

cd "$repo_root"

GIT_SHA=$(git rev-parse HEAD 2>/dev/null || echo "unknown")
GIT_DIRTY=$(git status --porcelain --untracked-files=no | wc -l | tr -d ' ')
TOOLCHAIN=$(cd "$agent_os" && rustc -V 2>/dev/null || echo "unknown")
PLATFORM="$(uname -s) $(uname -m)"
STARTED_AT=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

JSON_CHECKS="$out_dir/.checks.jsonl"
: > "$JSON_CHECKS"

check() { # name, command...
  local name="$1"; shift
  local started ended status detail log
  started=$(date +%s)
  log="$out_dir/.last-${name}.log"
  echo "=== GATE $name: $*"
  if "$@" >"$log" 2>&1; then
    status=pass
  else
    status=fail
  fi
  ended=$(date +%s)
  detail=$(tail -n 3 "$log" | tr '\n' ' ' | tr -d '"' | sed 's/\\/\\\\/g' | cut -c1-240)
  printf '{"name":"%s","status":"%s","seconds":%d,"detail":"%s"}\n' \
    "$name" "$status" "$((ended - started))" "$detail" >> "$JSON_CHECKS"
  if [ "$status" = fail ]; then
    echo "--- $name FAILED (log: $log)"; tail -n 15 "$log" >&2
    GATE_FAIL=1
  else
    echo "--- $name ok"
  fi
}

GATE_FAIL=0

# -- 1/2. Validators + contract hash/schema ----------------------------------
check buildpack_validator \
  python3 "$repo_root/agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py"
check repo_invariants \
  python3 "$repo_root/tools/validate_repo.py"
check contract_mirror_lock \
  bash "$agent_os/scripts/check-contract-mirror.sh"

# -- Rust quality gates -------------------------------------------------------
cd "$agent_os"
check cargo_fmt cargo fmt --check
check cargo_clippy cargo clippy --workspace --all-targets -- -D warnings
if [ "$FAST" -eq 0 ]; then
  check cargo_build cargo build --workspace --all-targets
fi

# -- 3. Test suites ------------------------------------------------------------
check workspace_tests cargo test --workspace
check concurrency_suite cargo test -p agentd --test concurrency
check security_suite cargo test -p agentd --test security
check property_suite env "PROPTEST_CASES=${PROPTEST_CASES:-512}" \
  cargo test -p agentd --test property
check e2e_suites cargo test -p agentd \
  --test e2e_basic --test e2e_children_workspace \
  --test e2e_effect_recovery --test lock_exclusion

# -- 4. Hygiene ---------------------------------------------------------------
check blocking_mvp_markers bash -c \
  '! grep -Rn "BLOCKING_MVP" "$0" --include="*.rs" --include="*.toml" --include="*.yaml" --include="*.md" -r .' \
  "$agent_os"
check test_sleep_hygiene bash -c '
  violations=$(grep -RIlE "thread::sleep|tokio::time::sleep" crates --include="*.rs" | while read -r file; do
    if [[ "$file" == */tests/* ]] || grep -q "#\[cfg(test)\]" "$file"; then
      echo "$file"
    fi
  done)
  if [ -n "$violations" ]; then
    echo "N2 violation: wall-clock sleep in test code:" >&2
    echo "$violations" >&2
    exit 1
  fi'
cd "$repo_root"

# -- 5. Reports ---------------------------------------------------------------
# Criterion-to-evidence mapping (MVP_EXIT_CRITERIA.md 1..24). Evidence names
# are test/validator identifiers exercised by the checks above — none of them
# is asserted as passing unless its gate ran and passed in this invocation.
read -r -d '' CRITERIA <<'EOF' || true
1	single writer authority|kernel-store-sqlite::tests/fence.rs (first_claim_creates_epoch_one_with_a_live_lease, successive_claims_strictly_increase_the_epoch, restart_keeps_the_fence_and_increments_the_next_claim, concurrent_claims_serialize_into_distinct_epochs) + agentd --test lock_exclusion (second_process_is_refused_and_kill_releases_the_lock)
2	idempotent command replay|kernel-store-sqlite::tests/idempotency.rs + command-coordinator replay tests (workspace_tests gate)
3	txn/outbox atomicity|kernel-store-sqlite::tests/outbox.rs (rollback_discards_outbox_rows_and_stream_allocations, outbox_rows_reject_mutation_outside_publication_metadata)
4	publisher re-run safe|events::dispatcher tests + kernel-store-sqlite::tests/outbox_concurrent.rs (workspace_tests gate)
5	no premature cursor|event-journal-sqlite::tests/journal.rs + events::live_bus tests (workspace_tests gate)
6	no dependency cycles|agentd --test concurrency::raced_graph_edge_insertions_never_cycle + agentd --test property::dependency_graph_never_contains_a_cycle
7	single claim winner|agentd --test concurrency::raced_ready_run_claims_single_winner + kernel-store-sqlite::tests/contention.rs::exactly_one_writer_wins_a_raced_cas
8	cancel/spawn race|agentd --test e2e_children_workspace::e2e_cancel_spawn_race
9	stale decision rejected|agentd --test property::run_transitions_legal_and_revision_monotonic (stale-revision CAS) + runtime loop_turn tests (workspace_tests)
10	single effect fencer|agentd --test concurrency::raced_effect_claim_single_fencer + raced_effect_commit_single_fencer + property::committed_effects_never_regress
11	crash -> reconcile or Unknown|agentd --test e2e_effect_recovery::e2e_crash_before_ack_reconciles_exactly_once + e2e_unreconcilable_effect_blocks_until_resolved
12	timer fire/cancel single winner|agentd --test concurrency::raced_timer_fire_cancel_single_winner + property::timer_version_monotonic_terminal_stable
13	descendants <= ancestor budget|agentd --test concurrency::raced_reservation_children_never_exceed_parent_budget + property::reservation_descendants_within_parent_budget
14	no capability amplification|agentd --test security::confused_deputy_cannot_widen_past_parent + confused_deputy_tool_restriction_denies
15	approval digest mismatch rejected|agentd --test security::approval_digest_binding_rejects_mutated_request
16	T2 cannot fall back to T0|agentd --test security::t2_requirement_never_served_by_untrusted_adapter
17	exclusive lease single writer|agentd --test concurrency::raced_exclusive_transfer_single_winner + property::never_two_active_exclusive_leases + e2e_children_workspace::e2e_exclusive_transfer_stale_lease_rejected
18	fork/merge|agentd --test e2e_children_workspace::e2e_parallel_fork_merge
19	frozen per-run bindings|config-engine::tests/config.rs::service_binding_change_requires_restart + reactivation_restores_known_good (workspace_tests)
20	frozen global binding change rejected|config-engine::tests/config.rs::activation_cas_one_winner + service_binding_change_requires_restart (workspace_tests)
21	handshake identity checks|agentd --test security::forged_adapter_hello_fields_rejected + inbound_frame_order_enforced
22	drain-first shutdown|agentd --test e2e_basic::e2e_idle_shutdown + api/daemon drain tests (workspace_tests)
23	end-to-end fixture flow|agentd --test e2e_basic::e2e_complete_run_via_daemon_api + e2e_effect_recovery::e2e_effect_executes_normally
24	all quality gates|this script: cargo_fmt, cargo_clippy, workspace_tests, concurrency_suite, security_suite, property_suite, e2e_suites
EOF

STATUS=pass
[ "$GATE_FAIL" -ne 0 ] && STATUS=fail

CRITERIA_TSV="$out_dir/.criteria.tsv"
printf '%s\n' "$CRITERIA" > "$CRITERIA_TSV"

python3 - "$GIT_SHA" "$GIT_DIRTY" "$TOOLCHAIN" "$PLATFORM" "$STARTED_AT" \
  "$STATUS" "$JSON_CHECKS" "$CRITERIA_TSV" <<'PYEOF' > "$out_dir/verification-report.json"
import json, sys
git_sha, dirty, toolchain, platform, started, status, checks_path, criteria_path = sys.argv[1:9]
checks = [json.loads(l) for l in open(checks_path)]
criteria = []
for line in open(criteria_path):
    line = line.strip()
    if not line:
        continue
    num, rest = line.split("\t", 1)
    crit, evidence = rest.split("|", 1)
    criteria.append({"criterion": int(num), "title": crit.strip(),
                     "evidence": evidence.strip(),
                     "status": "pass" if status == "pass" else "see failing check"})
print(json.dumps({
    "gate": "agent-os-microkernel-mvp",
    "status": status,
    "git_sha": git_sha,
    "working_tree_dirty_files": int(dirty),
    "toolchain": toolchain,
    "platform": platform,
    "started_at_utc": started,
    "checks": checks,
    "exit_criteria": criteria,
}, indent=2))
PYEOF

python3 - "$GIT_SHA" "$GIT_DIRTY" "$TOOLCHAIN" "$PLATFORM" "$STARTED_AT" \
  "$STATUS" "$JSON_CHECKS" "$CRITERIA_TSV" <<'PYEOF' > "$out_dir/verification-report.md"
import json, sys
git_sha, dirty, toolchain, platform, started, status, checks_path, criteria_path = sys.argv[1:9]
checks = [json.loads(l) for l in open(checks_path)]
criteria = []
for line in open(criteria_path):
    line = line.strip()
    if not line:
        continue
    num, rest = line.split("\t", 1)
    crit, evidence = rest.split("|", 1)
    criteria.append((num.strip(), crit.strip(), evidence.strip()))
out = []
out.append("# Agent OS MVP verification report")
out.append("")
out.append(f"- **verdict**: {status.upper()}")
out.append(f"- **git SHA**: `{git_sha}` (dirty files: {dirty})")
out.append(f"- **toolchain**: {toolchain}")
out.append(f"- **platform**: {platform}")
out.append(f"- **started**: {started}")
out.append("")
out.append("## Gate checks")
out.append("")
out.append("| check | status | seconds |")
out.append("|---|---|---|")
for c in checks:
    out.append(f"| {c['name']} | {c['status']} | {c['seconds']} |")
out.append("")
out.append("## MVP exit criteria")
out.append("")
out.append("| # | criterion | evidence |")
out.append("|---|---|---|")
for num, crit, evidence in criteria:
    out.append(f"| {num} | {crit} | {evidence} |")
out.append("")
print("\n".join(out))
PYEOF

echo ""
echo "=== release gate: $STATUS ==="
echo "report: $out_dir/verification-report.json"
echo "report: $out_dir/verification-report.md"
exit "$GATE_FAIL"
