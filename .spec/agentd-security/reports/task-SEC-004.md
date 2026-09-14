# Task SEC-004 — Secrets broker and macOS Keychain backend

- Status: review-ready
- Agent: agent-sec004
- Spec: agentd-security
- Commit: `2101de0` — feat(secrets): broker, in-memory store, and macOS Keychain [SEC-004]
- Branch: feat/agentd-microkernel-mvp

## What changed

Implements the design's `secrets` interface: `SecretMetadata`, `SecretValue`
(zeroizing, redacted `Debug`, explicit `expose`), `ActResult`, the
`SecretStore` trait, `SecretUseRequest`, `SecretUseOutcome`,
`SecretAuditRecord`, `AuditSink`, `SecretsBroker`, `InMemorySecretStore`, and
the target-gated `MacOsKeychainSecretStore`.

| File | Change |
| --- | --- |
| `agent-os/crates/secrets/src/lib.rs` | new public surface: metadata/value/act types, `SecretStore`, `SecretUseRequest`, audit types and sink trait, target-gated re-export |
| `agent-os/crates/secrets/src/broker.rs` | `SecretsBroker` (authorize-before-store, audit, deny/approval errors) and `InMemorySecretStore` plus `NullAuditSink` |
| `agent-os/crates/secrets/src/keychain.rs` | target-gated `MacOsKeychainSecretStore` over a narrow `security-framework` passwords wrapper (no shell interpolation) |
| `agent-os/crates/secrets/tests/broker.rs` | 11 tests: redaction, unauthorized denial, joint egress approval, scope narrowing, sign-or-act preference, in-memory contract, audit content |
| `agent-os/crates/secrets/tests/keychain.rs` | 2 opt-in macOS Keychain tests (gate, dedicated service, own item create/delete, cleanup guard) |

Broker semantics:

- `resolve_metadata` and `use_secret` require `secret.use`; `sign_or_act`
  requires `secret.sign_or_act`. Every path calls `permissions::evaluate`
  with the request's principal/actor/run/chain, caller-loaded `grant_scopes`,
  and `ScopedTarget::Secret { uri, egress }` **before** any store call.
- `Deny` → `FailedPrecondition`/`Never`; `RequireApproval` (joint egress) →
  `Conflict`/`Never`; both release no material, carry only a static message
  plus the stable deny-reason token, and are audited.
- `Allow` returns `SecretValue` whose `Debug` is `[REDACTED]`; no `Display`,
  no serialization. Denials, store failures, and audit records contain no
  value bytes.
- Audit records contain actor, run, uri, and outcome only (`Allowed`,
  `Acted`, `Denied`, `ApprovalRequired`, `Failed`) and implement
  `observability::Classified` (`Internal`).
- `InMemorySecretStore::sign_or_act` returns a deterministic scoped reference
  (`scoped:v1:<sha256>`) derived inside the backend; the value never leaves.
- `MacOsKeychainSecretStore` stores the value at `service`/`uri` and metadata
  at `service`/`uri#metadata` via `security-framework` byte APIs. Missing
  items are `NotFound`/`Never`; other backend failures are
  `Unavailable`/`Safe` with no plaintext fallback; `sign_or_act` is
  `FailedPrecondition` because generic-password items cannot sign.

## Acceptance criteria

### R4.1 — broker evaluates permission and target/egress first — MET

Every broker method authorizes before touching the store; the counting store
in `unauthorized_use_is_denied_before_the_store_is_touched` and
`metadata_resolution_goes_through_authorization` records zero store calls on
denial, and `joint_egress_requires_approval_and_releases_no_material` records
zero on the approval upgrade.

### R4.2 — denial/approval releases nothing and carries no secret content — MET

Denials are `FailedPrecondition` and approvals `Conflict` (both `Never`);
tests assert no store call and that error `Display`/`Debug` contain no secret
bytes (`debug_and_error_paths_never_render_secret_material`).

### R4.3/P4/N1 — zeroizing, never rendered raw — MET

`SecretValue` wraps `zeroize::Zeroizing<Vec<u8>>`, has no `Display` or
serialization, and `Debug` prints `[REDACTED]`; the redaction test checks the
rendered marker and the absence of the material in error and audit renderings.

### R4.4 — scoped operation preferred over raw material — MET

`sign_or_act_is_preferred_over_raw_material` calls the scoped path on a chain
holding both capabilities and asserts the backend's `get` was never invoked;
`sign_or_act_capability_does_not_grant_raw_access` proves `sign_or_act` does
not widen to raw access.

### R4.5 — audit carries actor/run/target/outcome only — MET

`audit_records_carry_identity_uri_and_outcome_only` asserts exactly the four
fields, both success and backend-failure outcomes, and no material in the
rendered records.

### R4.6 — in-memory backend shares the Keychain contract — MET

`in_memory_store_implements_the_shared_contract` covers metadata round-trip,
raw read, `NotFound` for both absent reads, and deterministic sign-or-act; the
opt-in Keychain test repeats the same round-trip and missing-item contract.

### R4.7 — only brokered material, no environment pass-through, narrow Keychain — MET

