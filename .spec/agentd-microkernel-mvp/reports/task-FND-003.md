# Task FND-003 — Implement domain IDs, mirror enums, and value types

- Status: **in review** (specflow: `FND-003 -> review (@agent-fnd003)`)
- Owner: @agent-fnd003
- Commit: `4bb0221` — `feat(domain): typed IDs, traits, and mirror enums [FND-003]`
- Depends on: FND-002 (generated contract module), GC-8 (effect enum value renames)

## What was implemented

All leased files were filled in; nothing outside the lease was touched.

### `src/ids.rs`

- `InvalidId` — unit error implementing `Display` (`invalid identifier`) and
  `std::error::Error`.
- `UuidV7` — wrapper around `uuid::Uuid` with `new(&dyn IdProvider)`, `as_uuid()`,
  `to_hyphenated()`, `FromStr` (`Err = InvalidId`), `Display`, and manual serde.
  Parsing accepts **only** the canonical lowercase hyphenated `8-4-4-4-12` form and
  rejects anything whose version nibble is not 7 (uppercase, simple/braced/URN forms,
  v4/nil all rejected, never normalized).
- `uuid_newtype!` macro generating all 28 newtypes from design.md (`RunId`, `TaskId`,
  `SessionId`, `EffectId`, `EventId`, `WorkspaceId`, `LeaseId`, `ReservationId`,
  `TimerId`, `ConfigGenerationId`, `ApprovalRequestId`, `CapabilityGrantId`,
  `AdapterInstanceId`, `PrincipalId`, `ActorId`, `DeviceId`, `CommandId`, `DecisionId`,
  `TurnId`, `OperationId`, `AgentSpecId`, `AdapterId`, `DependencyId`,
  `DelegationChainId`, `ArtifactId`, `SandboxId`, `EnvironmentId`, `DaemonInstanceId`)
  with `Copy/Eq/Hash/Ord`, `new`, `from_uuid_v7`, `as_uuid`, `as_uuid_v7`,
  `to_hyphenated`, `FromStr(Err = InvalidId)`, `Display`, and validated serde.
- `IdempotencyKey` — non-empty, at most 255 bytes, no ASCII control characters.
- `EventStreamKey` — canonical stream key: non-empty, only `a-z`, `0-9`, `-`, `.`, `/`
  (no `:`, whitespace, control characters, or uppercase).
- `EventCursor { stream_key, sequence }` — `v1:{stream_key}:{sequence}` with canonical
  decimal sequence (no leading zeros, `u64` overflow rejected), `Display`, `FromStr`,
  and serde; `new()` constructor.

### Traits (design D4)

- `provider.rs`: `IdProvider: Send + Sync + 'static` + `SystemIdProvider`
  (`Uuid::now_v7()`).
- `time.rs`: `Clock: Send + Sync + 'static` + `SystemClock` (`SystemTime`; pre-epoch
  saturates to 0, overflow saturates to `i64::MAX`; no panics).
- `faults.rs`: `FaultInjector: Send + Sync + 'static` + no-op `NoFaults`.

### Mirror enums

All 16 enums implement `from_wire(i32) -> Result<Self, UnknownEnumValue>` and
`to_wire(self) -> i32` via a shared `mirror_enum!` macro in `run.rs`.
`UnknownEnumValue { value, enum_name }` lives in `run.rs` and is re-exported from
`effect.rs`, `security.rs`, and `resource.rs`. Unknown values are always an error,
never a default.

| Module | Enums | Wire source |
|---|---|---|
| `run.rs` | `RunState`, `RecoveryDisposition` | `agent-os/proto/domain/core.proto` |
| `effect.rs` | `EffectClass`, `EffectState`, `IdempotencySemantics`, `ReconciliationSemantics` | `agent-os/proto/domain/effects.proto` (GC-8 names: `EFFECT_CLASS_READ_ONLY`=1, `EFFECT_STATE_FAILED`=6, `EFFECT_STATE_CANCELLED`=7) |
| `security.rs` | `SensitivityClass`, `RetentionClass` | `agent-os/proto/events/event.proto` |
| `security.rs` | `TrustState`, `ConformanceState`, `ApprovalState` | `agent-os/schema/kernel_store.sql` persisted-state order (0 = unspecified, then declaration order) |
| `resource.rs` | `WorkspaceAccessMode` | `agent-os/proto/domain/core.proto` |
| `resource.rs` | `DependencyCondition` | `agent-os/proto/domain/entities.proto` |
| `resource.rs` | `LeaseEnforcementState`, `TimerState`, `ReservationState` | `agent-os/schema/kernel_store.sql` persisted-state order |

