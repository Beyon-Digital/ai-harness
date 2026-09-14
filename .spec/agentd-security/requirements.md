# Requirements — agentd-security

**Status:** draft
**Date:** 2026-09-11
**Plan:** `plan.md` (approved)

## Glossary

| Term | Definition |
|---|---|
| Principal | The authenticated owner or system authority. |
| Actor | The client, agent, adapter, or tool making a request. |
| Delegation chain | The ordered authority path from a principal through hops to the current actor. |
| Grant | A persisted capability authorization identified by a grant ID. |
| Capability | A typed family plus action from the security capability catalogue. |
| Scoped target | The resource a capability applies to: workspace URI/path, domains, secret URI, config operation, child count. |
| Decision | `Allow`, `Deny`, or `RequireApproval` with a draft request. |
| Approval digest | The canonical SHA-256 binding of the exact approval request content. |
| Secret mediation | Broker-mediated access that never widens the daemon environment. |
| Sign-or-act | A broker operation producing a scoped signature or action instead of raw material. |

## Requirement R1: Authority lineage is persisted and subset-constrained

**User story:** As a security reviewer, I want every privileged call to carry an explicit,
persisted authority chain, so that a child can never hold more authority than its ancestors.

**Addresses:** SEC-001, decision D-020.

**Acceptance criteria (EARS):**

1. WHEN a delegation hop is created THE SYSTEM SHALL persist it with the exact grant IDs it carries, its parent hop, and the actor/run context.
2. WHEN a child chain is derived THE SYSTEM SHALL include only capabilities that are a subset of its parent's grants, and IF any requested capability is absent from the parent THEN derivation SHALL fail.
3. WHEN a chain is loaded THE SYSTEM SHALL validate integrity: every hop links to its parent, every referenced grant exists, and each hop's capabilities are a subset of its parent's.
4. IF validation fails THEN the chain SHALL be rejected as an integrity error and SHALL NOT be used for authorization.
5. WHEN a chain is used THE SYSTEM SHALL NOT infer authority from ancestry without the stored grant IDs.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| empty requested subset | derivation succeeds with an empty chain hop |
| superset request | rejected |
| missing parent grant | integrity error |
| tampered hop link | integrity error |
| duplicated hop | rejected deterministically |

**Non-goals for R1:** budget subsetting (resources module).

---

## Requirement R2: One permission engine yields Allow, Deny, or RequireApproval

**User story:** As a component author, I want a single authorization call, so that policy lives
in one place and confused-deputy scenarios cannot slip through.

**Addresses:** SEC-002, `specs/permissions.md`.

**Acceptance criteria (EARS):**

1. WHEN authorization is requested THE SYSTEM SHALL evaluate the capability against the principal, actor, delegation chain, scoped target, and system policy, and SHALL return exactly one of `Allow { grant_refs }`, `Deny { reason }`, or `RequireApproval { draft }`.
2. WHEN an untrusted child invokes a helper owned by a more privileged parent THE SYSTEM SHALL compute effective authority as the intersection of the child's delegated grants, the tool's allowed capabilities, ancestor constraints, and target policy.
3. WHEN a grant exists for one capability but the scoped target is outside its scope THEN THE SYSTEM SHALL deny with a scope reason.
4. WHEN a secret-use capability is requested alongside broad network egress THE SYSTEM SHALL return `RequireApproval` even if both grants exist.
5. WHEN policy evaluation runs THE SYSTEM SHALL NOT mutate state and SHALL be deterministic for identical inputs.
6. WHEN the engine refuses a request THE SYSTEM SHALL NOT include secret material in the reason.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| no grants | deny |
| exact scope match | allow |
| path traversal in a workspace scope | deny |
| wrong domain for a network scope | deny |
| secret plus unrestricted egress | require approval |

**Non-goals for R2:** enforcement; callers act on the decision.

---

## Requirement R3: Approvals bind to immutable content

**User story:** As an operator, I want approvals that cannot be replayed against changed
content, so that a granted approval means exactly what it was reviewed for.

**Addresses:** SEC-003, `specs/approvals.md`, decisions D-021.

**Acceptance criteria (EARS):**

