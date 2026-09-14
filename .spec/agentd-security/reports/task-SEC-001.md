# Task SEC-001 — Principals, actors, delegation chains, and grant lineage

- Status: review-ready
- Agent: agent-sec001
- Spec: agentd-security
- Commit: `0fdfe38` — feat(identity): delegation chains and grant lineage [SEC-001]
- Branch: feat/agentd-microkernel-mvp

## What changed

Implements the design's `identity :: delegation.rs` interface. Capability
family/action types and the canonical `family.action` token live in `identity`
per D1; after `SEC-000`, `identity` already carried the required dependencies,
so no manifest or lock change was needed.

| File | Change |
| --- | --- |
| `agent-os/crates/identity/src/delegation.rs` | new: `CapabilityFamily`, `CapabilityAction`, `Capability`, `Hop`, `DelegationChain`, `persist_hop`, `load_chain`, `derive_child_chain`, `validate_chain`, grant-id blob codec |
| `agent-os/crates/identity/src/lib.rs` | `pub mod delegation;` |
| `agent-os/crates/identity/tests/delegation.rs` | new: 24 tests (23 deterministic + 1 proptest) against in-memory chains and a real SQLite store in a temp root |

`agent-os/crates/kernel-store-sqlite/src/repos/security.rs` is in the lease but
needed no change: `get_grant` and `list_delegation_hops` already cover every
read the service performs, so the file is untouched.

Interface behaviour:

- `Capability::from_str` accepts exactly the `family.action` pairs in
  `contracts/capabilities/security-capabilities.yaml` (`transfer_exclusive_write`,
  `shared_coordinated_write`, `sign_or_act`, `resolve_unknown` are the
  non-verbatim tokens) and rejects unknown families, actions, missing dots, and
  unsupported pairs such as `network.read`.
- `derive_child_chain` validates the parent chain, then returns the requested
  capabilities that the parent's tip hop holds, sorted and deduplicated.
  Any non-subset request is `InvalidArgument`/`Never`.
- `validate_chain` (pure, per the design signature) checks contiguous
  `0..n` hop linkage, duplicate hops / capabilities / grant ids, and the
  grant-backing shape: capabilities may never appear without explicit grant
  ids, and a hop may not claim more distinct capabilities than it has grants
  (each grant row carries exactly one capability). Every hop after the root
  must be a subset of its parent. Violations are
  `FailedPrecondition`/`Never`.
- `persist_hop` resolves every referenced grant, requires the declared
  capability set to equal the union of those grants, appends the hop
  contiguously, and re-checks the parent subset rule before inserting.
- `load_chain` decodes the persisted grant-id blob, fails on missing grants,
  unrecognized capability ids, malformed blobs, duplicate grant entries, and
  link gaps, reconstructs capabilities from the stored grants only, and then
  runs `validate_chain`. Because capabilities are never persisted per hop,
  authority cannot be inferred from ancestry without the stored grant ids
  (R1.5); a hop whose capabilities are not backed by its own grants is
  rejected.
- Grant ids persist as the concatenation of their canonical 36-byte
  hyphenated forms (documented in the module); the blob is decoded through
  `CapabilityGrantId::from_str`, so only canonical UUIDv7 text is accepted.

## Acceptance criteria

### 1. R1.1 — hops persist exact grant ids, parent hop, actor/run context — MET

`persist_hop` writes `NewDelegationHop` with the actor id, run id, hop index,
and the encoded exact grant list. The round-trip test inserts two grants and a
child grant, persists hops 0 and 1, and asserts the reloaded hops carry the
same `grant_ids`, `actor_id`, `run_id`, and grant-derived capabilities. The
parent hop is the previous `hop_index`, enforced contiguously by
`persist_hop` and `validate_chain`.

### 2. R1.2 / P1 — subset derivation, superset rejection — MET

`derive_returns_the_requested_subset_sorted_and_deduplicated`,
`derive_of_an_empty_request_succeeds_with_an_empty_set`,
`derive_rejects_a_request_the_parent_does_not_hold` (`InvalidArgument`), and
the proptest `derived_capability_sets_are_always_subsets_of_the_parent`
(96 cases, 0..8 picked capabilities, 0..2 extra narrowing hops) hold the P1
invariant: any `Ok` derivation is a subset of the parent tip, and the derived
set is exactly the requested set filtered by the parent. Empty requests
succeed with an empty set (boundary).

### 3. R1.3 / R1.4 — load-time integrity, rejected chains — MET

`load_rejects_a_hop_referencing_a_missing_grant`,
`load_rejects_a_chain_with_a_link_gap`,
`load_rejects_an_unrecognized_grant_capability`, and the pure
`validate_rejects_capabilities_that_exceed_the_parent`,
`validate_rejects_a_tampered_gap_between_hops`,
`validate_rejects_a_duplicated_hop_index`,
`validate_rejects_capabilities_without_grant_ids` all return
`FailedPrecondition`/`Never`. `the_store_rejects_a_duplicate_hop_row` proves
the schema PK rejects a duplicate hop as `Conflict`/`Never`, and
`persist_rejects_out_of_order_and_duplicate_hops` rejects it deterministically
before the write.

