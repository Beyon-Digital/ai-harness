> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Types and IDs

## Identifier encoding

Persisted entity identifiers are UUIDv7 values. The canonical serialized form is lowercase
hyphenated (`8-4-4-4-12` hexadecimal, for example `018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70`).
A persisted, logged, or wire identifier that is not in this form is not a valid identifier.

Strong Rust newtypes are mandatory; raw `String` identifiers do not cross internal kernel
APIs (R4.3). Proto fields stay `string` on the wire and convert at the boundary.

```rust
RunId(Uuid)              TaskId(Uuid)             SessionId(Uuid)
EffectId(Uuid)           EventId(Uuid)            WorkspaceId(Uuid)
LeaseId(Uuid)            ReservationId(Uuid)      TimerId(Uuid)
ConfigGenerationId(Uuid) ApprovalRequestId(Uuid)  CapabilityGrantId(Uuid)
AdapterInstanceId(Uuid)  PrincipalId(Uuid)        ActorId(Uuid)
DeviceId(Uuid)           CommandId(Uuid)          DecisionId(Uuid)
TurnId(Uuid)             OperationId(Uuid)        AgentSpecId(Uuid)
AdapterId(Uuid)          DependencyId(Uuid)       DelegationChainId(Uuid)
ArtifactId(Uuid)         SandboxId(Uuid)          EnvironmentId(Uuid)
DaemonInstanceId(Uuid)
```

Rules:

- `UuidV7::to_hyphenated()` returns the lowercase hyphenated form; parsers accept only that
  canonical form (upper case is rejected, not normalized).
- Parsing validates the version nibble is `7`. Empty input, a non-UUID string, or a UUID whose
  version is not 7 returns the stable invalid-identifier error (`InvalidId`) and no state is
  mutated (R4.2).
- `Display` and `FromStr` round-trip byte-identically for every valid value.
- `UuidV7::new(provider: &dyn IdProvider)` draws the value from the injected provider;
  `SystemIdProvider` uses `uuid::Uuid::now_v7()`. Generated values must be unique within the
  store (R4 boundary).

## Idempotency keys

`IdempotencyKey` is a typed string: non-empty, at most 255 bytes, and containing no ASCII
control characters. A key is scoped by `(principal_id, idempotency_key)`. Same key with the
same request digest is a replay; same key with a different digest is a conflict and mutates
nothing.

## Versions

- Adapter versions, protocol versions, and every `VersionedRef.version` are semantic versions
  per SemVer 2.0.0: `MAJOR.MINOR.PATCH`, optionally followed by a pre-release and build
  suffix. No `v` prefix; numeric components have no leading zeros.
- The major component is unambiguously extractable by splitting on `.` and parsing the first
  component as a decimal integer (R4.4). Compatibility checks compare majors; an incompatible
  major is rejected.
- Digests referenced alongside a version (`VersionedRef.digest`, `adapter_digest`,
  `body_digest`, `manifest_digest`) use the digest encoding below.

## Digests

All content, request, and body digests are SHA-256, persisted and displayed as lowercase
hexadecimal with no prefix and no `0x`: exactly 64 `[0-9a-f]` characters (R3.4).

### Request digest

`request_digest` is `SHA-256(digest_input)` where `digest_input` is the canonical byte string
defined below (R3.1).

1. Covered envelope fields, in exactly this order, each preceded by its UTF-8 byte length as
   a 4-byte big-endian unsigned integer (`u32be`):

```text
digest_input =
    u32be(len(command_id))        || utf8(command_id)
 || u32be(len(idempotency_key))   || utf8(idempotency_key)
 || u32be(len(principal_id))      || utf8(principal_id)
 || u32be(len(actor_id))          || utf8(actor_id)
 || u32be(len(device_id))         || utf8(device_id)
 || u32be(len(command_type))      || utf8(command_type)
 || u32be(len(payload_bytes))     || payload_bytes
```

