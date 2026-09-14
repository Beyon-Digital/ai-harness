# Task SEC-002 — Capability and permission engine

- Status: review-ready
- Agent: agent-sec002
- Spec: agentd-security
- Commit: `a1c01f9` — feat(permissions): capability engine and joint risk [SEC-002]
- Branch: feat/agentd-microkernel-mvp

## What changed

Implements the design's `permissions :: policy.rs` interface plus the
capability catalogue. `Capability`, `CapabilityAction`, and `CapabilityFamily`
stay defined in `identity` (D1) and are re-exported from `permissions`. The
engine is pure: `evaluate` performs no I/O, mutates nothing, and returns an
identical `Decision` for identical inputs.

| File | Change |
| --- | --- |
| `agent-os/crates/permissions/src/capabilities.rs` | new: contract family/action catalogue, `supports`, strict `parse_capability` |
| `agent-os/crates/permissions/src/policy.rs` | new: `ScopedTarget`, `DenyReason`, `Decision`, `ApprovalDraft`, `PermissionRequest`, `evaluate`, scope normalizers, `domain_matches` |
| `agent-os/crates/permissions/src/lib.rs` | module wiring and re-exports |
| `agent-os/crates/permissions/tests/policy.rs` | new: 23 deterministic tests + 2 proptests |

`agent-os/crates/permissions/Cargo.toml` needed no change: the crate already
declared `domain`, `errors`, `identity`, and the `proptest` dev-dependency, so
the four-path lease was sufficient.

Engine semantics:

- `evaluate` first rejects off-contract pairs (`Unsupported`), then intersects
  every hop's grant-backed capability set root through tip. A capability the
  tip claims but an ancestor narrowed away is `AncestorConstraint`; a
  capability no hop holds is `NoGrant`; any chain that fails
  `identity::validate_chain` (link gaps, unbacked claims, duplicate grants) is
  `NoGrant`.
- The tool's allow list, when present, is intersected on top of the chain and
  a capability the tool excludes is `ToolRestriction`. The tool can never add
  authority, so the confused-deputy case (child grants ∩ privileged helper
  allow list) fails closed.
- The scoped target must match the capability family: workspace targets for
  workspace actions, network domains for `network.connect`, a secret URI and
  optional egress for secret actions, a matching config operation, a positive
  child count for `agent.spawn`, and `None` for extension/effect/resource.
  Malformed or mismatched targets are `OutOfScope`.
- Workspace roots are normalized (canonical scheme, no `.`/`..` segments); a
  relative `path` is joined to the root and an absolute (`scheme://`) path is
  segment-prefix-matched against it, so traversal and foreign roots are
  rejected.
- Network domains are lowercased and DNS-validated. `*` matches any host; a
  leading `.` is the documented suffix rule (`.example.com` matches
  `example.com` and every subdomain); every other token matches exactly
  (`domain_matches` applies the rule to a concrete host).
- Joint risk upgrades secret `Use`/`SignOrAct` to `RequireApproval` when the
  effective authority (chain ∩ tool) holds `network.connect` or the secret
  target declares an egress list. The draft cites the exercised capabilities
  sorted by family then action, defaults to `now_ms + DEFAULT_APPROVAL_TTL_MS`
  (15 minutes), and carries a deterministic canonical nonce.
- `Allow` cites the tip hop's grant ids, deduplicated and sorted; denials are
  payload-free enum reasons, so no secret content or target data can leak.

## Acceptance criteria

### 1. R2.1 — one decision function, exactly one variant — MET

`evaluate(&PermissionRequest) -> Decision` returns `Allow { grant_refs }`,
`Deny { reason }`, or `RequireApproval { request }`. The catalogue only admits
the contract pairs: `catalogue_parses_every_contract_pair_and_rejects_unknown
_values` round-trips all 20 pairs and rejects `network.read`,
`workspace.bogus`, `bogus.read`, `read`, `""`, `"."`, `"workspace."`, `".read"`,
`"workspace.read.extra"`, `"secret.sign"`, and `"effect.unknown"`.
`an_off_contract_capability_denies_as_unsupported` covers the constructed
`network.read` pair.

### 2. R2.2 — confused-deputy intersection — MET