### 4. R1.5 — no ancestry-only inference — MET

`validate_rejects_capabilities_without_grant_ids` and
`validate_rejects_more_capabilities_than_grants` fail closed on a hop that
claims authority its stored grant ids cannot back; `load_chain` rebuilds every
hop's capabilities from the referenced grant rows, so ancestry alone can never
produce a capability; `derive_child_chain` first validates the parent and then
checks membership against the grant-backed tip.

### 5. N1 — no secret material in errors — MET

Every error is a static message via `KernelError`; no grant ids, capability
strings, scopes, or payloads are interpolated, and no logging was added.

### 6. No file outside `files:` changed — MET

```
$ git show --stat --format='%h %s' HEAD
0fdfe38 feat(identity): delegation chains and grant lineage [SEC-001]

 agent-os/crates/identity/src/delegation.rs   | 507 ++++++++++++++++++++
 agent-os/crates/identity/src/lib.rs          |   1 +
 agent-os/crates/identity/tests/delegation.rs | 671 +++++++++++++++++++++++++++
 3 files changed, 1179 insertions(+)
```

## Commands and evidence

### RED (tests written first)

`cargo test -p identity --test delegation` — real captured tail:

```
error[E0282]: type annotations needed
   --> crates/identity/tests/delegation.rs:636:19
...
error[E0432]: unresolved import `identity::delegation`
...
warning: unused import: `kernel_store::repositories::SecurityRepo`
   --> crates/identity/tests/delegation.rs:19:5
...
error: could not compile `identity` (test "delegation") due to 19 previous errors; 2 warnings emitted
```

### GREEN

`cargo test -p identity -p kernel-store-sqlite`:

```
     Running tests/delegation.rs (target/debug/deps/delegation-2efd67d783fd4a28)
test result: ok. 24 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.83s
...
test result: ok. 13 passed;   (repos_remaining)
test result: ok. 7 passed;    (rollback)
... all kernel-store-sqlite suites ok
exit=0
```

`cargo test --workspace`:

```
TOTAL passed=336 failed=0
exit=0
```

`cargo clippy --workspace --all-targets -- -D warnings`:

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 16.62s
clippy exit=0
```

`cargo fmt --check`:

```
fmt exit=0
```

## Verification checklist

- [x] Tests written first, RED confirmed (unresolved `identity::delegation`)
- [x] From `agent-os/`: `cargo test -p identity -p kernel-store-sqlite` — exit 0, 24/24 new tests
- [x] `cargo test --workspace` — 336 passed, 0 failed
- [x] `cargo clippy --workspace --all-targets -- -D warnings` — clean, exit 0
- [x] `cargo fmt --check` — exit 0
- [x] Real SQLite store in a `tempfile` temp root for every persistence test
- [x] No `unwrap()`/`expect()` outside tests; no sleeps
- [x] Commit scoped with the task id; `index.lock` retry loop present (no contention occurred)
- [x] No deferred-work markers

## Concerns

- `validate_chain(chain: &DelegationChain)` is pure per the design signature, so
  it cannot itself query grant rows. Grant existence is therefore enforced by
  the paths that build and persist chains: `load_chain` (R1.3 "when a chain is
  loaded") and `persist_hop`; `validate_chain` enforces the checkable proxy —
  capabilities are never present without explicit grant ids and never exceed
  the grant count. If a later task wants `validate_chain` to own existence
  directly, the signature would have to take a transaction, which would be a
  design change.
- The persisted `capability_grant_ids` blob format (concatenated canonical
  36-byte UUIDv7 text) is defined here because no contract specifies it; it
  matches the proto's `repeated string` shape and is decoded strictly. Any
  future writer of `delegation_hops` must use `persist_hop` rather than raw
  `SecurityRepo::insert_delegation_hop` to stay compatible.
- `Capability::from_str` follows the capability contract, so grant rows whose
  `capability_id` is an out-of-contract string (e.g. the
  `"filesystem.write"` used by the pre-existing store-level test) make a chain
  load fail closed as an integrity error. This is intentional (unknown values
  are rejected) but means chain loads only succeed for contract-valid grants.
- `.spec/agentd-security/{ledger,tasks}.md` carry spec-flow bookkeeping edits
  from claim/start/review. They are outside the lease and intentionally left
  uncommitted; they are not part of `0fdfe38`.
- The untracked `agent-os/crates/run-graph/tests/graph.proptest-regressions`
  pre-dated this task (timestamp 04:16, before this session) and was left
  untouched.
