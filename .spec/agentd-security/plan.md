# Plan — agentd-security

**Status:** draft
**Date:** 2026-09-11

Module spec. Phase 1 input is the approved umbrella plan at
`.spec/agentd-microkernel-mvp/plan.md`. This module is sixth in the build order; foundation,
persistence, command-core, events, and runtime-graph are complete and approved. Approvals below
are recorded under the user's standing instruction to proceed without per-step confirmation.

## Problem statement

Authority is unenforced. Runtime handlers execute with whatever the caller supplies; there is no
principal/actor/run lineage, no delegation subset invariant, no capability evaluation, no
approval binding, and no secret mediation. The tables (`capability_grants`, `delegation_hops`,
`approval_requests`, `approval_responses`) and grant/approval repositories exist, but no policy
code uses them.

## Outcome

Privileges become checkable: principals, actors, and ordered delegation chains persisted with
explicit grant IDs and a subset invariant; one permission engine evaluating capability families,
scoped targets, ancestor constraints, and joint secret-plus-egress risk into
Allow/Deny/RequireApproval; immutable approval requests bound to a canonical digest with
single-terminal-response semantics; and a secrets broker that mediates raw material through
authorized, audited, zeroizing access with an in-memory test backend and a macOS Keychain
backend. Tasks SEC-001 through SEC-004 are implemented and reviewed.

## Assumptions surfaced

| # | Assumption | If wrong, what changes |
|---|---|---|
| 1 | Capability families and actions come from `contracts/capabilities/security-capabilities.yaml`; the engine encodes them as a typed enum | A different vocabulary changes SEC-002 and every caller |
| 2 | Approval resolution is derived: `approval_requests` rows are immutable (schema triggers), so "unresolved" means no `approval_responses` row exists for the request | Persisting request state would need a schema change, which this module avoids |
| 3 | The approval digest covers the exact sorted capability request plus extension/config digests, expiry, nonce, principal/actor/run, operation, and target, canonicalized as sorted field framing hashed with SHA-256 lowercase hex | A different digest input changes replay/invalidations |
| 4 | The Keychain backend uses the `security-framework` crate behind a target-gated dependency; Keychain tests run only with an explicit environment opt-in and a dedicated service name | Shelling out to `security` would risk secret interpolation; an absent crate choice changes SEC-004's dependencies |
| 5 | Secret material crosses the boundary only as `zeroize`-backed values wrapped in the observability redaction type; audit is structured logs, not a new event type | Adding an audit event would need a catalogue change |
| 6 | Handlers for `CreateApprovalRequest` and `RespondApproval` live in the approvals crate and are registered by tests and by the control-api module later | Registration shape changes |
| 7 | Capability subsetting compares families and actions, while resource budget subsetting arrives with the resources module; SEC-001 enforces capability subset only | Budget subset moves earlier |

## Codebase evidence

| Finding | Evidence (`path:line`) | Consequence for this work |
|---|---|---|
| Identity layers, subset invariant, confused-deputy intersection are normative | `specs/identity-delegation.md` | SEC-001/002 implement exactly these |
| Decision enum and capability families are normative | `specs/permissions.md`; `contracts/capabilities/security-capabilities.yaml` | SEC-002's public shape and vocabulary |
| Approval digest inputs and rejection rules are normative | `specs/approvals.md` | SEC-003 hashes exactly the listed fields |
| Broker operations, joint evaluation, and backend list are normative | `specs/secrets.md`; `specs/permissions.md` joint section | SEC-004 implements mediate/redact/audit |
| Grants and delegation hops are persisted with repositories | persistence reports; `kernel-store-sqlite/src/repos/security.rs` | SEC-001 consumes the port; no schema change |
| Approval rows are immutable by trigger | `specs/kernel-store-schema.sql` triggers; recovery module | Resolution derived from response rows (assumption 2) |
| The coordinator is the only mutation path | command-core reports | Approval commands are handlers |
| Classification/redaction substrate exists | `crates/observability` (`Secret`, redaction) | SEC-004 reuses it; no payload logging |

## Existing conventions to follow

- Commands from `agent-os/`; validators and the mirror check from the repo root.
- No `unwrap()`/`expect()` outside tests; no secret or payload bytes in errors or logs; no sleeps.
- Commit per task with the task id; reports under this spec's `reports/` directory.

## Approach

### Chosen

Five sequential tasks:

1. **SEC-000** — declare dependencies; add `security-framework` and `zeroize` to the workspace.
2. **SEC-001** — identity: principal/actor/run context and ordered delegation chains persisted
   with explicit grant IDs; `derive_child_chain` subsetting; chain-integrity validation on load.
3. **SEC-002** — permissions: typed capability families and scoped targets; one
   `evaluate(request)` returning Allow/Deny/RequireApproval; confused-deputy intersection;
   joint secret-plus-egress rule.
4. **SEC-003** — approvals: canonical digest; create-request and respond commands; digest,
   expiry, identity, and single-response enforcement; resolution queries for blocked commands.
5. **SEC-004** — secrets: broker over a `SecretStore` trait, in-memory test backend, macOS
   Keychain backend, zeroizing redacted raw access, permission-plus-egress checks, and audit.

### Rejected

| Alternative | Why not |
|---|---|
| Persisting a `pending/resolved` state column on approval requests | Rows are immutable by trigger; resolution derives from responses (assumption 2) |
| A boolean approval flag | Explicitly forbidden: approval binds to a digest |
| Shelling out to the macOS `security` CLI | Interpolating secret values into argv risks leakage and injection |
| A new `SecretUsed` event | The catalogue is locked; audit stays structured logs until a later module adds an event |
| Inferring authority from ancestry without stored grant IDs | `specs/identity-delegation.md` forbids it |
| Resource budget subsetting in this module | Budgets belong to the resources module (assumption 7) |

## Scope

**In scope**

- SEC-000 through SEC-004
- Workspace dependencies: `security-framework` (target-gated), `zeroize`
- Handlers for `CreateApprovalRequest` and `RespondApproval` with registration tests

**Explicitly out of scope**

- Resource budget delegation (resources module)
- Remote device signing and relay approvals (remote-access module)
- Enforcement inside workspace/sandbox/adapters (later modules call the engine)
- New event types or schema changes

## Capability map

Single capability — authority. Five sequential tasks.

## Risks

| Risk | Likelihood | Blast radius | Mitigation |
|---|---|---|---|
| Digest canonicalization must be reproducible across modules | Medium | Approval replay/invalidations | One canonical encoder with fixed field framing and a golden test |
| Keychain tests are environment-dependent | High | SEC-004 CI stability | Opt-in environment gate plus a dedicated service name; in-memory backend covers the logic |
| Policy evaluation complexity sprawls | Medium | SEC-002 churn | One decision function, explicit inputs, table-driven tests; no DSL |
| Chain integrity checks duplicate the subset rule | Low | Drift | Subset is implemented once in `derive_child_chain` and reused by validation |
| New dependencies on macOS-only frameworks | Low | Build portability | Target-gated dependency; non-macOS builds compile the trait only |

## Parallelisation forecast

Sequential: identity → permissions → approvals → secrets. The policy engine is the load-bearing
surface and every later module consumes it, so it lands before approvals and secrets.

## Open questions for the user

None blocking. Assumptions 1-7 are decisions under the standing instruction and visible here for
correction.

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
