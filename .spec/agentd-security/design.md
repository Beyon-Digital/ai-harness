# Design — agentd-security

**Status:** draft
**Date:** 2026-09-11
**Requirements:** `requirements.md` (approved)

## Architecture

```
 runtime handlers ──▶ permissions::evaluate(request) ──▶ Allow | Deny | RequireApproval
        │                    ▲                                  │
        │              identity chain                          ▼
        │                    │                        approvals (digest-bound)
        ▼                    ▼                                  │
 kernel store (grants, hops, approvals) ◀────── coordinator command handlers
        │
        ▼
 secrets broker ──▶ SecretStore (in-memory test | macOS Keychain) ──▶ zeroizing value
```

| Component | Responsibility | New or existing | Path |
|---|---|---|---|
| Identity | Principal/actor/run context, hops, subset derivation, integrity | new fill | `agent-os/crates/identity/src/{lib,delegation}.rs` |
| Permissions | Capability families, scoped targets, one decision function | new fill | `agent-os/crates/permissions/src/{lib,capabilities,policy}.rs` |
| Approvals | Canonical digest, request/response commands, resolution | new fill | `agent-os/crates/approvals/src/lib.rs` |
| Secrets | Broker, in-memory backend, Keychain backend | new fill | `agent-os/crates/secrets/src/{lib,broker,keychain}.rs` |

## Data flow

**Happy path — authorization**

1. A component builds a `PermissionRequest` from the principal, actor, loaded chain, requested `Capability`, and `ScopedTarget`.
2. `evaluate` intersects the chain's grants, the tool's allowed capabilities, ancestor constraints, and system policy.
3. An in-scope grant yields `Allow { grant_refs }`; missing grants or scopes yield `Deny { reason }`.
4. Secret use plus broad egress upgrades the outcome to `RequireApproval { draft }`.

**Happy path — approval**

1. A blocked operation calls `create_request`, which canonicalizes the draft, computes the digest, inserts the immutable request, and stages `ApprovalRequested` in the command transaction.
2. The operator responds through `RespondApproval`; the handler validates id, digest, expiry, unresolved state, and responder identity, then persists exactly one terminal response.
3. The blocked command calls `is_satisfied` and proceeds only for an approved, unexpired, digest-matching response.

**Failure path — replay against changed content**

1. The underlying extension or config digest changes.
2. `is_satisfied` recomputes or compares the stored digest input and reports `Invalidated`; the prior approval no longer authorizes.

**Failure path — secret leak attempt**

1. A caller requests raw material with an unauthorized or joint-risk context.
2. The broker denies or requires approval and returns a stable error with no secret content.
3. Authorized raw access returns a zeroizing value whose `Debug` renders a redacted marker; an audit record carries ids and outcome only.

## Interfaces

### identity :: delegation.rs

```rust
pub struct Capability { pub family: CapabilityFamily, pub action: CapabilityAction }

pub struct Hop {
    pub hop_index: u32,
    pub actor_id: domain::ids::ActorId,
    pub run_id: Option⟨domain::ids::RunId⟩,
    pub grant_ids: Vec⟨domain::ids::CapabilityGrantId⟩,
    pub capabilities: Vec⟨Capability⟩,
}

pub struct DelegationChain {
    pub chain_id: domain::ids::DelegationChainId,
    pub hops: Vec⟨Hop⟩,
}

pub async fn persist_hop(txn: &mut dyn KernelTxn, chain: DelegationChainId, hop: Hop) -> Result⟨()⟩;
pub async fn load_chain(txn: &mut dyn KernelTxn, chain: DelegationChainId) -> Result⟨DelegationChain⟩;

/// Returns the capability subset persisted for the child hop; errors when the
/// requested set is not a subset of the parent's capabilities or grant ids.
pub fn derive_child_chain(
    parent: &DelegationChain,
    requested: &[Capability],
) -> Result⟨Vec⟨Capability⟩⟩;

pub fn validate_chain(chain: &DelegationChain) -> Result⟨()⟩;
```

### permissions :: policy.rs

