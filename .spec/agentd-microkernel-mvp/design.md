# Design — agentd-microkernel-mvp

**Status:** draft
**Date:** 2026-09-11
**Requirements:** `requirements.md` (approved)

Scope: the gap-closure wave and the foundation module, per the approved `plan.md`. Later
modules repeat phases 2–4 in their own spec directories and consume the interfaces fixed here.

## Architecture

```
┌─────────────────────────────── build pack (documentation) ───────────────────────────────┐
│  contracts/  ──► contract-lock.sha256        specs/  ──► dag.yaml ──► dag.json/DAG.md    │
│       │              │                           │            │        tasks.csv/*.md     │
│       │              └── validate_buildpack.py ◄─┴── limits.yaml, kernel-store-schema.sql│
│       │                          ▲                                                       │
│       ▼                          │                                                       │
│  agent-os/proto, agent-os/schema │                                                       │
└───────┬──────────────────────────┼───────────────────────────────────────────────────────┘
        │                          │ CI runs all validators
        ▼                          ▼
┌─────────────────────── agent-os workspace (one daemon, 31 library crates) ───────────────┐
│  domain/build.rs ──(vendored protoc + prost)──► OUT_DIR generated types                   │
│  domain (ids, enums, time, faults, provider traits)                                       │
│  errors (codes, retry class, KernelError)                                                 │
│  testkit (TestClock, DeterministicIds, ArmedFaults, TempDaemonHost)                       │
│  observability (Classification, Redacted, Secret, span helpers)                           │
│  agentd (placeholder composition root, pre-declared module roots)                         │
│  remaining 26 crates: empty stubs declared by FND-001                                     │
└───────────────────────────────────────────────────────────────────────────────────────────┘
```

| Component | Responsibility | New or existing | Path |
|---|---|---|---|
| Build pack contracts | Normative protobuf and JSON schemas, locked | existing, modified | `agent-os-microkernel-mvp-buildpack/contracts/` |
| Build-pack validator | Enforces lock, limits, DAG-source equality, schema coverage, module-root ownership, manifest | existing, modified | `agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py` |
| DAG sync script | Regenerates `dag.json`, `DAG.md`, `tasks.csv` from `dag.yaml` | new | `agent-os-microkernel-mvp-buildpack/scripts/sync_dag_sources.py` |
| Normative limits | Single machine-readable source of every threshold | new | `agent-os-microkernel-mvp-buildpack/specs/limits.yaml` |
| Contract mirror | Snapshot installed for compilation | new | `agent-os/proto/`, `agent-os/schema/` |
| domain crate | IDs, mirrored enums, time/provider/fault traits, generated types | new | `agent-os/crates/domain/` |
| errors crate | Machine-readable codes and structural retry classification | new | `agent-os/crates/errors/` |
| testkit crate | Deterministic test doubles and fault injection | new | `agent-os/crates/testkit/` |
| observability crate | Classification, redaction types, span helpers | new | `agent-os/crates/observability/` |
| agentd crate | Placeholder composition root; pre-declared module roots for later workers | new | `agent-os/crates/agentd/` |
| CI workflow | Quality gates, validators, lock and determinism checks | new | `agent-os/.github/workflows/ci.yml` |

## Data flow

**Happy path — developer build**

