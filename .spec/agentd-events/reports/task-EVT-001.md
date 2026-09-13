# Task EVT-001 — Event primitives

- Status: complete (awaiting review)
- Agent: agent-evt001d
- Spec: agentd-events
- Commit: `584cc63` — feat(events): validated envelopes, stream keys, and cursors [EVT-001]
- Branch: feat/agentd-microkernel-mvp

## What changed

| File | Change |
| --- | --- |
| `agent-os/crates/events/Cargo.toml` | verified the prior attempt's `prost.workspace = true`; left as-is |
| `agent-os/Cargo.lock` | verified the prior attempt's `prost` edge on the `events` package |
| `agent-os/crates/events/src/stream.rs` | new: `StreamKind`, `StreamKey` constructors, strict `FromStr`/`Display` |
| `agent-os/crates/events/src/cursor.rs` | new: `pub use domain::ids::EventCursor` + `EventCursorExt::for_event` |
| `agent-os/crates/events/src/envelope.rs` | new: `ClassificationPolicy`, `CatalogClassificationPolicy::embedded()`, `EventBuilder`, `EventEnvelope` codec |
| `agent-os/crates/events/src/lib.rs` | declares `cursor`, `envelope`, `stream`; keeps `outbox`; re-exports crate-level names |
| `agent-os/crates/events/tests/primitives.rs` | new: 8 integration tests (written first, RED) |

No file outside the task's `files:` lease was modified or committed.

## Implementation notes

- **Manifest verification.** `crates/events/Cargo.toml` already carried
  `prost.workspace = true` (line 12) and `Cargo.lock` already listed `prost`
  under the `events` package; `cargo check -p events` passed before any source
  was written. Both edits are included in `584cc63` as leased files.
- **Catalog policy.** `include_str!("../../../proto/events/catalog.yaml")` is
  parsed with `serde_yaml` into `{ id, default_sensitivity }` entries; the
  floor map is built once in `embedded()`. There is no hard-coded event-type
  table (R1.5). Unknown types return `None` (no floor), per the task brief.
  Malformed YAML or an unknown `default_sensitivity` value fails closed with
  `Internal`/`Never`.
- **Cursor constructor.** `EventCursor` belongs to `domain`, so an inherent
  `impl EventCursor` in this crate is impossible under the orphan rule. The
  interface intent is preserved with the `EventCursorExt` trait; with the trait
  in scope the call site is `EventCursor::for_event(&key, sequence)` as specified.
- **`StreamKey` internals.** The wrapper caches its `StreamKind` so `kind()` is
  total (no fallback panic path); `as_str` still returns the canonical text.
  Parsing rejects unknown prefixes, wrong segment counts, empty segments, and
  non-canonical bytes (`InvalidArgument`/`Never`). `StreamKey::adapter` takes
  free-form `&str` parts and inherits the interface's infallible signature; it
  asserts the canonical precondition and is documented as panicking on
  non-canonical `version`/`digest`.
- **Builder.** `EventBuilder::new` draws a fresh `EventId` from
  `SystemIdProvider` (the design omits an id parameter, and the task's required
  list for `build` omits the id); an explicit `event_id` setter is provided for
  deterministic construction. `build` requires event type, non-zero version,
  sequence, occurred time, non-`Unspecified` sensitivity, and payload; retention
  defaults to `Standard` when unset. Downgrades below the catalog floor are
  `FailedPrecondition`/`Never`.
- **Codec.** `EventEnvelope` mirrors `contracts/events/event.proto` field for
  field and converts to/from `domain::generated::contract::EventEnvelope` with
  prost. `from_bytes` validates event id, stream key, optional ids, and
  sensitivity/retention presence (1-4 / 1-3) before returning. No error path
  renders input or payload bytes, and `KernelError::with_source` is safe because
  `Display` never prints the source.
- No `unwrap()`/`expect()` outside tests (verified with grep over `src/`).
- No deferred-work markers.

## Acceptance criteria

### R1.1–R1.4 — construction and validation rules enforced, downgrade rejected — MET

```
$ cargo test -p events
running 8 tests
test cursors_round_trip_and_malformed_forms_fail_closed ... ok
test envelope_decoding_fails_closed_without_echoing_payload ... ok
test embedded_policy_reads_floors_from_the_catalog ... ok
test every_stream_kind_round_trips ... ok
test stream_key_parsing_rejects_non_canonical_forms ... ok
test envelope_round_trips_every_field ... ok
test builder_rejects_classification_downgrades ... ok
test builder_requires_every_required_field ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
```