```rust
pub enum CapabilityFamily { Workspace, Network, Secret, Agent, Extension, Config, Effect, Resource }
pub enum CapabilityAction {
    Read, Write, Fork, Merge, TransferExclusive, SharedCoordinated,
    Connect, Use, SignOrAct, Spawn, Install, Enable, Disable,
    Propose, Test, Activate, Rollback, Reconcile, ResolveUnknown, Reserve,
}

pub enum ScopedTarget {
    Workspace { uri: String, path: Option⟨String⟩ },
    Network { domains: Vec⟨String⟩ },
    Secret { uri: String, egress: Option⟨Vec⟨String⟩⟩ },
    Config { operation: CapabilityAction },
    ChildCount { max: u32 },
    None,
}

pub enum DenyReason {
    NoGrant, OutOfScope, AncestorConstraint, ToolRestriction, ExpiredGrant, Unsupported,
}

pub enum Decision {
    Allow { grant_refs: Vec⟨CapabilityGrantId⟩ },
    Deny { reason: DenyReason },
    RequireApproval { request: ApprovalDraft },
}

pub struct ApprovalDraft {
    pub operation: String,
    pub target: String,
    pub capabilities: Vec⟨Capability⟩,
    pub extension_digest: Option⟨String⟩,
    pub config_digest: Option⟨String⟩,
    pub expiry_ms: i64,
    pub nonce: String,
}

pub struct GrantScope {
    pub grant_id: CapabilityGrantId,
    pub capability: Capability,
    /// `None` means the grant is unscoped for this capability.
    pub scope: Option⟨ScopedTarget⟩,
    pub expires_at_ms: Option⟨i64⟩,
}

pub struct PermissionRequest {
    pub principal_id: PrincipalId,
    pub actor_id: ActorId,
    pub run_id: Option⟨RunId⟩,
    pub chain: DelegationChain,
    /// Loaded from `capability_grants` (grant id, capability, scope, expiry).
    pub grant_scopes: Vec⟨GrantScope⟩,
    pub tool_capabilities: Option⟨Vec⟨Capability⟩⟩,
    pub capability: Capability,
    pub target: ScopedTarget,
    pub extension_digest: Option⟨String⟩,
    pub config_digest: Option⟨String⟩,
    pub now_ms: i64,
}

/// Allow requires a chain grant whose capability matches AND whose scope covers
/// the target (unscoped grants allow any target) AND that has not expired.
/// `Allow { grant_refs }` cites exactly the backing grants.
/// `Deny { OutOfScope }` when a matching grant exists but no scope covers the target.
/// `Deny { ExpiredGrant }` when the only matching grants have expired.
pub fn evaluate(request: &PermissionRequest) -> Decision;
```

### approvals :: lib.rs

```rust
pub struct ApprovalDigest([u8; 32]);
impl std::str::FromStr for ApprovalDigest { type Err = KernelError; }
impl std::fmt::Display for ApprovalDigest;

pub struct DigestInput {
    pub request_id: Option⟨ApprovalRequestId⟩,
    pub principal_id: PrincipalId,
    pub actor_id: ActorId,
    pub run_id: Option⟨RunId⟩,
    pub operation: String,
    pub target: String,
    pub capabilities: Vec⟨Capability⟩,       // sorted by family then action
    pub extension_digest: Option⟨String⟩,
    pub config_digest: Option⟨String⟩,
    pub expiry_ms: i64,
    pub nonce: String,
}
pub fn canonical_digest(input: &DigestInput) -> ApprovalDigest;

pub async fn create_request(txn, input: DigestInput, now_ms: i64) -> Result⟨(ApprovalRequestId, ApprovalDigest)⟩;
pub async fn respond(txn, request_id, digest: &ApprovalDigest,
                     decision: ApprovalDecision, responder: Responder, now_ms: i64) -> Result⟨()⟩;

pub enum ApprovalDecision { Approve, Deny }
pub struct Responder { pub principal_id: PrincipalId, pub device_id: Option⟨DeviceId⟩ }

pub enum ApprovalOutcome { Approved, Denied, Pending, Expired, Invalidated, Unknown }
pub async fn is_satisfied(txn, request_id, expected: &ApprovalDigest, now_ms: i64) -> Result⟨ApprovalOutcome⟩;

pub struct CreateApprovalRequestHandler { /* deps */ }
pub struct RespondApprovalHandler { /* deps */ }
```

### secrets