### `tests/prop_enums.rs`

Tests are wrapped in `mod prop_enums` so `cargo test -p domain prop_enums` actually
selects them. One proptest per enum over `any::<i32>()`:

```rust
match <$mirror>::from_wire(value) {
    Ok(variant) => prop_assert_eq!(variant.to_wire(), value),
    Err(error) => prop_assert_eq!(error.value, value),
}
```

plus `unknown_wire_values_are_rejected_with_their_context` and a full
`wire_numbers_follow_the_contract_snapshot` table for every variant.

## Acceptance criteria

| # | Criterion | Result | Demonstrating command / evidence |
|---|---|---|---|
| R4.2 | invalid identifiers rejected with `InvalidId`; typed newtypes used everywhere | met | `cargo test -p domain invalid_ids_are_rejected` (`invalid_ids_are_rejected ... ok`); 28 `uuid_newtype!` invocations; only typed wrappers cross the API |
| R15.1 / P1 | unknown wire values never map to a default variant, proven by the property test | met | `cargo test -p domain prop_enums`; 16 round-trip proptests + `unknown_wire_values_are_rejected_with_their_context` pass |
| R15.2 | valid values round-trip exactly | met | same proptests: on `Ok`, `variant.to_wire() == value`; `wire_numbers_follow_the_contract_snapshot` pins every variant |
| R15.3 | generated IDs are UUIDv7 and unique in a quick loop test | met | `cargo test -p domain generated_ids_are_uuid_v7_and_unique` — 1000 `RunId`s, version nibble 7, no duplicates |
| — | no `unwrap()`/`expect()` in `src/` | met | `grep -n 'unwrap()\|expect(' agent-os/crates/domain/src/{ids,provider,time,faults,run,effect,security,resource}.rs` → no output (exit 1) |
| — | no file outside `files:` changed | met | `git show --stat 4bb0221` lists exactly the 9 leased paths |

### Verification commands

````text
$ cargo test -p domain
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out  (unittests src/lib.rs)
test result: ok.  2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out  (tests/contract_codegen.rs)
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out  (tests/prop_enums.rs)
test result: ok.  0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out  (Doc-tests)

$ cargo clippy -p domain --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 17s

$ cargo fmt --check
(no output; exit 0)
````

## RED / GREEN

RED — `tests/prop_enums.rs` written first against the unimplemented modules:

```text
$ cargo test -p domain prop_enums
   Compiling domain v0.1.0 (/Users/jainamshah/Documents/GitHub/ai-harness/agent-os/crates/domain)
error[E0432]: unresolved imports `domain::effect::EffectClass`, `domain::effect::EffectState`, `domain::effect::IdempotencySemantics`, `domain::effect::ReconciliationSemantics`
 --> crates/domain/tests/prop_enums.rs:4:22
  |
4 | use domain::effect::{EffectClass, EffectState, IdempotencySemantics, ReconciliationSemantics};
  |                      ^^^^^^^^^^^  ^^^^^^^^^^^  ^^^^^^^^^^^^^^^^^^^^  ^^^^^^^^^^^^^^^^^^^^^^^ no `ReconciliationSemantics` in `effect`
  |                      |            |            |
  |                      |            |            no `IdempotencySemantics` in `effect`
  |                      |            no `EffectState` in `effect`
  |                      no `EffectClass` in `effect`
  |
  = help: consider importing this enum instead:
          domain::generated::contract::EffectClass

error[E0432]: unresolved imports `domain::resource::DependencyCondition`, `domain::resource::LeaseEnforcementState`, `domain::resource::ReservationState`, `domain::resource::TimerState`, `domain::resource::WorkspaceAccessMode`
 --> crates/domain/tests/prop_enums.rs:6:5
  |
6 |     DependencyCondition, LeaseEnforcementState, ReservationState, TimerState, WorkspaceAccessMode,
  |     ^^^^^^^^^^^^^^^^^^^  ^^^^^^^^^^^^^^^^^^^^^  ^^^^^^^^^^^^^^^^  ^^^^^^^^^^  ^^^^^^^^^^^^^^^^^^^ no `WorkspaceAccessMode` in `resource`

error[E0432]: unresolved imports `domain::run::RecoveryDisposition`, `domain::run::RunState`
 --> crates/domain/tests/prop_enums.rs:8:19
  |
8 | use domain::run::{RecoveryDisposition, RunState};
  |                   ^^^^^^^^^^^^^^^^^^ no `RecoveryDisposition` in `run`
(output truncated after the unresolved-import errors; build aborted)
```