Effective authority is the intersection of chain grants, the tool allow list,
ancestor capabilities, and target policy. `every_contract_capability_allows_on_
a_matching_grant_and_target` exercises all 20 actions; `confused_deputy_
intersects_child_grants_with_the_tool_allow_list` proves a privileged helper
(`tool_capabilities = [workspace.write]`) cannot exercise authority the child
chain narrowed away (`NoGrant`); `tool_restriction_denies_capabilities_outside_
the_tool_allow_list` and `tool_allow_list_does_not_widen_authority` cover the
restriction direction; `a_tip_that_exceeds_its_ancestor_denies_with_ancestor_
constraint` covers ancestor narrowing; `an_invalid_chain_shape_denies_with_no_
grant` fails closed on a broken linkage.

### 3. R2.3 — scope checking — MET

`workspace_scope_accepts_normalized_roots_and_in_scope_paths` and
`workspace_scope_rejects_traversal_and_foreign_paths` cover normalized roots,
in-root relative/absolute paths, `..` traversal, absolute path escapes,
`workspace://acme/proj-evil`, malformed roots, and empty/double-slash paths.
`network_scope_accepts_exact_suffix_and_wildcard_domains` and
`network_scope_rejects_malformed_domains_and_empty_lists` cover exact, suffix,
wildcard, case-folded, empty, schemed, ported, underscore, and label-boundary
domains. `secret_scope_requires_a_uri_and_well_formed_egress` and
`scoped_targets_must_match_the_capability_family` cover secret, config,
child-count, and family-mismatch targets.

### 4. R2.4 — joint risk upgrade — MET

`secret_use_with_egress_requires_approval_with_a_deterministic_draft` asserts
the upgrade, draft operation/target/capabilities/expiry/nonce, and byte-equal
drafts across two evaluations. `secret_sign_or_act_with_held_network_connect_
requires_approval` covers the held-connect trigger and sorted capabilities.
`secret_use_without_egress_or_connect_allows` proves the rule is conditional;
`joint_risk_respects_the_tool_intersection` proves a tool without network
authority cannot trigger it; `denial_takes_precedence_over_joint_risk` proves
joint risk never rescues a denied request.

### 5. R2.5 / N2 — pure, deterministic, table-testable — MET

No I/O, timers, or mutation in `src/` (verified by scan);
`p2_every_input_yields_exactly_one_deterministic_variant` re-evaluates every
generated request and asserts equality, and `p2_tool_allow_lists_never_widen_
authority` asserts an empty allow list can never allow and that dropping the
tool list can never turn an allowed request into a denial. The proptests also
assert `Allow` cites only the tip's own grants, uniquely.

### 6. R2.6 — no secret content in reasons — MET

`DenyReason` is a payload-free enum; `denial_reasons_carry_no_target_content`
asserts a denied secret request's rendered decision contains neither
`secret://` nor the key name. Reason tokens are stable lowercase strings.

### 7. P2 — property coverage — MET

Both proptests run the default 256 cases each over randomized chains, tools,
targets, capabilities, and clocks.

### 8. No file outside `files:` changed — MET

```
$ git show --stat --format='%h %s' HEAD
a1c01f9 feat(permissions): capability engine and joint risk [SEC-002]

 agent-os/crates/permissions/src/capabilities.rs | 118 ++++++
 agent-os/crates/permissions/src/lib.rs          |   9 +-
 agent-os/crates/permissions/src/policy.rs       | 520 +++++++++++++++++
 agent-os/crates/permissions/tests/policy.rs     | 803 +++++++++++++++++++++++
 4 files changed, 1449 insertions(+), 1 deletion(-)
```

## Commands and evidence

### RED (tests written first)

`cargo test -p permissions` — real captured head before implementation:

```
error[E0432]: unresolved import `permissions::capabilities`
  --> crates/permissions/tests/policy.rs:12:18
   |
12 | use permissions::capabilities::{parse_capability, supports};
   |                  ^^^^^^^^^^^^ could not find `capabilities` in `permissions`

error[E0432]: unresolved import `permissions::policy`
  --> crates/permissions/tests/policy.rs:13:18
...
error[E0432]: unresolved imports `permissions::Capability`, `permissions::CapabilityAction`, `permissions::CapabilityFamily`
...
error: could not compile `permissions` (test "policy") due to 11 previous errors
```

### GREEN

`cargo test -p permissions`:

```
running 25 tests
...
test result: ok. 25 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.18s
```

`cargo test --workspace`:

```
workspace exit=0
TOTAL passed=361 failed=0
96 suites reported "test result: ok"
```

`cargo clippy --workspace --all-targets -- -D warnings`:

```
    Checking permissions v0.1.0 (/Users/jainamshah/Documents/GitHub/ai-harness/agent-os/crates/permissions)
    Checking secrets v0.1.0 (/Users/jainamshah/Documents/GitHub/ai-harness/agent-os/crates/secrets)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 28.37s
clippy exit=0
```