```rust
#[derive(Clone)]
pub struct SecretMetadata { pub uri: String, pub kind: String, pub scopes: Vec⟨String⟩ }

pub struct SecretValue(zeroize::Zeroizing⟨Vec⟨u8⟩⟩);
impl SecretValue { pub fn expose(&self) -> &[u8]; }
impl std::fmt::Debug for SecretValue { /* prints a redacted marker */ }

pub struct ActResult { pub reference: String }

#[async_trait::async_trait]
pub trait SecretStore: Send + Sync {
    async fn metadata(&self, uri: &str) -> errors::Result⟨SecretMetadata⟩;
    async fn get(&self, uri: &str) -> errors::Result⟨SecretValue⟩;
    async fn sign_or_act(&self, uri: &str, action: &str, payload: &[u8]) -> errors::Result⟨ActResult⟩;
}

pub struct SecretUseRequest {
    pub principal_id: PrincipalId,
    pub actor_id: ActorId,
    pub run_id: Option⟨RunId⟩,
    pub chain: DelegationChain,
    pub uri: String,
    pub egress: Option⟨Vec⟨String⟩⟩,
    pub operation: String,
}

pub struct SecretsBroker { /* store, clock, audit sink */ }
impl SecretsBroker {
    pub async fn resolve_metadata(&self, request: &SecretUseRequest) -> Result⟨SecretMetadata⟩;
    pub async fn use_secret(&self, request: &SecretUseRequest) -> Result⟨SecretValue⟩;
    pub async fn sign_or_act(&self, request: &SecretUseRequest, payload: &[u8]) -> Result⟨ActResult⟩;
}

pub struct InMemorySecretStore;                  // documented test backend
pub struct MacOsKeychainSecretStore;             // target-gated; security-framework
```

| Interface | Produced by | Consumed by | Serves |
|---|---|---|---|
| `Capability`, `DelegationChain` | SEC-001 | permissions, runtime, later modules | R1, R2 |
| `evaluate`, `Decision` | SEC-002 | workspace, sandbox, adapters, config, effects, resources | R2 |
| `canonical_digest`, `create_request`, `respond`, `is_satisfied` | SEC-003 | control-api, blocked commands | R3 |
| `SecretsBroker`, `SecretStore` | SEC-004 | process supervisor, adapters | R4 |

## Data model

Existing tables only: `capability_grants`, `delegation_hops`, `approval_requests`,
`approval_responses`. Approval rows are immutable; resolution derives from response rows. No
migration. **Backward compatible:** n/a.

## Error handling

| Failure mode | Detection | Response | Serves |
|---|---|---|---|
| Requested capability not in parent | subset check | `FailedPrecondition`, `Never` | R1.2 |
| Missing grant row | integrity load | `FailedPrecondition`, `Never` | R1.3 |
| Chain hop mismatch | integrity load | `FailedPrecondition`, `Never` | R1.3 |
| Out-of-scope target | policy evaluation | `Deny { OutOfScope }` | R2.3 |
| Joint secret plus egress | joint rule | `RequireApproval` | R2.4 |
| Digest mismatch on response | digest comparison | `Conflict`, `Never` | R3.2 |
| Expired request | clock comparison | `FailedPrecondition`, `Never` | R3.2 |
| Second response | response existence | `Conflict`, `Never` | R3.4 |
| Unauthorized secret | broker policy | `FailedPrecondition` or `Conflict`, no material | R4.2 |
| Keychain unavailable | backend error | `Unavailable`, `Safe`, no fallback | R4.7 |
| Unknown capability | catalogue lookup | `InvalidArgument`, `Never` | R2.1 |

## Security considerations

| Concern | Treatment |
|---|---|
| Authentication / authorisation | this module is the authorization core; transport identity is control-api's |
| Input validation | capability catalogue, scope forms, and digest framing validated before use |
| Secrets handling | broker-only access; zeroizing raw values; redacted Debug/Display; audit without values |
| Data exposure | no secret or payload bytes in errors, logs, or events |
| New network surface | none |
| Dependency additions | `zeroize`; `security-framework` target-gated to macOS in `secrets` |

## Test strategy

| Level | Framework | Location | Covers |
|---|---|---|---|
| Unit/table | in-crate | identity, permissions, approvals, secrets | R1-R4 tables |
| Integration | real SQLite store | `identity/tests/delegation.rs`, `approvals/tests/approvals.rs` | persistence and commands |
| Golden digest | unit | `approvals/src/lib.rs` tests | canonical digest stability |
| Property | proptest | `permissions/tests/policy.rs`, `identity/tests/delegation.rs` | P1, P2 |
| Redaction | unit | `secrets/tests/broker.rs` | P4, N1 |
| Keychain | opt-in env gate | `secrets/tests/keychain.rs` | R4.6 |
| Regression | gates and validators | repo root and `agent-os/` | G1, G2, N3 |