1. WHEN an approval is requested THE SYSTEM SHALL compute a canonical digest over request ID, principal/actor/run, operation, target, sorted capabilities, extension and config digests when present, expiry, and nonce, and SHALL persist the immutable request through the command transaction with the catalogue event.
2. WHEN a response arrives THE SYSTEM SHALL require the request ID and digest to match, the request to be unexpired and unresolved, and the responder identity to be present.
3. IF any of those checks fail THEN THE SYSTEM SHALL reject the response with a stable code and persist nothing.
4. WHEN a valid response is persisted THE SYSTEM SHALL allow at most one terminal response per request.
5. WHEN the extension or config digest changes THE SYSTEM SHALL invalidate prior approvals bound to the old digest.
6. WHEN a blocked command checks its approval THE SYSTEM SHALL answer from the persisted request plus response, never from a boolean alone.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| digest mismatch | rejected |
| expired request | rejected |
| second response | rejected |
| changed bundle digest | prior approval invalid |
| wrong device/principal | rejected |

**Non-goals for R3:** remote signing and device authentication (remote-access module).

---

## Requirement R4: Secrets are mediated, scoped, and redacted

**User story:** As an operator, I want secrets to leave the daemon only through the broker, so
that adapters receive exactly the material one operation needs.

**Addresses:** SEC-004, decisions D-022, `specs/secrets.md`.

**Acceptance criteria (EARS):**

1. WHEN secret metadata or material is requested THE SYSTEM SHALL go through the broker, which evaluates permission and target/egress context first.
2. IF authorization denies or requires approval THEN material SHALL NOT be released and the failure SHALL carry no secret content.
3. WHEN raw material is returned THE SYSTEM SHALL wrap it in a zeroizing value that is never rendered by Debug or Display.
4. WHEN a provider supports scoped issuance or sign-or-act THE SYSTEM SHALL expose that operation and prefer it over raw material.
5. WHEN a secret is used THE SYSTEM SHALL emit an audit record carrying actor, run, target, and outcome but never the value.
6. WHERE the test backend is used THE SYSTEM SHALL provide an in-memory store that behaves identically to the Keychain backend's contract.
7. WHEN external code receives secrets THE SYSTEM SHALL supply only explicitly brokered material and SHALL NOT pass the daemon environment.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| unauthorized request | denied, no material |
| joint egress risk | approval required, no material |
| raw access authorized | zeroizing value, redacted rendering |
| sign-or-act available | scoped result, no raw value |
| keychain unavailable | stable error, no fallback to plaintext |

**Non-goals for R4:** secret creation/rotation UX; remote secret backends.

---

## Non-functional requirements

| Id | Category | Requirement (measurable) |
|---|---|---|
| N1 | Security | Secret values never appear in errors, logs, events, or Debug/Display; digest comparisons are constant-shape and exact. |
| N2 | Testability | Policy evaluation is pure and table-testable; Keychain tests are opt-in; no sleeps. |
| N3 | Compatibility | No schema or contract changes; new dependencies are workspace-pinned and target-gated. |

## Invariants (property-test candidates)

| Id | Invariant | Derived from |
|---|---|---|
| P1 | For any derived child chain, its capability set is a subset of its parent's. | R1.2 |
| P2 | For any evaluation inputs, the decision is one of the three variants and deterministic. | R2.5 |
| P3 | For any request, at most one terminal response exists. | R3.4 |
| P4 | For any broker path, no rendered output contains the secret value. | R4.3 |

## Regression guards

| Id | WHEN … THE SYSTEM SHALL CONTINUE TO … |
|---|---|
| G1 | WHEN the workspace quality gates run THE SYSTEM SHALL CONTINUE TO pass fmt, clippy with warnings denied, and the full suite. |
| G2 | WHEN the pack validators run THE SYSTEM SHALL CONTINUE TO report `BUILD PACK OK` and `OK` with contracts and schema unchanged. |

## Requirements self-analysis

- [x] **Contradictions** — R3's immutability and R3.4's single response agree: resolution derives from the single response row
- [x] **Ambiguity** — every decision, digest field, and scope is enumerated
- [x] **Conflicts** — R2.3 target scoping and R2.4 joint risk compose: the joint rule upgrades Allow to RequireApproval
- [x] **Unstated assumptions** — digest canonicalization, Keychain dependency, and audit-by-log are in the plan
- [x] **Missing edge cases** — each requirement carries rejection and tamper boundaries
- [x] **Testability** — pure engine, golden digest, barrier-free; Keychain opt-in
- [x] **Coverage** — every SEC brief item maps to R1-R4

**Findings and resolutions:**

| Finding | Requirements involved | Resolution |
|---|---|---|
| Where handlers for approval commands live | R3 | Proposed: in the approvals crate, registered by its tests and by control-api later |
| How Keychain tests stay deterministic | R4.6 | Proposed: env-gated opt-in with a dedicated service name; the in-memory backend carries the contract tests |
| Audit without a new event type | R4.5 | Proposed: structured observability logs with ids and outcome only |

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (standing instruction to proceed without per-step confirmation)
**Date:** 2026-09-11