`cargo fmt --check`:

```
fmt exit=0
```

## Verification checklist

- [x] Tests written first, RED confirmed (11 compile errors from missing modules)
- [x] From `agent-os/`: `cargo test -p permissions` — exit 0, 25/25 new tests
- [x] `cargo test --workspace` — 361 passed, 0 failed, exit 0
- [x] `cargo clippy --workspace --all-targets -- -D warnings` — clean, exit 0
- [x] `cargo fmt --check` — exit 0
- [x] Pure engine: no I/O, no sleeps, no `unwrap()`/`expect()` outside tests
- [x] Commit scoped with the task id; `index.lock` retry loop present (no contention occurred)
- [x] No deferred-work markers

## Concerns

- `DenyReason::ExpiredGrant` is part of the design's decision taxonomy, but the
  fixed `PermissionRequest` shape carries no grant expiry and the pure engine
  cannot query `capability_grants.expires_at_ms`. The variant is defined for
  interface completeness; nothing in `permissions` currently returns it.
  Enforcing expiry belongs to the chain-loading path (SEC-001/whatever owns
  grant rows), not to `evaluate`.
- `ApprovalDraft::extension_digest` and `config_digest` are always `None` from
  `evaluate` because `PermissionRequest` has no digest fields. Callers that own
  a bundle or config generation pass their digest when they build the approval
  digest in SEC-003.
- The nonce is a deterministic canonical fingerprint
  (`v1|principal|actor|run|capability|target|now_ms`) because the engine is
  required to be pure and cannot draw randomness; identical requests therefore
  produce identical drafts (idempotent), and uniqueness/replay protection stays
  with approval request ids in SEC-003.
- `ScopedTarget` describes the requested target, not the stored grant scope
  blob; the chain carries grant-backed capability sets only. Grant-scope
  matching against `capability_grants.scope` therefore happens where grants are
  constructed/loaded, and `evaluate` owns target-family matching plus the
  documented normalization/traversal/suffix rules.
- Any secret egress list triggers the joint upgrade, not only "broad" egress:
  the engine cannot rank destination sensitivity, so it chooses the
  conservative reading of R2.4 ("secret plus unrestricted egress"). A scoped
  egress list that should stay `Allow` would need an explicit policy input
  that the current interface does not have.
- Workspace path comparison is segment-aware and case-sensitive; network
  domains are case-insensitive. The difference is documented in `policy.rs`.
- `.spec/agentd-security/{ledger,tasks}.md` carry spec-flow bookkeeping edits
  from claim/start/review. They are outside the lease and intentionally left
  uncommitted; they are not part of `a1c01f9`.
- The untracked `agent-os/crates/run-graph/tests/graph.proptest-regressions`
  pre-dated this task and was left untouched.
- The machine briefly ran out of disk while building; the 8 GB
  `agent-os/target/debug/incremental` cache was deleted to recover space. No
  source file outside the lease was affected.

---

# Fix report — review follow-up

- Status: review-ready (task not marked done)
- Commit: `f1999ba` — fix(permissions): enforce grant scopes and bind approval digests [SEC-002]
- Design basis: amended `permissions :: policy.rs` block (`GrantScope`,
  `PermissionRequest.grant_scopes`, digest inputs) and decisions D9/D10.

## Finding 1 (Critical) — grant-scope enforcement — FIXED

`evaluate` no longer allows a target merely because the chain holds the
capability. It now resolves the tip hop's grants against the caller-supplied
`grant_scopes` rows:

- `Allow` requires at least one tip grant whose `capability` matches the
  request, that is unexpired (`expires_at_ms > now_ms`; exactly at expiry is
  expired), and that is either unscoped (`scope: None`, allows any well-formed
  target) or whose scope covers the request target.
- `Allow { grant_refs }` cites exactly the covering, matching, unexpired tip
  grants, sorted and unique.
- `Deny { OutOfScope }` when matching unexpired grants exist but none covers
  the target (this includes malformed/family-mismatched target shapes).
- `Deny { ExpiredGrant }` when every matching grant has lapsed.
- `Deny { NoGrant }` when no supplied grant row backs the held capability
  (fail closed), consistent with D9's caller-supplied metadata.
- Coverage now actually runs the existing matchers: workspace composed
  root+path segment-prefix (`workspace://acme/proj` does not cover
  `workspace://evil/proj`; `workspace://acme` covers `workspace://acme/proj`),
  `domain_matches` for concrete network hosts plus suffix/wildcard coverage,
  secret URI segment-prefix and egress bounding, config operation equality,
  and child-count bounds.
