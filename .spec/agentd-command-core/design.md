# Design — agentd-command-core

**Status:** draft
**Date:** 2026-09-11
**Requirements:** `requirements.md` (approved)

## Architecture

```
                    ┌──────────────────────────┐
   caller ────────▶ │   CommandCoordinator     │  lib.rs
                    │  validate → begin_write  │
                    │  replay → handler → out  │
                    │  commit → fault points   │
                    └───────┬──────────┬───────┘
                            │          │
              ┌─────────────▼──┐   ┌───▼──────────────────┐
              │ CommandRegistry│   │ KernelStore (port)   │
              │ type → handler │   │ fenced KernelTxn     │
              └───────┬────────┘   └───┬──────────────────┘
                      │                │
              ┌───────▼────────┐  ┌────▼─────────────────┐
              │ CommandHandler │  │ persistence module   │
              │ ctx + txn only │  │ idempotency, streams │
              └────────────────┘  └──────────────────────┘
```

| Component | Responsibility | New or existing | Path |
|---|---|---|---|
| Envelope | Validated request wrapper and digest type | new | `agent-os/crates/command-coordinator/src/envelope.rs` |
| Handler | Context, outcome, handler trait, registry | new | `agent-os/crates/command-coordinator/src/handler.rs` |
| Coordinator | Execute algorithm, replay branch, fault points | new | `agent-os/crates/command-coordinator/src/lib.rs` |
| Fault seam | Additive `inject` on the foundation fault trait | modify | `agent-os/crates/domain/src/faults.rs`, `agent-os/crates/testkit/src/faults.rs` |

## Data flow

**Happy path — fresh command**

1. `execute(envelope)` resolves the handler from the registry by `command_type`; unknown types reject with `InvalidArgument` before any transaction.
2. `envelope.validate(clock.now_unix_ms())` checks key bounds, digest format, ids, and deadline.
3. The coordinator builds `TxContext { daemon_epoch: fence.epoch(), principal_id, command_id, correlation_id }` and calls `store.begin_write` — the persistence guard asserts the live fence.
4. `txn.idempotency().lookup(principal, key)` returns none, so the handler runs with `CommandContext` and `&mut dyn KernelTxn` only.
5. The handler applies mutations and stages events through `events::outbox::stage`, which writes stream-head and outbox rows in the same transaction.
6. The fault point `command.before_commit` is consulted; unarmed, it is a no-op.
7. The coordinator inserts the idempotency record with the outcome code and payload and calls `commit`.
8. The fault point `command.after_commit` is consulted; unarmed, the outcome returns to the caller.

**Replay path**

1. Validation and `begin_write` run exactly as above.
2. `lookup` returns a record: same digest returns the stored code and payload after dropping the transaction (rollback of an otherwise empty write); different digest returns `Conflict`.
3. No handler runs and no row changes.

**Failure path — pre-commit fault**

1. The handler has staged mutations and outbox rows.
2. `command.before_commit` is armed; the coordinator returns `Unavailable` (retry-safe) and drops the transaction.
3. `Drop` rolls back; zero canonical rows, zero outbox rows, zero idempotency rows remain.

**Failure path — post-commit fault**

1. The transaction commits with mutations, outbox rows, and the idempotency record.
2. `command.after_commit` is armed; the coordinator returns `Unavailable` although the work committed.
3. A later identical replay finds the stored outcome and returns it with unchanged row counts.

## Interfaces

### envelope.rs