`builder_requires_every_required_field` covers event type, event version,
sequence, occurred time, classification (absent and `Unspecified`), and payload.
`builder_rejects_classification_downgrades` asserts
`CapabilityRequested` (floor `confidential`) refuses `public`/`internal` with
`FailedPrecondition`/`Never` and accepts `confidential`/`secret`.

### R1.2/R1.3 — canonical stream keys and cursors — MET

`every_stream_kind_round_trips` exercises all seven kinds through construct →
display → parse → equality; `stream_key_parsing_rejects_non_canonical_forms`
covers 24 bad keys; `cursors_round_trip_and_malformed_forms_fail_closed` covers
`v1::3`, `v2:x:1`, trailing segments, empty/leading-zero/non-numeric sequences,
and trailing text.

### R1.5 — floor comes from the embedded catalog — MET

`embedded_policy_reads_floors_from_the_catalog` asserts catalog-derived floors
(`CapabilityRequested`/`SecretActionPerformed` → confidential,
`SessionCreated`/`RunStarted`/`AdapterRegistered` → internal) and `None` for an
unlisted type.

### R1.6 — envelope round trip preserves every field — MET

`envelope_round_trips_every_field` builds a fully populated event with
non-UTF-8 payload bytes and asserts equality after `to_bytes`/`from_bytes`, plus
field-by-field assertions and the derived `cursor()`; a minimal event with all
optional fields absent round-trips as `None`.

### N1 — payload bytes never in error messages — MET

`builder_rejects_classification_downgrades` and
`envelope_decoding_fails_closed_without_echoing_payload` place a distinctive
canary in the payload/input bytes and assert it never appears in
`error.to_string()`.

### Gates — MET

```
$ cd agent-os
$ cargo test -p events            # exit 0
$ cargo clippy -p events --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.11s
    exit 0 (no warnings)
$ cargo fmt --check               # exit 0 (empty output)
```

## RED → GREEN

RED (`cargo test -p events --test primitives`, before implementation, exit 101):

```
error[E0432]: unresolved import `events::cursor`
  --> crates/events/tests/primitives.rs:9:13
error[E0432]: unresolved import `events::envelope`
  --> crates/events/tests/primitives.rs:10:13
error[E0432]: unresolved imports `events::StreamKey`, `events::StreamKind`
  --> crates/events/tests/primitives.rs:13:14
error: could not compile `events` (test "primitives") due to 5 previous errors
```

GREEN (`cargo test -p events --test primitives`, exit 0):

```
running 8 tests
test cursors_round_trip_and_malformed_forms_fail_closed ... ok
test envelope_decoding_fails_closed_without_echoing_payload ... ok
test embedded_policy_reads_floors_from_the_catalog ... ok
test every_stream_kind_round_trips ... ok
test stream_key_parsing_rejects_non_canonical_forms ... ok
test envelope_round_trips_every_field ... ok
test builder_rejects_classification_downgrades ... ok
test builder_requires_every_required_field ... ok

test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s
```

## Files committed

```
$ git show --stat --format='%h %s' 584cc63
584cc63 feat(events): validated envelopes, stream keys, and cursors [EVT-001]

 agent-os/Cargo.lock                        |   1 +
 agent-os/crates/events/Cargo.toml          |   1 +
 agent-os/crates/events/src/cursor.rs       |  24 ++
 agent-os/crates/events/src/envelope.rs     | 407 +++++++++++++++++++++++++++
 agent-os/crates/events/src/lib.rs          |   9 +
 agent-os/crates/events/src/stream.rs       | 162 ++++++++++
 agent-os/crates/events/tests/primitives.rs | 436 +++++++++++++++++++++++++++++
 7 files changed, 1040 insertions(+)
```

## Concerns

- **Design conflict on unknown types.** `design.md:225` says an unknown event
  type is "allowed with the type's implicit internal floor", while the task
  brief (twice) and `ClassificationPolicy::minimum`'s `Option` return say
  unknown types have no floor. Implemented the task brief: `None` → any
  classification allowed. If the implicit-internal reading is intended, the
  policy needs a documented default; flagging for review.
- **`StreamKey::adapter` free-form parts.** The interface returns `Self`, not
  `Result`, so non-canonical `version`/`digest` cannot be reported; the
  constructor asserts and panics on violation (documented). Callers should
  validate registration inputs upstream.
- **Retention default.** The brief lists classification but not retention as
  required; `build` defaults retention to `Standard` and refuses `Unspecified`.
  If EVT-003+ expects a per-catalog `default_retention`, the policy trait would
  need to expose it.