GREEN — after implementing IDs, traits, and the mirror enums:

```text
$ cargo test -p domain prop_enums
running 18 tests
test prop_enums::approval_state ... ok
test prop_enums::conformance_state ... ok
test prop_enums::dependency_condition ... ok
test prop_enums::effect_class ... ok
test prop_enums::effect_state ... ok
test prop_enums::idempotency_semantics ... ok
test prop_enums::lease_enforcement_state ... ok
test prop_enums::reconciliation_semantics ... ok
test prop_enums::recovery_disposition ... ok
test prop_enums::reservation_state ... ok
test prop_enums::retention_class ... ok
test prop_enums::run_state ... ok
test prop_enums::sensitivity_class ... ok
test prop_enums::timer_state ... ok
test prop_enums::trust_state ... ok
test prop_enums::unknown_wire_values_are_rejected_with_their_context ... ok
test prop_enums::wire_numbers_follow_the_contract_snapshot ... ok
test prop_enums::workspace_access_mode ... ok
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
```

## Files changed

- `agent-os/crates/domain/src/ids.rs`
- `agent-os/crates/domain/src/provider.rs`
- `agent-os/crates/domain/src/time.rs`
- `agent-os/crates/domain/src/faults.rs`
- `agent-os/crates/domain/src/run.rs`
- `agent-os/crates/domain/src/effect.rs`
- `agent-os/crates/domain/src/security.rs`
- `agent-os/crates/domain/src/resource.rs`
- `agent-os/crates/domain/tests/prop_enums.rs`

## Concerns

- **No protobuf source for six mirror enums.** `TrustState`, `ConformanceState`,
  `ApprovalState`, `LeaseEnforcementState`, `TimerState`, and `ReservationState` have
  no enum in `agent-os/proto/`; the pack stores them as TEXT literals. I encoded them
  as `0 = Unspecified` followed by the `CHECK`/spec declaration order
  (`kernel_store.sql`), e.g. `TrustState::Trusted = 1`, `ApprovalState::Expired = 4`,
  `TimerState::Fired = 3`, `ReservationState::Unknown = 5`. If a later task or a
  contract revision publishes canonical integers for these, this mapping must be
  revisited.
- **Serde serialization is compiled but not unit-tested.** `domain` has no
  `serde_json` (or equivalent) dev-dependency and `Cargo.toml` is outside the lease;
  deserialization validation is tested through `serde::de::value::StrDeserializer`.
  The serialization impls are trivial `serialize_str` calls of the canonical text.
- **`UuidV7::new` trusts the provider.** The design signature returns `Self`, so a
  misbehaving provider could yield a non-v7 UUID without a typed error; production
  wiring uses `SystemIdProvider`, and testkit's `DeterministicIds` must emit v7.
- **`EventStreamKey` follows the EBNF literally.** The grammar
  `1*( %x61-7A / %x30-39 / "-" / "." / "/" )` permits `//` and trailing `/`; I
  implemented the grammar rather than adding unstated segment rules.

---

## Fix after review (round 2)

- Commit: `1db07a7` — `fix(domain): schema-checked state strings and canonical serialization tests [FND-003]`
- Findings addressed: Important (six schema-backed enums had invented integer
  persistence semantics) and Minor (`Serialize` evidence missing from `ids.rs`).

### Important — text conversions for the six schema-backed enums

The six enums with no proto source (`TrustState`, `ConformanceState`,
`ApprovalState`, `LeaseEnforcementState`, `TimerState`, `ReservationState`) now carry
exact text conversions generated by a new `state_enum!` macro in `run.rs`:

- `as_str(&self) -> &'static str` — the exact `CHECK` literal;
- `from_state_str(&str) -> Result<Self, UnknownStateValue>` — accepts exactly the
  `CHECK` set and nothing else (no trimming, no case folding);
- `UnknownStateValue { value: String, enum_name: &'static str }` lives in `run.rs` and
  is re-exported from `security.rs`/`resource.rs`, next to `UnknownEnumValue`.