```rust
pub struct RequestDigest([u8; 32]);
impl std::str::FromStr for RequestDigest { type Err = errors::KernelError; }
impl std::fmt::Display for RequestDigest;   // lowercase hex

pub struct CommandEnvelope {
    pub command_id: domain::ids::CommandId,
    pub idempotency_key: domain::ids::IdempotencyKey,
    pub principal_id: domain::ids::PrincipalId,
    pub actor_id: domain::ids::ActorId,
    pub device_id: Option⟨domain::ids::DeviceId⟩,
    pub delegation_chain_id: Option⟨domain::ids::DelegationChainId⟩,
    pub request_digest: RequestDigest,
    pub correlation_id: Option⟨String⟩,
    pub causation_id: Option⟨String⟩,
    pub deadline_unix_ms: Option⟨i64⟩,
    pub command_type: String,
    pub payload: Vec⟨u8⟩,
}

impl CommandEnvelope {
    /// Rejects empty keys, malformed digests, empty command types, and past deadlines.
    pub fn validate(&self, now_unix_ms: i64) -> errors::Result⟨()⟩;
}
```

### handler.rs

```rust
pub struct CommandContext {
    pub command_id: domain::ids::CommandId,
    pub principal_id: domain::ids::PrincipalId,
    pub actor_id: domain::ids::ActorId,
    pub device_id: Option⟨domain::ids::DeviceId⟩,
    pub delegation_chain_id: Option⟨domain::ids::DelegationChainId⟩,
    pub correlation_id: Option⟨String⟩,
    pub causation_id: Option⟨String⟩,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutcomeCode { Ok }
impl OutcomeCode {
    pub const fn as_str(self) -> &'static str;          // "ok"
    pub fn from_stored(value: &str) -> errors::Result⟨Self⟩;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandOutcome {
    pub code: OutcomeCode,
    pub payload: Vec⟨u8⟩,
}

#[async_trait::async_trait]
pub trait CommandHandler: Send + Sync + 'static {
    async fn handle(
        &self,
        ctx: &CommandContext,
        txn: &mut dyn kernel_store::KernelTxn,
        payload: Vec⟨u8⟩,
    ) -> errors::Result⟨CommandOutcome⟩;
}

#[derive(Default)]
pub struct CommandRegistry { /* HashMap⟨String, Arc⟨dyn CommandHandler⟩⟩ */ }
impl CommandRegistry {
    pub fn new() -> Self;
    /// Returns `Conflict` when the command type is already registered.
    pub fn register(&mut self, command_type: impl Into⟨String⟩,
                    handler: std::sync::Arc⟨dyn CommandHandler⟩) -> errors::Result⟨()⟩;
    pub fn get(&self, command_type: &str) -> Option⟨std::sync::Arc⟨dyn CommandHandler⟩⟩;
    pub fn len(&self) -> usize;
}
```

### lib.rs

```rust
pub const BEFORE_COMMIT: &str = "command.before_commit";
pub const AFTER_COMMIT: &str = "command.after_commit";

pub trait FenceProvider: Send + Sync {
    fn epoch(&self) -> u64;
}
pub struct FixedFence(pub u64);
impl FenceProvider for FixedFence;

pub struct CommandCoordinator { /* store, registry, fence, clock, faults */ }
impl CommandCoordinator {
    pub fn new(
        store: std::sync::Arc⟨dyn kernel_store::KernelStore⟩,
        registry: std::sync::Arc⟨CommandRegistry⟩,
        fence: std::sync::Arc⟨dyn FenceProvider⟩,
        clock: std::sync::Arc⟨dyn domain::time::Clock⟩,
        faults: std::sync::Arc⟨dyn domain::faults::FaultInjector⟩,
    ) -> Self;

    pub async fn execute(&self, envelope: CommandEnvelope)
        -> errors::Result⟨CommandOutcome⟩;
}
```

### Fault seam addition (CMD-001A)

```rust
// crates/domain/src/faults.rs — additive, defaulted
pub trait FaultInjector: Send + Sync + 'static {
    fn trigger(&self, point: &str);
    /// Fires `point` once if armed and reports whether it fired. Default: never fires.
    fn inject(&self, point: &str) -> bool { let _ = point; false }
}
// crates/testkit/src/faults.rs — ArmedFaults::inject marks triggered and returns true once per arm.
```