- **Cursor interface shape.** The design's literal `impl EventCursor` is not
  expressible cross-crate; the `EventCursorExt` trait preserves the call syntax
  but consumers must import the trait.
- `.spec/agentd-events/{ledger,tasks}.md` carry spec-flow bookkeeping from
  claim/start/review; they are outside this task's lease and are not part of
  `584cc63`.

---

# Fix report — review items 1-3 (commit `20aab94`)

- Status: in review (not done)
- Agent: agent-evt001d
- Commit: `20aab94` — fix(events): catalog retention defaults, redacted envelope debug, fallible adapter keys [EVT-001]
- Design: amended interfaces (`design.md:73`, `design.md:92-97`) implemented as written.

## Fix 1 — `EventEnvelope` Debug no longer renders payload bytes

`#[derive(Debug)]` was removed; a manual `fmt::Debug` prints every field plus
`payload_len` and never the payload bytes. `Clone`, `PartialEq`, and `Eq` are
kept, so field-level test comparisons still work.

New test `envelope_debug_elides_payload_bytes` builds an envelope whose payload
is `canary-payload-bytes`, renders `format!("{envelope:?}")`, and asserts the
output contains `payload_len: 20` and `RunStarted` but not `canary`.

## Fix 2 — `StreamKey::adapter` is fallible

Signature is now
`pub fn adapter(id: AdapterId, version: &str, digest: &str) -> errors::Result<Self>`
(`stream.rs:74`). `try_assemble` validates the composed key and returns
`InvalidArgument`/`Never` on empty or non-canonical `version`/`digest`; the
typed-ID constructors keep using an internal `canonical` helper whose only
failure path is impossible by construction. `FromStr` now shares
`try_assemble`.

New test `adapter_stream_keys_reject_malformed_external_parts` covers empty
version, empty digest, `1.0.0+build`, uppercase digest, `sha256:abc`, and
embedded whitespace; the round-trip case uses `.expect("canonical adapter key")`.

## Fix 3 — catalog retention defaults, no silent fallback

`ClassificationPolicy` now declares
`fn default_retention(&self, event_type: &str) -> Option<RetentionClass>`
(`envelope.rs:33`); `CatalogClassificationPolicy` parses `default_retention`
from the embedded catalog into a second map. In `build`, an explicit setter
wins; otherwise the catalog default is used; if neither exists the build fails
with `InvalidArgument`/`Never` ("event retention is required"). `Unspecified`
is still refused.

New test `builder_uses_catalog_retention_defaults_and_requires_a_class`:
`CapabilityRequested` defaults to `Audit`, `AdapterHealthy` defaults to
`Ephemeral`, explicit `Audit` overrides `AdapterHealthy`'s default, and an
unlisted type without explicit retention errors `InvalidArgument`/`Never`.
`embedded_policy_reads_floors_from_the_catalog` now also asserts the policy's
`default_retention` for `CapabilityRequested`/`AdapterHealthy`/`RunStarted` and
`None` for an unlisted type.

## Gates

```
$ cd agent-os
$ cargo test -p events
running 11 tests
test adapter_stream_keys_reject_malformed_external_parts ... ok
test builder_uses_catalog_retention_defaults_and_requires_a_class ... ok
test envelope_debug_elides_payload_bytes ... ok
... (11 passed; 0 failed)

$ cargo clippy -p events --all-targets -- -D warnings   # exit 0, no warnings
$ cargo fmt --check                                      # exit 0, empty output
```

## Files

```
$ git show --stat --format='%h %s' 20aab94
20aab94 fix(events): catalog retention defaults, redacted envelope debug, fallible adapter keys [EVT-001]

 agent-os/crates/events/src/envelope.rs     | 76 +++++++++++++++++++++---
 agent-os/crates/events/src/stream.rs       | 49 +++++++--------
 agent-os/crates/events/tests/primitives.rs | 95 +++++++++++++++++++++++++++++-
 3 files changed, 184 insertions(+), 36 deletions(-)
```

## Open concerns after fixes

- The `unreachable!` in the private `canonical` helper remains for typed-ID
  constructors; those inputs are compiler-checked (`UuidV7`) plus fixed
  literals, so it is an invariant guard, not external input.
- The unknown-type floor question from the original report still stands:
  `minimum` returns `None` per the task brief, while `design.md`'s error table
  mentions an implicit internal floor. Retention now errors for unknown types
  without an explicit class, which is stricter than replying on a silent
  default; the cross-file divergence is unchanged from the approved review and
  left as documented.
- `.spec/agentd-events/{design,ledger,tasks}.md` and the review diff artifact
  are outside the task lease and are not part of `20aab94`.