1. Developer runs `cargo check --workspace` in `agent-os/`.
2. `crates/domain/build.rs` starts, locates the vendored protoc via `protoc-bin-vendored`, and resolves include paths against `agent-os/proto/`.
3. `prost-build` compiles every `.proto` file under `agent-os/proto/` into one Rust module per package and writes it to `OUT_DIR`.
4. `crates/domain/src/generated.rs` includes the OUT_DIR artifact and re-exports the generated types.
5. All 31 crates compile; agentd's placeholder main exits zero without touching the database or sockets.
6. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` pass.
7. CI additionally runs `tools/validate_repo.py` and `scripts/validate_buildpack.py`, which verify the contract lock, normative limits, schema coverage, DAG-source equality, module-root ownership, and manifest hashes.

**Happy path — gap-closure verification**

1. Maintainer edits a normative pack file (for example `specs/limits.yaml` or `specs/kernel-store-schema.sql`).
2. `sync_dag_sources.py` regenerates derived DAG sources when `dag.yaml` changed.
3. `validate_buildpack.py` re-checks every rule, including mutation-tested negative cases from `scripts/test_validator_mutations.sh`.
4. `MANIFEST.json` is rewritten with `--write-manifest`, and `contract-lock.sha256` with `--update-lock`, as the final step of the change.

**Failure path — contract inconsistency**

1. A proto file gains a duplicate message or a snapshot file changes without a lock update.
2. `validate_buildpack.py` fails and names the file; CI blocks before any Rust build.
3. If the inconsistency slips through, `prost-build` or protoc fails inside `domain/build.rs` with file and line, and `cargo check` fails.
4. Codegen never emits partially written output: it writes to OUT_DIR only on success, so a failed build cannot leave stale generated types in the tree.

**Failure path — runtime conversion of unknown wire values**

1. A caller sends an enum value unknown to this build.
2. `prost`'s generated `TryFrom<i32>` yields an unknown-value error.
3. The domain conversion maps it to a typed error with a stable code; no default variant is substituted.

## Interfaces

### errors crate

```rust
// crates/errors/src/codes.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    InvalidArgument, NotFound, Conflict, FailedPrecondition,
    ResourceExhausted, Unavailable, Internal,
}
impl ErrorCode { pub const fn as_str(self) -> &'static str; }
impl std::fmt::Display for ErrorCode { /* stable lowercase token */ }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryClass { Never, Safe, ReconciliationRequired }

// crates/errors/src/lib.rs
#[derive(Debug)]
pub struct KernelError {
    code: ErrorCode,
    retry: RetryClass,
    message: std::borrow::Cow<'static, str>,
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}
impl KernelError {
    pub fn new(code: ErrorCode, retry: RetryClass,
               message: impl Into<std::borrow::Cow<'static, str>>) -> Self;
    pub fn with_source(self, source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self;
    pub fn code(&self) -> ErrorCode;
    pub fn retry_class(&self) -> RetryClass;
    pub fn message(&self) -> &str;
}
impl std::error::Error for KernelError { /* source() */ }
impl std::fmt::Display for KernelError { /* code + message; never secret content */ }
pub type Result⟨T⟩ = std::result::Result⟨T, KernelError⟩;
```

### domain crate

```rust
// crates/domain/src/ids.rs
pub struct InvalidId;
macro_rules! uuid_newtype { /* Copy, Eq, Hash, Ord, FromStr(Err = InvalidId), Display, serde */ }

pub struct UuidV7(uuid::Uuid);          // validates version nibble 7 on parse
impl UuidV7 {
    pub fn new(provider: &dyn IdProvider) -> Self;
    pub fn as_uuid(&self) -> &uuid::Uuid;
    pub fn to_hyphenated(&self) -> String;   // lowercase hyphenated
}
uuid_newtype!(RunId);      uuid_newtype!(TaskId);          uuid_newtype!(SessionId);
uuid_newtype!(EffectId);   uuid_newtype!(EventId);         uuid_newtype!(WorkspaceId);
uuid_newtype!(LeaseId);    uuid_newtype!(ReservationId);   uuid_newtype!(TimerId);
uuid_newtype!(ConfigGenerationId); uuid_newtype!(ApprovalRequestId);
uuid_newtype!(CapabilityGrantId);  uuid_newtype!(AdapterInstanceId);
uuid_newtype!(PrincipalId); uuid_newtype!(ActorId);        uuid_newtype!(DeviceId);
uuid_newtype!(CommandId);  uuid_newtype!(DecisionId);      uuid_newtype!(TurnId);
uuid_newtype!(OperationId); uuid_newtype!(AgentSpecId);    uuid_newtype!(AdapterId);
uuid_newtype!(DependencyId); uuid_newtype!(DelegationChainId);
uuid_newtype!(ArtifactId); uuid_newtype!(SandboxId);       uuid_newtype!(EnvironmentId);
uuid_newtype!(DaemonInstanceId);

pub struct IdempotencyKey(String);   // non-empty, length-bounded, no control characters
pub struct EventStreamKey(String);   // canonical stream-key encoding
pub struct EventCursor {             // format: v1:<stream-key>:<sequence, decimal u64>
    pub stream_key: EventStreamKey,
    pub sequence: u64,
}
impl std::str::FromStr for EventCursor { type Err = InvalidId; }
impl std::fmt::Display for EventCursor;

// crates/domain/src/provider.rs
pub trait IdProvider: Send + Sync + 'static { fn new_uuid_v7(&self) -> uuid::Uuid; }
pub struct SystemIdProvider;         // uuid::Uuid::now_v7()
impl IdProvider for SystemIdProvider;

// crates/domain/src/time.rs
pub trait Clock: Send + Sync + 'static { fn now_unix_ms(&self) -> i64; }
pub struct SystemClock;
impl Clock for SystemClock;

// crates/domain/src/faults.rs
pub trait FaultInjector: Send + Sync + 'static { fn trigger(&self, point: &str); }
pub struct NoFaults;
impl FaultInjector for NoFaults { fn trigger(&self, _point: &str) {} }

// crates/domain/src/run.rs      RunState, RecoveryDisposition, mirrored from contracts/domain/core.proto
// crates/domain/src/effect.rs   EffectState, EffectClass, IdempotencySemantics,
//                               ReconciliationSemantics from contracts/domain/effects.proto
// crates/domain/src/security.rs SensitivityClass, RetentionClass, TrustState, ConformanceState, ApprovalState
// crates/domain/src/resource.rs WorkspaceAccessMode, LeaseEnforcementState, TimerState,
//                               ReservationState, DependencyCondition
// Each mirror enum provides: pub fn from_wire(v: i32) -> Result<Self, UnknownEnumValue>;
// and pub fn to_wire(self) -> i32; unknown values are an error, never a default.

// crates/domain/src/generated.rs
pub mod contract { /* include!(concat!(env!("OUT_DIR"), "/...")) */ }
```

### testkit crate

```rust
// crates/testkit/src/clock.rs
pub struct TestClock { /* Arc<Mutex<i64>> */ }
impl TestClock {
    pub fn new(start_unix_ms: i64) -> Self;
    pub fn advance(&self, delta_ms: i64);
    pub fn set(&self, unix_ms: i64);
}
impl domain::time::Clock for TestClock;

// crates/testkit/src/ids.rs
pub struct DeterministicIds { /* fixed timestamp + AtomicU64 */ }
impl DeterministicIds { pub fn new(seed_ms: i64) -> Self; }
impl domain::provider::IdProvider for DeterministicIds;

// crates/testkit/src/faults.rs
#[derive(Default)]
pub struct ArmedFaults { /* Mutex<HashMap<String, FaultState>> */ }
impl ArmedFaults {
    pub fn new() -> Self;
    pub fn arm(&self, point: &str);
    pub fn is_triggered(&self, point: &str) -> bool;
    pub fn assert_triggered(&self, point: &str);   // panics with the point name if never fired
}
impl domain::faults::FaultInjector for ArmedFaults;  // fires an armed point exactly once

// crates/testkit/src/process.rs
pub struct TempDaemonHost { /* TempDir, Child, socket path, log files */ }
impl TempDaemonHost {
    pub fn new(daemon_bin: &std::path::Path) -> std::io::Result⟨Self⟩;
    pub fn home(&self) -> &std::path::Path;
    pub fn runtime_dir(&self) -> &std::path::Path;
    pub fn wait_for_exit(&mut self, timeout: std::time::Duration) -> std::io::Result<std::process::ExitStatus>;
}
impl Drop for TempDaemonHost;   // kill, wait, remove temporary tree
```

### observability crate

```rust
// crates/observability/src/classification.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Classification { Public = 0, Internal = 1, Confidential = 2, Secret = 3 }
pub trait Classified { fn classification(&self) -> Classification; }

pub struct Redacted⟨T⟩(T);
impl⟨T⟩ Redacted⟨T⟩ {
    pub fn new(inner: T) -> Self;
    pub fn get(&self) -> &T;
    pub fn into_inner(self) -> T;
}
impl⟨T⟩ std::fmt::Debug for Redacted⟨T⟩;    // prints [REDACTED]

pub struct Secret⟨T⟩(T);
impl⟨T⟩ Secret⟨T⟩ {
    pub fn new(inner: T) -> Self;
    pub fn expose(&self) -> &T;             // the only access path
}
impl⟨T⟩ std::fmt::Debug for Secret⟨T⟩;      // prints [REDACTED]
impl⟨T⟩ std::fmt::Display for Secret⟨T⟩;    // prints [REDACTED]

// crates/observability/src/lib.rs
pub mod classification;
pub use classification::{Classification, Classified, Redacted, Secret};

#[derive(Clone, Debug, Default)]
pub struct KernelFields {
    pub correlation_id: Option⟨String⟩,
    pub run_id: Option⟨String⟩,
    pub task_id: Option⟨String⟩,
    pub effect_id: Option⟨String⟩,
}
pub fn init_tracing(json: bool) -> Result<(), Box<dyn std::error::Error>>;
pub fn kernel_span(fields: &KernelFields) -> tracing::Span;
```

### Normative limits (specs/limits.yaml)

Flat, machine-readable keys; validator requires every key present and numeric. Exact values:

```yaml
schema_version: 1
queue:
  capacity_messages: 1024
adapters:
  max_frame_bytes: 4194304
  handshake_timeout_ms: 5000
  request_deadline_ms: 60000
  health_ping_ms: 10000
  health_missed_allowed: 3
  supervisor_max_restarts: 5
  supervisor_backoff_initial_ms: 100
  supervisor_backoff_max_ms: 10000
effects:
  lease_ms: 30000
  lease_renew_ms: 10000
runs:
  claim_ttl_ms: 30000
daemon:
  fence_lease_ms: 15000
  fence_renew_ms: 5000
approvals:
  ttl_ms: 900000
shutdown:
  drain_deadline_ms: 30000
events:
  live_buffer_events: 256
  dispatcher_poll_ms: 500
  scheduler_poll_ms: 1000
streams:
  read_default_limit: 100
  read_max_limit: 1000
fs:
  db_file_mode: "0600"
  runtime_dir_mode: "0700"
  control_socket_mode: "0600"
```

| Interface | Produced by | Consumed by | Serves |
|---|---|---|---|
| `KernelError`, `ErrorCode`, `RetryClass` | errors crate (FND-004) | every later module | R16, N1 |
| `uuid_newtype!` IDs, `UuidV7`, `EventCursor` | domain crate (FND-003) | every later module | R4, R15 |
| Mirror enums with `from_wire`/`to_wire` | domain crate (FND-003) | later modules converting wire values | R15, P1 |
| `Clock`, `IdProvider`, `FaultInjector` traits | domain crate (FND-003) | production crates needing injection | R17, N2 |
| `SystemClock`, `SystemIdProvider`, `NoFaults` | domain crate (FND-003) | agentd and later production wiring | R17 |
| `TestClock`, `DeterministicIds`, `ArmedFaults`, `TempDaemonHost` | testkit crate (FND-005) | every later test suite | R17, N2, P3 |
| `Classification`, `Redacted`, `Secret`, `init_tracing` | observability crate (FND-006) | every later module | R18, N1 |
| Generated protobuf types + `contract` module | domain build script (FND-002) | every crate converting wire values | R1, R2, R14, N4 |
| `limits.yaml` keys | normative limits file (G5) | validator, config schema, tests | R10 |
| `EventCursor` wire form and subscription frames | `contracts/control-api/mvp_control.proto` (G1) | control-api and events modules | R4.5, C5 |

## Data model

Everything below modifies the inception schema `specs/kernel-store-schema.sql`. No migration
lifecycle exists (D-003, R5); the schema is executed verbatim when `kernel.db` is created.

**New tables**

| Table | Key fields | Constraints |
|---|---|---|
| `artifacts` | `artifact_id` PK, `uri` UNIQUE, `digest`, `media_type`, `size_bytes`, `origin_run_id`, `origin_effect_id`, `sensitivity`, `retention`, `locator`, `created_at_ms` | FKs to runs/effects; CHECK on sensitivity/retention ranges |
| `workspaces` | `workspace_id` PK, `kind`, `base_revision`, `parent_workspace_id`, `created_at_ms` | FK to itself for fork lineage |
| `loop_turns` | `turn_id` PK, `run_id`, `run_revision`, `loop_epoch`, `step_sequence`, `input_event_cursor`, `state`, `issued_at_ms` | FK to runs; UNIQUE `(run_id, turn_id)`; CHECK state |
| `decisions` | `decision_id` PK, `run_id`, `turn_id`, `decision_type`, `decision_digest`, `decision_bytes`, `run_revision`, `loop_epoch`, `step_sequence`, `input_event_cursor`, `accepted_at_ms` | FKs to runs/loop_turns; UNIQUE `(run_id, decision_id)`; CHECK decision_type |
| `adapter_instances` | `adapter_instance_id` PK, `adapter_id`, `adapter_version`, `bundle_digest`, `daemon_instance_id`, `pid`, `process_start_identity`, `state`, `exit_reason`, `last_heartbeat_ms`, `started_at_ms`, `ended_at_ms` | FK to `adapter_registrations`; CHECK state |
| `conformance_reports` | PK `(adapter_id, adapter_version, bundle_digest)`, `report_digest`, `harness_version`, `result`, `run_at_ms`, `details` | FK to `adapter_registrations`; CHECK result |

**New columns on existing tables**

| Table | Column | Type | Null | Purpose |
|---|---|---|---|---|
| `runs` | `output_ref` | TEXT | yes | persists `Complete` decision output (C10) |
| `runs` | `current_turn_id` | TEXT | yes | outstanding loop turn reference |
| `runs` | `claim_daemon_epoch` | INTEGER | yes | fencing epoch that issued the claim (B3) |
| `effects` | `daemon_fencing_epoch` | INTEGER | yes | epoch that issued the executor lease (B3) |
| `timers` | `claim_daemon_epoch` | INTEGER | yes | epoch that issued the timer claim (B3) |
| `kernel_meta` | seeded row `schema_version` | BLOB | no | insert `"1"` at schema creation (B5) |

**CHECK constraints added** on: `runs.state`, `runs.recovery_disposition`, `effects.state`,
`effects.effect_class`, `effects.idempotency_semantics`, `effects.reconciliation_semantics`,
`workspace_leases.mode`, `workspace_leases.enforcement_state`, `outbox_events.sensitivity`,
`outbox_events.retention`, `adapter_registrations.trust_state`,
`adapter_registrations.conformance_state`, `config_generations.validation_state`,
`config_generations.test_state`, `run_dependencies.dependency_condition` (R6.1).

**Immutability triggers** blocking UPDATE and DELETE on: `resolved_run_environments`,
`resolved_bindings`, stored `agent_specs` revision rows, `approval_requests`,
`conformance_reports`; `outbox_events` allows only publication-metadata columns to change.

**PRAGMAs** in the schema header, now normative (R6.3): `foreign_keys=ON`,
`journal_mode=WAL`, `synchronous=FULL`, `busy_timeout=5000`. File modes come from
`specs/limits.yaml`.

**Migration required:** no.
**Backward compatible:** n/a — greenfield, inception schema only.

## Error handling

| Failure mode | Detection | Response | Surfaced as | Serves |
|---|---|---|---|---|
| Duplicate protobuf symbol | protoc/prost-build exits non-zero | build fails with file and line; validator also fails | `cargo check` error output | R1.2 |
| Snapshot changed without lock | validator hash comparison | validator exits 2 naming the file | CI failure | R1.4, N3 |
| Command type without payload message | registry lookup at decode time | reject, no mutation | `InvalidArgument` with code `unknown_command_type` | R2.2 |
| Unknown enum value from wire | `from_wire` conversion error | fail safely; no default substitution | `InvalidArgument` (or `Internal` for persisted corruption) | R15.1, P1 |
| Missing or non-numeric normative limit | validator parses `limits.yaml` | validator exits 2 naming the key | CI failure | R10.2 |
| Out-of-domain state persisted | SQLite CHECK on write | storage rejects the write | `Internal` with constraint detail | R6.1 |
| Module root owned by zero or many tasks | validator ownership computation | validator exits 2 with the path and owners | CI failure | R11.4, R12.1 |
| Manifest count or hash drift | validator hashes every listed file | validator exits 2 | CI failure | R12.2, C6 |
| Armed fault never triggered | `ArmedFaults::assert_triggered` or Drop check | test panics naming the point | test failure | R17.2, P3 |
| Malformed identifier or cursor | `FromStr` validation | reject with stable invalid-identifier error | `InvalidArgument` | R4.2 |

**Error taxonomy:** `crates/errors` is new; it reuses the pack's error vocabulary at
`agent-os-microkernel-mvp-buildpack/specs/error-model.md` and the stable codes defined there,
mapped onto `ErrorCode` and `RetryClass`. No `unwrap()` or `expect()` outside proven startup
invariants and test code (`AGENTS.md` prohibited shortcuts).

## Security considerations

| Concern | Treatment |
|---|---|
| Authentication / authorisation | n/a in this scope — foundation adds no network surface; peer-credential principal derivation is a later control-api requirement already recorded in `tasks/API-001.md` and restored to `dag.yaml` by gap closure |
| Input validation & injection | proto decoding is schema-bound and fails closed; IDs and cursors validated on parse; no string-built SQL anywhere in foundation |
| Secrets handling | `observability::Secret⟨T⟩` and `Redacted⟨T⟩` render as `[REDACTED]` through Debug and Display; `Secret::expose` is the only access path; no secret type gets a derived Debug (R18.2, N1) |
| Data exposure in logs / errors | `Classification` gates emission; `KernelError` messages carry codes, never payloads; the sink redacts above its threshold (R18.3) |
| New network surface | none — no sockets, listeners, or HTTP in foundation; agentd placeholder exits before opening the Control API socket (R13.2) |
| Dependency additions (pinned versions) | exactly the workspace set named by FND-001: tokio, tracing, serde, prost, tonic, uuid, sha2, sqlx, thiserror, async-trait, tempfile, proptest, protoc-bin-vendored; caret ranges in `Cargo.toml` with `Cargo.lock` committed; `protoc-bin-vendored` removes any system-protoc dependency (R14.1) |

## Test strategy

| Level | Framework | Location | Covers |
|---|---|---|---|
| Unit | `cargo test` in-crate | `crates/errors/tests/`, `crates/domain/src/**` `#[cfg(test)]`, `crates/observability/tests/` | R15.1, R15.2, R16.1, R18.2, R18.3 |
| Integration | `cargo test` integration targets | `crates/domain/tests/contract_codegen.rs`, `crates/testkit/tests/daemon_host.rs` | R14.1, R14.2, R13.2 |
| Property | `proptest` | `crates/domain/tests/prop_enums.rs`, `crates/testkit/tests/prop_faults.rs` | P1, P3 |
| Quality gates | cargo + CI | `.github/workflows/ci.yml` | R13.1, R13.3, N2 |
| Validator | Python + shell | `scripts/validate_buildpack.py`, `scripts/test_validator_mutations.sh` | R1.2, R1.4, R10.2, R11.*, R12.*, G1, G2 |
| Regression | Python validators | `tools/validate_repo.py`, validator equality checks | G1, G2 |

**Property tests**

| Property | Statement | Generator strategy |
|---|---|---|
| P1 | for any 32-bit integer standing in for an enum wire value, `from_wire` either returns a valid variant or an error; it never returns a variant different from the wire meaning | arbitrary `i32` per mirror enum, including negative and large values |
| P3 | for any arm/trigger interleaving, each armed point that is triggered fires exactly once | arbitrary sequences of arm, trigger, and re-arm over a small point set |
| P2 | for any crate set produced by the workspace manifest, the dependency graph is acyclic | `cargo metadata` parsed in CI; Rust and Cargo already reject cycles at build time, so this is a validator check rather than a generative test |

**Explicitly not tested (and why):**

- Runtime behaviour of the other 26 crates — out of scope here; each later module brings its
  own tests.
- macOS Keychain, sandbox tiers, and remote access — later modules, intentionally deferred by
  `SCOPE.md:41-45`.
- N2 ("no wall-clock sleep") is enforced by a CI grep over test targets plus review; a grep is
  coarse, so it is a guard, not a proof.

## Observability

| Signal | Where | Content |
|---|---|---|
| `kernel.span` | `observability::kernel_span` used by every later module | correlation id plus run, task, and effect ids when present |
| Structured log records | `init_tracing(json)` sink | classification, stable code, message; payloads redacted above the sink threshold |
| Codegen diagnostics | `domain/build.rs` on failure | proto file path and line from prost/protoc |
| Validator output | `validate_buildpack.py` | names the exact file, key, path, or hash that failed |

## Performance

| Requirement | Design mechanism | How it is measured |
|---|---|---|
| N1 (security) | `Secret`/`Redacted` types plus `Classification` gating | redaction unit tests; Debug and Display assertions |
| N2 (testability) | injected `FaultInjector`; `TestClock`; no global test state | testkit tests plus CI grep for sleep calls in test code |
| N3 (compatibility) | contract lock enforced in the validator; generated types rebuilt from the snapshot on every build | validator run in CI on every change |
| N4 (reproducibility) | vendored protoc, pinned prost/tonic versions, committed lockfile, OUT_DIR generation | `contract_codegen` test generates twice and compares bytes |

Runtime latency targets do not exist in this scope; the repository has no such requirements
yet, and inventing them here would be unmeasurable.

## Design decisions

| # | Decision | Alternatives rejected | Rationale | Serves |
|---|---|---|---|---|
| D1 | Delete `control.proto` from the pack snapshot; `mvp_control.proto` is the single normative Control API | keeping both under different packages; renaming symbols | Both files define the same six messages in one package and cannot compile; the MVP surface is the nine-RPC proto the spec already matches | R1.3 |
| D2 | `command_type` is the fully-qualified protobuf message name of the command payload | kebab-case string constants; numeric enum | One authority: the descriptor registry is derived from generated code, so a new command cannot exist without a message | R2.1, R2.4 |
| D3 | Code generation runs in `domain/build.rs` into OUT_DIR with vendored protoc | checked-in generated files; a dedicated proto crate | No drift surface, no extra crate, hermetic builds; freshness is the contract lock's job | R14, N4 |
| D4 | `Clock`, `IdProvider`, and `FaultInjector` traits live in domain; testkit implements them | traits in testkit; global test statics | Production crates must not depend on testkit, and global state breaks parallel tests | R17, N2 |
| D5 | Storage-level CHECK constraints and triggers in addition to application pre-checks | application checks only; triggers only | The pack's own acceptance criterion requires constraint-backed invariants, and triggers alone give poor diagnostics | R6.1, R5.4 |
| D6 | Every threshold lives in `specs/limits.yaml`, mirrored by the config schema and default config | inline values in tests; schema-only defaults | One edit moves every test and default; the validator can fail on a missing key | R10 |
| D7 | Unmapped recovery combinations fail closed at startup | default to `REQUIRES_HUMAN_DECISION` | A silent default hides a modelling gap; a human decision is a state, not a fallback (user-approved) | R7.2 |
| D8 | Remove `RunPaused`/`RunResumed` from the MVP catalog | defining a suspend command now | No transition produces them; adding a command is scope creep (user-approved) | R9.3 |
| D9 | Add `RollbackConfigGeneration` and `ResolveBlockedRun` commands | leaving both gaps open | The capability grant and event already exist for rollback; blocked runs otherwise have no exit (user-approved) | R7.3, R8.2 |
| D10 | `sqlx` is the store driver; foundation declares it but opens no database | `rusqlite`; opening the DB in foundation | User choice; PST-002/PST-003 solve `BEGIN IMMEDIATE` via a managed raw-SQL transaction wrapper | R13 |
| D11 | Rust edition 2024, toolchain pinned to 1.94.0, macOS-only CI | floating toolchain; Linux matrix | Matches the verified local toolchain and assumption 8; reproducibility first | R13, N4 |
| D12 | `dag.json`, `DAG.md`, and `tasks.csv` are generated by `sync_dag_sources.py`; the validator enforces equality | hand-maintained copies; validator checks only | Six sources in exact agreement already; generation removes the drift class instead of policing it | R11.1, G2 |
| D13 | FND-001 pre-declares every crate root and the agentd module files as empty modules | adding dependency edges between every pair of tasks in a shared crate | Removes 38 concurrent same-crate pairs without serializing the schedule; edges remain the fallback | R11.3, R11.4 |
| D14 | Gap-closure work lives only in the spec-tracked graph, not in `dag.yaml` | adding tasks to the pack DAG | Keeps the 59-task pack graph and its status file intact; spec-flow owns orchestration | R11.1, G2 |
| D15 | Subscription frames carry lag notices; cursors are `v1:{stream_key}:{sequence}` | `after_sequence` alone with no lag channel | A cursor must be resumable after a retention gap; the current proto cannot express one | R4.5, C5 |
| D16 | `run_graph_heads` rows are created in the same transaction as their task | first child spawn inserts on demand | Closes the missing-insert ambiguity the audit found; no runner-up design was safe under concurrent spawn | R5.1 |

## Requirements traceability

| Requirement | Covered by | Verified by |
|---|---|---|
| R1 | D1, contract snapshot edits, lock regeneration | validator duplicate/lock checks; `cargo check` |
| R2 | D2, `commands.proto`, typed decisions | registry unit tests; decoder tests |
| R3 | digest specification in `types-and-ids.md` | cross-implementation digest fixture tests |
| R4 | D15, domain IDs and cursor types | `FromStr`/round-trip unit tests |
| R5 | schema additions, D16 | validator schema-coverage check; DB constraint tests |
| R6 | D5, PRAGMAs, triggers | SQLite CHECK/trigger tests |
| R7 | D7, complete recovery table | exhaustive matrix test |
| R8 | D9, command catalog edits | validator catalog check |
| R9 | D8, D15, event catalog edits | validator event-catalog check |
| R10 | D6, limits file and schema | validator limits check |
| R11 | D12, D13, D14 | validator ownership and equality checks |
| R12 | validator extensions | mutation test script |
| R13 | workspace and CI files | quality commands in CI |
| R14 | D3 | `contract_codegen` determinism test |
| R15 | mirror enums and IDs | P1, round-trip unit tests |
| R16 | errors crate | code/retry mapping unit tests |
| R17 | testkit, D4 | P3, testkit unit tests |
| R18 | observability crate | redaction and classification tests |
| N1 | `Secret`/`Redacted`, classification | redaction tests |
| N2 | injected fault points | testkit tests, CI grep |
| N3 | contract lock | validator lock check in CI |
| N4 | D3, pinned deps | determinism test |
| P1 | mirror enum conversion | `prop_enums.rs` |
| P2 | workspace manifest | `cargo metadata` check in CI |
| P3 | `ArmedFaults` | `prop_faults.rs` |
| G1 | no edits to the `docs/` tree | `tools/validate_repo.py` in CI |
| G2 | D12, D14 | validator task-id and edge checks |

## File structure

Gap-closure files (paths relative to `agent-os-microkernel-mvp-buildpack/`):

| Path | Create or modify | Responsibility |
|---|---|---|
| `contracts/control-api/control.proto` | modify (delete) | remove the colliding duplicate service |
| `contracts/control-api/commands.proto` | create | one payload message per catalogued command; command type naming rule |
| `contracts/control-api/mvp_control.proto` | modify | subscription frames and lag notice (D15) |
| `contracts/protocols/agent_loop.proto` | modify | typed loop decisions for the six variants |
| `contracts/domain/core.proto` | modify | `output_ref` and `current_turn_id` on `AgentRun` |
| `contracts/events/catalog.yaml` | modify | complete event list with version, classification, retention, stream |
| `contracts/config/agent-os.schema.json` | modify | typed `limits` object replacing the bare `policies` catch-all |
| `contracts/README.md` | modify | snapshot inventory and authority note |
| `contract-lock.sha256` | modify | regenerated hashes |
| `specs/kernel-store-schema.sql` | modify | new tables, columns, CHECKs, triggers, meta seed, PRAGMAs |
| `specs/event-journal-schema.sql` | create | extracted event-journal DDL (C8) |
| `specs/limits.yaml` | create | single normative threshold source |
| `specs/recovery-table.md` | modify | complete run-state by effect-state matrix |
| `specs/command-catalog.md` | modify | add rollback and blocked-run resolution; payload message names |
| `specs/command-coordinator.md` | modify | align the coordinator command list with the catalog |
| `specs/event-catalog.md` | modify | align with `catalog.yaml`; remove paused/resumed |
| `specs/types-and-ids.md` | modify | encodings, full newtype list, cursor format |
| `specs/kernel-store.md` | modify | repository and table inventory |
| `specs/run-graph.md` | modify | head-row creation rule (D16) |
| `specs/config-engine.md` | modify | limits section and rollback command |
| `specs/artifacts.md` | modify | metadata to durable-record mapping |
| `specs/workspace.md` | modify | workspace identity and fork lineage |
| `specs/process-supervisor.md` | modify | durable adapter-instance record |
| `specs/adapter-registry.md` | modify | conformance-report binding |
| `specs/README.md` | modify | index new files |
| `architecture/persistence.md` | modify | PRAGMAs become normative |
| `examples/default-config.yaml` | modify | limits values |
| `dag.yaml` | modify | task metadata fixes, concrete file lists, ownership edges |
| `dag.json` | modify | generated from `dag.yaml` |
| `DAG.md` | modify | generated from `dag.yaml` |
| `tasks.csv` | modify | generated from `dag.yaml` |
| `tasks/FND-001.md` | modify | module-root outputs |
| `tasks/FND-002.md` | modify | codegen outputs and paths |
| `tasks/API-001.md` | modify | restore peer-principal test (C4) |
| `tasks/RUN-002.md` | modify | correct goal wording (C4) |
| `tasks/CFG-002.md` | modify | test list order (C4) |
| `scripts/validate_buildpack.py` | modify | new checks and `--update-lock` / `--write-manifest` |
| `scripts/sync_dag_sources.py` | create | generate derived DAG sources |
| `scripts/test_validator_mutations.sh` | create | negative tests for the validator |
| `MANIFEST.json` | modify | regenerated |
| `SOURCE_CORRECTIONS.md` | modify | document proto changes |

Foundation files (paths relative to `agent-os/`):

| Path | Create or modify | Responsibility |
|---|---|---|
| `Cargo.toml` | create | workspace members, shared dependency policy |
| `Cargo.lock` | create | committed resolution |
| `rust-toolchain.toml` | create | pin 1.94.0 plus rustfmt and clippy |
| `.cargo/config.toml` | create | cargo aliases used by CI |
| `.github/workflows/ci.yml` | create | quality gates, validators, lock, determinism, hygiene grep |
| `proto/` | create | mirror of the contract snapshot (task enumerates each file) |
| `schema/kernel_store.sql`, `schema/event_journal.sql` | create | installed inception schemas |
| `crates/{crate}/Cargo.toml` and `crates/{crate}/src/lib.rs` | create | one pair per crate in the crate map; agentd uses `src/main.rs` |
| `crates/domain/build.rs` | create | vendored protoc plus prost generation into OUT_DIR |
| `crates/domain/src/generated.rs` | create | include and re-export generated types |
| `crates/domain/src/ids.rs` | create | newtypes, `UuidV7`, keys, cursor |
| `crates/domain/src/provider.rs` | create | `IdProvider` trait and system implementation |
| `crates/domain/src/time.rs` | create | `Clock` trait and system implementation |
| `crates/domain/src/faults.rs` | create | `FaultInjector` trait and no-op implementation |
| `crates/domain/src/run.rs`, `effect.rs`, `security.rs`, `resource.rs` | create | mirror enums and immutable value types |
| `crates/domain/tests/contract_codegen.rs` | create | determinism and freshness tests |
| `crates/domain/tests/prop_enums.rs` | create | P1 |
| `crates/errors/src/codes.rs`, `lib.rs` | create | codes, retry class, `KernelError` |
| `crates/errors/tests/codes.rs` | create | code and retry mapping |
| `crates/testkit/src/clock.rs`, `ids.rs`, `faults.rs`, `process.rs`, `lib.rs` | create | deterministic test doubles |
| `crates/testkit/tests/daemon_host.rs` | create | placeholder agentd smoke test |
| `crates/testkit/tests/prop_faults.rs` | create | P3 |
| `crates/observability/src/classification.rs`, `lib.rs` | create | classification, redaction, tracing init |
| `crates/observability/tests/redaction.rs` | create | N1, R18.2, R18.3 |
| `crates/agentd/src/main.rs` | create | placeholder composition root |
| `crates/agentd/src/lock.rs`, `recovery.rs`, `api.rs`, `workers/mod.rs`, `workers/outbox.rs`, `workers/scheduler.rs`, `workers/loops.rs` | create | empty pre-declared module roots (D13) |

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (recorded from explicit chat instruction)
**Date:** 2026-09-11