2. Identifier-valued fields use the canonical UUIDv7 encoding above; `device_id` is encoded
   as zero bytes when absent; `command_type` is the fully-qualified protobuf message name of
   the payload (design D2).
3. `payload_bytes` is the command payload message serialized with protobuf deterministic
   serialization: ascending field-number order, repeated fields in element order, map entries
   sorted by key, no unknown fields, and no duplicate scalar fields. An empty payload
   serializes to zero bytes and has a defined, stable digest (R3 boundary).

Field coverage is exhaustive; fields not listed are excluded and never affect the digest:

| Envelope field | Covered | Reason |
|---|---|---|
| `command_id` | yes | request identity |
| `idempotency_key` | yes | replay identity |
| `principal_id` | yes | replay scope |
| `actor_id` | yes | authenticated authorship |
| `device_id` | yes | authenticated device; zero-length when absent |
| `command_type` | yes | selects the payload schema |
| `payload_bytes` | yes | full logical request content |
| `deadline_unix_ms` | no | delivery metadata; a retry may carry a new deadline |
| `correlation_id` | no | tracing metadata |
| `causation_id` | no | tracing metadata |
| `delegation_chain_id` | no | authenticated context, re-verified per request, not replay identity |
| `request_digest` | no | the computed value itself |

Properties:

- Two independent implementations following this section produce byte-identical digests for
  the same logical request (R3.2), and changing any single covered field changes the digest.
- The daemon recomputes the digest and rejects a mismatch before authorization, with no
  mutation.
- Same key and digest: return the stored outcome. Same key, different digest: conflict, no
  mutation.

## Stream keys and event cursors

`EventStreamKey` is the canonical stream key from `event-pipeline.md`:

```text
run/<run-id>               task/<task-id>            session/<session-id>
effect/<effect-id>         config/global
adapter/<adapter-id>/<version>/<digest>
security/principal/<principal-id>
```

A stream key is non-empty, contains no `:` and no whitespace or control characters, and is
lowercase; segments are separated by `/`. UUID segments use the canonical identifier encoding
and digest segments use the canonical digest encoding.

`EventCursor` identifies a journal position. Its grammar is:

```text
cursor     = "v1" ":" stream-key ":" sequence
stream-key = 1*( %x61-7A / %x30-39 / "-" / "." / "/" )
sequence   = "0" / %x31-39 *DIGIT
```

- `Display` writes `v1:{stream_key}:{sequence}` with the sequence as canonical decimal (no
  leading zeros), for example `v1:run/018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70:42` (design D15).
- `FromStr` accepts exactly the canonical form: the literal prefix `v1`, exactly three
  `:`-separated parts, a valid stream key, and a canonical decimal `u64`. Malformed input
  returns the stable parse error (`InvalidId`) (R4 boundary).
- For every valid cursor, `EventCursor::from_str(&cursor.to_string()) == cursor`; round-trips
  are byte-identical (R4).
- Resumption: a durable subscriber resumes from the last cursor it received; the journal
  delivers every event on that stream key with a strictly greater sequence and no event at or
  before the cursor (R4.5). After a retention gap the subscriber receives a `LagNotice`
  carrying `stream_key` and `resume_sequence`, whose cursor is
  `v1:{stream_key}:{resume_sequence}`; sequence advancement is never fabricated and no event
  is silently skipped.
- Persisted cursor columns (`runs.input_event_cursor`, `loop_turns.input_event_cursor`,
  `decisions.input_event_cursor`) carry this exact string encoding.
- Loop-decision fencing fields in `contracts/protocols/agent_loop.proto` carry the same
  encoding on the wire.

## Time

Persist wall-clock timestamps as UTC Unix milliseconds. Lease/timeout calculations use a
monotonic process clock while running; persisted expiry uses wall-clock plus defensive
recovery rules.

## Revisions

All revisions/epochs/sequences are unsigned 64-bit integers and monotonically increase within
their scope. Overflow is treated as fatal invariant exhaustion rather than wrapping.