- Duplicate rows for one grant id aggregate conservatively (a grant is a
  candidate only when every row for it is unexpired and covering).

New tests: `a_workspace_grant_scope_covers_its_prefix_and_rejects_foreign_roots`,
`a_workspace_grant_scope_covers_nested_workspaces`,
`a_network_grant_scope_covers_only_matching_destinations`,
`expiry_decides_between_allow_out_of_scope_and_expired`,
`expired_grants_are_ignored_while_other_backing_grants_cover`,
`allow_cites_exactly_the_covering_grants`,
`grants_not_held_by_the_tip_do_not_authorize`,
`a_capability_without_grant_scope_rows_denies_with_no_grant`,
`unscoped_grants_allow_any_well_formed_target`,
`a_secret_grant_scope_bounds_uri_prefix_and_egress`,
`config_and_child_count_grants_bound_their_targets`, and the proptest
`p2_missing_grant_scope_metadata_never_allows`.

## Finding 2 (Important) — digest propagation — FIXED

`PermissionRequest` now carries `extension_digest` and `config_digest`, and
`approval_draft` copies both into the draft so `approvals::create_request` can
bind bundle/config content. New test `drafts_propagate_extension_and_config_
digests`, and the P2 proptest asserts draft digests equal the request digests.

## Minor — joint-risk egress tightening — FIXED

Joint risk now upgrades secret `Use`/`SignOrAct` only when the request's egress
is unbounded: the egress list contains the global `*` wildcard, or it is empty
(egress declared without bounding it). A bounded list of concrete hosts or
documented suffixes stays `Allow`. Network-connect intersection and the
existing tool-intersection behavior are unchanged. New test
`bounded_secret_egress_allows_while_unbounded_egress_requires_approval`; the
deterministic-draft test now uses `egress=["*"]`. The exact rule is documented
on `joint_risk` and in the module header.

## Commands and evidence

RED (new tests against the previous engine, via `git stash` of `src/`):

```
error[E0432]: unresolved import `permissions::policy::GrantScope`
  --> crates/permissions/tests/policy.rs:17:52
...
error[E0560]: struct `PermissionRequest` has no field named `grant_scopes`
...
error[E0560]: struct `PermissionRequest` has no field named `extension_digest`
...
error[E0560]: struct `PermissionRequest` has no field named `config_digest`
```

GREEN (from `agent-os/`):

```
cargo test -p permissions
test result: ok. 39 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.94s

cargo test --workspace
workspace exit=0
TOTAL passed=375 failed=0
96 suites reported "test result: ok"

cargo clippy --workspace --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 26.10s
clippy exit=0

cargo fmt --check
fmt exit=0
```

```
$ git show --stat --format='%h %s' HEAD
f1999ba fix(permissions): enforce grant scopes and bind approval digests [SEC-002]

 agent-os/crates/permissions/src/lib.rs      |   4 +-
 agent-os/crates/permissions/src/policy.rs   | 288 +++++++++--
 agent-os/crates/permissions/tests/policy.rs | 762 ++++++++++++++++++++++--
 3 files changed, 971 insertions(+), 83 deletions(-)
```

## Open concerns after the fix

- Tip-grant scoping: backing candidates are restricted to grants the chain's
  tip hop holds, because that hop holds the exercised authority. An ancestor
  grant with the same capability and a wider scope does not authorize the tip's
  request; child grants must carry their own scope rows. This is the fail-closed
  reading of "a chain grant whose capability matches".
- `Deny { OutOfScope }` vs `Deny { ExpiredGrant }` precedence for a malformed
  target with an expired matching grant: expiry is classified first, so the
  reason is `ExpiredGrant`. Both fail closed.
- `Some([])` egress is now a valid, unbounded declaration (canonical token
  `...|egress=*`) and triggers joint risk; an unbounded grant scope
  (`Some([])` or `*`) is required to cover it. Bounded `None` egress remains
  distinct from an empty declared list.
- The draft nonce remains a deterministic template fingerprint per D10;
  `approvals::create_request` assigns the canonical nonce it binds.
- The remaining concerns from the original report (caller-supplied
  `grant_scopes` trust, case-sensitivity split, spec-flow bookkeeping files
  left uncommitted) are unchanged.
- Disk recovered safely before the workspace gate (6.8 GB free after the run);
  no source outside the lease was touched.