| Interface | Produced by | Consumed by | Serves |
|---|---|---|---|
| `CommandEnvelope`, `RequestDigest` | CMD-001B | control-api module, later handlers | R2 |
| `CommandHandler`, `CommandRegistry`, `OutcomeCode` | CMD-001B | every later command module | R1, R3 |
| `CommandCoordinator::execute` | CMD-001B | control-api module and daemon composition | R1, R4 |
| `FaultInjector::inject` | CMD-001A | coordinator and later fault-injected modules | R4, N2 |
| `FenceProvider` | CMD-001B | daemon composition (identity fence) | R2.5 |

## Data model

No schema changes. The coordinator reads and writes:

| Use | Table | Rule |
|---|---|---|
| Idempotency lookup and insert | `idempotency_records` | unique `(principal_id, idempotency_key)`; insert only on success, inside the transaction |
| Event staging | `event_stream_heads`, `outbox_events` | staged by handlers through `events::outbox::stage`; immutable rows, publication metadata later |

**Migration required:** no. **Backward compatible:** n/a.

## Error handling

| Failure mode | Detection | Response | Serves |
|---|---|---|---|
| Malformed envelope (key, digest, ids) | `validate` before transaction | `InvalidArgument`, `Never` | R2.1, R2.2 |
| Past deadline | `validate` with the injected clock | `FailedPrecondition`, `Never` | R2.3 |
| Unknown command type | registry lookup | `InvalidArgument`, `Never`, no transaction | R1.4 |
| Duplicate registration | registry insert | `Conflict` at registration | R3.2 |
| Digest mismatch on replay | stored digest comparison | `Conflict`, `Never`, no mutation | R1.3 |
| Stale or absent fence | persistence `begin_write` assertion | `FailedPrecondition`, `Never` | R2.4 |
| Handler error | handler result | transaction dropped; mapped error returned; nothing recorded | R3, R4.1 |
| Pre-commit fault | `inject(BEFORE_COMMIT)` | `Unavailable`, `Safe`, rollback | R4.1 |
| Post-commit fault | `inject(AFTER_COMMIT)` | `Unavailable`, `Safe`, committed state retained | R4.2 |

**Error taxonomy:** `agent-os/crates/errors` (`KernelError`, `ErrorCode`, `RetryClass`); no new
codes. Error messages never include payload bytes (N1).

## Security considerations

| Concern | Treatment |
|---|---|
| Authentication / authorisation | out of scope here; the context carries principal/actor for later handlers; transport authentication is the control-api module's requirement |
| Input validation and injection | envelope validation before any transaction; no SQL in this crate |
| Secrets handling | no secrets handled; payloads are opaque bytes |
| Data exposure in logs / errors | faults and validation errors name points and fields only; never payloads (N1) |
| New network surface | none |
| Dependency additions | `async-trait`; dev-only `tokio`, `testkit`, `tempfile`, `kernel-store-sqlite`; all existing workspace dependencies (CMD-000) |

## Test strategy

| Level | Framework | Location | Covers |
|---|---|---|---|
| Integration | `cargo test` with a real SQLite store in a temp root | `crates/command-coordinator/tests/coordinator.rs` | R1.1–R1.5, R2.1–R2.5, R4.1–R4.4, P1, P2 |
| Unit | in-crate tests | `src/envelope.rs`, `src/handler.rs` | digest parsing, validation predicates, duplicate registration, outcome codes |
| Fault-seam tests | testkit integration tests | `crates/testkit/tests/prop_faults.rs` | `inject` once-per-arm, default no-op |
| Regression | workspace gates and pack validators | repo root and `agent-os/` | G1, G2, N3 |

**Property tests**

| Property | Statement | Generator strategy |
|---|---|---|
| P1 | any number of identical replays returns the stored outcome with unchanged canonical row counts | fixed command submitted N times with assertions between calls |
| P2 | any pre-commit failure leaves zero attributable rows | arm the fault point before commit for distinct commands and assert row counts |

**Explicitly not tested (and why):**

- Real command handlers and their authorization — later modules.
- Process-level crash recovery — verification module.
- Digest recomputation — foundation spec; the coordinator validates format only.