The broker is the only path to stores; `security-framework` is called with
byte slices (no shell, no argv), and the Keychain error mapping is stable
`NotFound`/`Unavailable` with no fallback.

### N2 — opt-in Keychain tests, no sleeps — MET

`tests/keychain.rs` returns early with a printed note unless
`AGENTD_KEYCHAIN_TEST=1`, uses the dedicated service
`dev.agentd.secrets.sec004.opt-in`, and creates/deletes its own item with a
drop guard. No sleeps anywhere; the in-memory suite is deterministic.

### Files — MET

Only the five leased paths changed in commit `2101de0`; no other path was
touched.

## Commands and evidence

### RED (tests first, before implementation)

```
$ cargo test -p secrets
error[E0432]: unresolved import `secrets::broker`
  --> crates/secrets/tests/broker.rs:18:14
   |
18 | use secrets::broker::InMemorySecretStore;
   |              ^^^^^^ could not find `broker` in `secrets`

error[E0432]: unresolved imports `secrets::ActResult`, `secrets::AuditSink`,
`secrets::SecretAuditRecord`, `secrets::SecretMetadata`, `secrets::SecretStore`,
`secrets::SecretUseOutcome`, `secrets::SecretUseRequest`, `secrets::SecretValue`,
`secrets::SecretsBroker`
  --> crates/secrets/tests/broker.rs:20:5
   |
20 |     ActResult, AuditSink, SecretAuditRecord, SecretMetadata, SecretStore, SecretUseOutcome,
   |     ^^^^^^^^^  ^^^^^^^^^  ^^^^^^^^^^^^^^^^^  ^^^^^^^^^^^^^^  ^^^^^^^^^^^  ^^^^^^^^^^^^^ no `SecretUseOutcome` in the root
21 |     SecretUseRequest, SecretValue, SecretsBroker,
   |     ^^^^^^^^^^^^^^^^  ^^^^^^^^^^^  ^^^^^^^^^^^^^ no `SecretsBroker` in the root
error: could not compile `secrets` (test "broker") due to 2 previous errors
```

### GREEN — `cargo test -p secrets --features ''` (from `agent-os/`)

```
running 11 tests
test debug_and_error_paths_never_render_secret_material ... ok
test unauthorized_use_is_denied_before_the_store_is_touched ... ok
test joint_egress_requires_approval_and_releases_no_material ... ok
test unbounded_egress_without_network_authority_requires_approval ... ok
test scoped_grant_covers_child_secret_uris_without_widening ... ok
test bounded_egress_below_the_joint_rule_is_allowed ... ok
test sign_or_act_is_preferred_over_raw_material ... ok
test sign_or_act_capability_does_not_grant_raw_access ... ok
test in_memory_store_implements_the_shared_contract ... ok
test audit_records_carry_identity_uri_and_outcome_only ... ok
test metadata_resolution_goes_through_authorization ... ok
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

running 2 tests
test keychain_round_trip_matches_the_shared_contract ... ok
test broker_reads_authorized_material_from_the_keychain ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
exit=0
```

### Keychain gate

```
$ cargo test -p secrets --test keychain -- --nocapture          # gate unset
running 2 tests
AGENTD_KEYCHAIN_TEST is not set to 1; skipping the opt-in Keychain integration test
AGENTD_KEYCHAIN_TEST is not set to 1; skipping the opt-in Keychain integration test
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ AGENTD_KEYCHAIN_TEST=1 cargo test -p secrets --test keychain -- --nocapture
running 2 tests
test keychain_round_trip_matches_the_shared_contract ... ok
test broker_reads_authorized_material_from_the_keychain ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 14.04s
exit=0
```

### Gates (from `agent-os/`)

```
$ cargo test --workspace
exit=0; 397 tests passed across 99 "test result: ok" groups; 0 failures

$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.30s
exit=0

$ cargo fmt --check
exit=0
```

## Concerns

1. `SecretUseRequest` carries `grant_scopes: Vec<GrantScope>` beyond the
   design sketch because `permissions::evaluate` requires the caller-loaded
   grant rows (the engine is pure and I/O-free); the broker passes
   `tool_capabilities: None`. A later component with a grant loader can extend
   the request without changing the broker seam.
2. Metadata resolution authorizes with `secret.use`, so a sign-or-act-only
   chain can act but not read metadata. If metadata must be visible to
   sign-only actors, that is a deliberate policy change in the broker.
3. The Keychain backend cannot sign with generic-password items, so its
   `sign_or_act` is `FailedPrecondition`; the scoped-operation contract is
   carried by `InMemorySecretStore`, whose `scoped:v1:<sha256>` reference is a
   deterministic stand-in for provider issuance, not a cryptographic
   signature.
4. Audit delivery is an injected `AuditSink`; production wiring to the
   observability tracing substrate belongs to the runtime/daemon composition,
   since `secrets/Cargo.toml` has no `tracing` dependency to emit events
   directly. Records implement `observability::Classified` (`Internal`).
5. Per ruling 4 the Keychain suite is opt-in, so full Keychain coverage
   depends on running it locally with `AGENTD_KEYCHAIN_TEST=1`; the run above
   is from this machine.