## Observability

| Signal | Where | Content |
|---|---|---|
| Authorization outcome | caller logging | decision variant, capability, actor, run; never target contents beyond identifiers |
| Approval lifecycle | command events and logs | request id, digest, outcome |
| Secret use | broker audit record | actor, run, uri, outcome; never the value |

## Performance

| Requirement | Design mechanism | How it is measured |
|---|---|---|
| N2 | pure `evaluate`, deterministic digest | unit and property suites |
| Keychain latency | only on authorized raw access | opt-in integration test |

## Design decisions

| # | Decision | Alternatives rejected | Rationale | Serves |
|---|---|---|---|---|
| D1 | `Capability` lives in identity; permissions re-exports it | permissions owning it forces identity to depend upward | dependency direction stays inward | R1, R2 |
| D2 | One pure `evaluate` with explicit inputs | trait-based policy objects; a DSL | deterministic, table-testable | R2.5 |
| D3 | Joint risk upgrades Allow to RequireApproval | separate egress policy | `specs/permissions.md` requires joint evaluation | R2.4 |
| D4 | Approval digest frames sorted fields and hashes with SHA-256 lowercase hex | JSON canonicalization | no new dependency; explicit and testable | R3.1 |
| D5 | Resolution derives from the single response row | storing state on the request (forbidden by triggers) | immutability preserved | R3.4 |
| D6 | Raw secrets are `zeroize`-backed with redacted Debug/Display | plain `Vec⟨u8⟩` | prevents accidental rendering and residue | R4.3 |
| D7 | Keychain via `security-framework`, target-gated | shelling to `security` | no argv interpolation of secrets | R4.7 |
| D8 | Audit is structured logs, not a new event | adding `SecretUsed` | catalogue is locked | R4.5 |
| D9 | Callers supply `grant_scopes` loaded from `capability_grants`; the engine stays pure | permissions depending on the store | scope enforcement lives in `evaluate` while the engine remains testable without I/O | R2.3 |
| D10 | `ApprovalDraft` is a template; `approvals::create_request` assigns the nonce and canonicalizes | engine-side randomness would break purity | `evaluate` stays deterministic | R2.4, R3.1 |

## Requirements traceability

| Requirement | Covered by | Verified by |
|---|---|---|
| R1.1-R1.5 | identity types and chain service | `tests/delegation.rs`, P1 |
| R2.1-R2.6 | permissions engine | `tests/policy.rs`, P2 |
| R3.1-R3.6 | approvals service and handlers | `tests/approvals.rs`, P3 |
| R4.1-R4.7 | broker and backends | `tests/broker.rs`, `tests/keychain.rs`, P4 |
| N1-N3 | redaction, pure engine, pinned deps | suites and validators |
| G1, G2 | gates and validators | repo root and `agent-os/` |

## File structure

Paths relative to `agent-os/`.

| Path | Create or modify | Responsibility | Owner task |
|---|---|---|---|
| `Cargo.toml`, `crates/{identity,permissions,approvals,secrets}/Cargo.toml`, `Cargo.lock` | modify | workspace deps (`zeroize`, target-gated `security-framework`) and crate edges | SEC-000 |
| `crates/identity/src/{lib,delegation}.rs` | modify/create | identity types and chain service | SEC-001 |
| `crates/identity/tests/delegation.rs` | create | subset, tamper, persistence suites | SEC-001 |
| `crates/permissions/src/{lib,capabilities,policy}.rs` | modify/create | catalogue, scopes, evaluate | SEC-002 |
| `crates/permissions/tests/policy.rs` | create | decision tables, confused deputy, joint risk | SEC-002 |
| `crates/approvals/src/lib.rs` | modify | digest, service, handlers | SEC-003 |
| `crates/kernel-store-sqlite/src/repos/security.rs` | modify | any missing approval response reads | SEC-003 |
| `crates/approvals/tests/approvals.rs` | create | rejection and invalidation suites | SEC-003 |
| `crates/secrets/src/{lib,broker,keychain}.rs` | modify/create | broker, backends | SEC-004 |
| `crates/secrets/tests/{broker,keychain}.rs` | create | redaction, denial, joint, opt-in Keychain | SEC-004 |

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