## Observability

No new signals in this module: the persistence ruling deferred store-level instrumentation, and
the coordinator depends on no observability crate. Fault-point usage is observable through the
testkit injector, and errors carry stable codes.

## Performance

| Requirement | Design mechanism | How it is measured |
|---|---|---|
| N1 | validation before transactions; no payload echoing | unit tests assert error content |
| N2 | once-per-arm injector with no sleeps | testkit tests |
| N3 | no contract or schema edits | pack validators |

## Design decisions

| # | Decision | Alternatives rejected | Rationale | Serves |
|---|---|---|---|---|
| D1 | Type-erased registry keyed by command type string | generic `execute` per command; closed enum | runtime dispatch over the wire string; later modules add handlers without editing the coordinator | R1.4, R3.2 |
| D2 | Handlers receive raw payload bytes and decode their own typed message | coordinator decodes via a prost registry | simplest erasure; each handler owns its payload type | R3.1 |
| D3 | Success-only idempotency recording | record failures | replay of failures is unspecified; safe default | R1.5 |
| D4 | Unknown command type and malformed envelopes reject before any transaction | open then reject | no trace, cheaper, and consistent with R2 | R1.4, R2.2 |
| D5 | Fencing epoch comes from the fence provider, never the envelope | envelope-carried epoch | clients must not influence the fence | R2.5 |
| D6 | Replay returns the stored outcome after dropping the empty write transaction | read-only transaction variant | one code path; dropping a write transaction is a no-op rollback | R1.2 |
| D7 | Outcome vocabulary is `ok` plus bytes | rich codes now | nothing to code until the real commands exist | R1.1 |
| D8 | Fault seam is an additive defaulted `inject` method | coordinator-local trait; panics | non-breaking for every existing implementor; testkit keeps one seam | R4.3 |
| D9 | No span instrumentation in this module | add observability now | the persistence ruling deferred store instrumentation; no consumer yet | N3 |

## Requirements traceability

| Requirement | Covered by | Verified by |
|---|---|---|
| R1.1–R1.6 | `execute` algorithm, registry, idempotency repo | `tests/coordinator.rs` |
| R2.1–R2.5 | `validate`, fence provider, persistence assertion | unit tests, `tests/coordinator.rs` |
| R3.1–R3.4 | handler trait, registry, txn-only access | compile-time shape plus registry tests |
| R4.1–R4.4 | fault points, injector semantics | `tests/coordinator.rs`, `crates/testkit/tests/prop_faults.rs` |
| N1 | validation and error construction | unit tests |
| N2 | `ArmedFaults::inject` | testkit tests |
| N3 | no contract or schema edits | pack validators |
| P1, P2 | replay and pre-commit tests | `tests/coordinator.rs` |
| G1, G2 | quality gates and validators | repo root and `agent-os/` |

## File structure

Paths relative to `agent-os/`.

| Path | Create or modify | Responsibility | Owner task |
|---|---|---|---|
| `crates/command-coordinator/Cargo.toml` | modify | add `async-trait`; dev `tokio`, `testkit`, `tempfile`, `kernel-store-sqlite` | CMD-000 |
| `Cargo.lock` | modify | resolution update | CMD-000 |
| `crates/domain/src/faults.rs` | modify | additive `inject` with default | CMD-001A |
| `crates/testkit/src/faults.rs` | modify | `ArmedFaults::inject` once per arm | CMD-001A |
| `crates/testkit/tests/prop_faults.rs` | modify | inject semantics tests | CMD-001A |
| `crates/command-coordinator/src/envelope.rs` | create | envelope, digest, validation | CMD-001B |
| `crates/command-coordinator/src/handler.rs` | create | context, outcome, handler trait, registry | CMD-001B |
| `crates/command-coordinator/src/lib.rs` | modify | coordinator, fence provider, execute algorithm, fault points | CMD-001B |
| `crates/command-coordinator/tests/coordinator.rs` | create | integration suite with a real store and a test handler | CMD-001B |

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