`as_str` is total over each enum, so the enums' string set is exactly the schema set:

| Enum | Text domain (schema `CHECK`) |
|---|---|
| `TrustState` | `trusted`, `untrusted` |
| `ConformanceState` | `untested`, `passed`, `failed` |
| `ApprovalState` | `pending`, `approved`, `denied`, `expired` |
| `LeaseEnforcementState` | `active`, `revoked` |
| `TimerState` | `scheduled`, `claimed`, `fired`, `cancelled` |
| `ReservationState` | `reserved`, `allocated`, `released`, `expired`, `unknown` |

The invented `Unspecified = 0` variant was removed from these six enums: `0` is not a
legal stored value, and keeping a variant whose `as_str` would have to invent
`"unspecified"` would have made the string set unequal to the schema set. `to_wire`
therefore emits only the provisional numbers `1..n`, `from_wire(0)` is an error, and
the docs on every one of the six state that the integer mapping is **provisional and
must not be used for persistence** (the macro injects that doc line into each enum).

Test `prop_enums::state_strings_equal_the_schema_check_domains` reads the in-repo
schema mirror `agent-os/schema/kernel_store.sql` (byte-identical to
`agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql`, verified with
`diff`), extracts each column's `CHECK (<col> IN (...))` literals, and asserts
**exact vector equality** with the variants' `as_str` output, plus
`from_state_str` round-trips and rejection of unknown strings:

```text
$ cargo test -p domain prop_enums
test prop_enums::state_strings_equal_the_schema_check_domains ... ok
test prop_enums::wire_numbers_follow_the_contract_snapshot ... ok
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

### Minor — `Serialize` evidence in `ids.rs`

Added `serialization_emits_canonical_string_forms`, backed by a test-only
`StringSerializer` implementing `serde::Serializer` (JSON-like `"..."` output). It
serializes `RunId` (lowercase hyphenated, e.g. `"018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70"`),
`IdempotencyKey`, and `EventCursor` (`"v1:run/<uuid>:42"`), locking R4.1. No
`unwrap()` was introduced in `src/` (tests use a generic `accepted` helper).

### Verification (from `agent-os/`)

```text
$ cargo test -p domain
test result: ok. 20 passed; 0 failed; 0 ignored; 0 measured  (unittests src/lib.rs)
test result: ok.  2 passed; 0 failed; 0 ignored; 0 measured  (tests/contract_codegen.rs)
test result: ok. 19 passed; 0 failed; 0 ignored; 0 measured  (tests/prop_enums.rs)
test result: ok.  0 passed; 0 failed; 0 ignored; 0 measured  (Doc-tests)

$ cargo clippy -p domain --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 27s

$ cargo fmt --check
(no output; exit 0)

$ grep -n 'unwrap()\|expect(' agent-os/crates/domain/src/{ids,provider,time,faults,run,effect,security,resource}.rs
(no output; exit 1)
```

```text
$ git show --stat --oneline 1db07a7
1db07a7 fix(domain): schema-checked state strings and canonical serialization tests [FND-003]
 agent-os/crates/domain/src/ids.rs          | 219 ++++++++++++++++++++++++++++-
 agent-os/crates/domain/src/resource.rs     |  54 ++++---
 agent-os/crates/domain/src/run.rs          |  74 ++++++++++
 agent-os/crates/domain/src/security.rs     |  49 ++++---
 agent-os/crates/domain/tests/prop_enums.rs | 109 +++++++++++++-
 5 files changed, 462 insertions(+), 43 deletions(-)
```

### Updated concerns

- **Resolved:** persistence text is now schema-locked for the six enum domains, and
  the illegal `0`/`Unspecified` state is gone from them; the integer mapping remains
  present only to satisfy the design-mandated `from_wire`/`to_wire` signatures and is
  documented as provisional.
- **Remaining:** the provisional integers (`1..n`) for the six enums are still not
  contract-published; if a future contract revision defines canonical wire numbers,
  `to_wire`/`from_wire` must be revisited (persistence is now insulated from them).
- **Remaining:** serde serialization is exercised only through the test-only
  `StringSerializer`; adding `serde_json` as a dev-dependency would allow a
  standard-path assertion, but `Cargo.toml` is outside the lease.
- `UuidV7::new` still trusts the provider to return a v7 UUID (design signature has
  no `Result`), and `EventStreamKey` still follows the pack EBNF literally.
