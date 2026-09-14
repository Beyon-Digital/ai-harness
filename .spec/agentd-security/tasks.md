# Tasks — agentd-security

**Date:** 2026-09-11
**Requirements:** `requirements.md` (approved)
**Design:** `design.md` (approved)

## Global constraints

- All Rust lives under `agent-os/`; no build-pack edits.
- From `agent-os/`: `cargo check --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.
- From the repo root: both validators and the contract mirror check.
- No `unwrap()`/`expect()` outside tests; no secret or payload bytes in errors, logs, or events; no sleeps.
- Commit per task with the task id; retry `git commit` on `index.lock` after two seconds.

## Execution contract for subagents

1. Claim before writing: `python3 "$SPECFLOW" claim agentd-security TASK_ID AGENT_ID` then `start`.
2. Stay inside the task's `files:` lease; `block` and report if another file is needed.
3. `depends_on` interfaces are contracts from `design.md`; do not redesign them.
4. Test first where the task says so; focused test, then the affected suite once.
5. Report `DONE` · `DONE_WITH_CONCERNS` · `BLOCKED` · `NEEDS_CONTEXT` honestly.
6. Never dispatch your own reviewer.
7. Commit scoped to your files, then `review`, then write `.spec/agentd-security/reports/task-TASK_ID.md`.

---

### Task SEC-000: Declare security-module dependencies

- status: done
- owner: agent-sec000
- depends_on: none
- files: `agent-os/Cargo.toml`, `agent-os/Cargo.lock`, `agent-os/crates/identity/Cargo.toml`, `agent-os/crates/permissions/Cargo.toml`, `agent-os/crates/approvals/Cargo.toml`, `agent-os/crates/secrets/Cargo.toml`
- requirements: N3, G1, G2
- scope: small
- model: cheap

**Objective:** Every security task compiles against declared dependencies, with `zeroize` and a target-gated Keychain crate added once.

**Context the implementer cannot infer:**

- Workspace `[workspace.dependencies]`: add `zeroize = "1"` and `security-framework = "2"`.
- `identity`: add `kernel-store`, `async-trait`; dev `kernel-store-sqlite`, `testkit`, `tempfile`, `tokio` (macros, rt-multi-thread), `proptest`.
- `permissions`: add `identity`, `domain`, `errors`, `async-trait`; dev `proptest`.
- `approvals`: add `identity`, `domain`, `errors`, `kernel-store`, `command-coordinator`, `events`, `sha2`, `async-trait`, `prost`; dev `kernel-store-sqlite`, `testkit`, `tempfile`, `tokio`, `proptest`.
- `secrets`: add `identity`, `domain`, `errors`, `permissions`, `observability`, `async-trait`, `zeroize`, `sha2`; dev `testkit`, `tokio`. Add `security-framework` under `[target.'cfg(target_os = "macos")'.dependencies]`.
- Append-only `.workspace = true` entries; resolve the lock with `cargo check --workspace`.

**Steps:**

- [ ] Edit the five manifests; resolve the lock
- [ ] Run `cargo test --workspace --no-run`, clippy, fmt
- [ ] Commit: `chore(workspace): declare security module dependencies [SEC-000]`

**Acceptance criteria:**

- [ ] All crates resolve; lock updated
- [ ] N3/G1/G2 — no contract or schema change; gates and validators pass
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo check --workspace && cargo test --workspace --no-run` exits 0
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean

---

### Task SEC-001: Principals, actors, delegation chains, and grant lineage

- status: done
- owner: agent-sec001
- depends_on: SEC-000
- files: `agent-os/crates/identity/src/lib.rs`, `agent-os/crates/identity/src/delegation.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/security.rs`, `agent-os/crates/identity/tests/delegation.rs`
- requirements: R1.1, R1.2, R1.3, R1.4, R1.5, P1, N1
- scope: large
- model: capable

**Objective:** Authority lineage is persisted with explicit grant IDs, and a child chain can only be a subset of its parent.

**Context the implementer cannot infer:**

- Interfaces are in the design's `identity :: delegation.rs` block. `Capability` lives here (D1); permissions re-exports it.
- The existing `SecurityRepo` provides grant and delegation-hop insert/read (`insert_grant`, `get_grant`, `insert_delegation_hop`, `list_delegation_hops`). Read `kernel-store-sqlite/src/repos/security.rs` first; add read queries there only if the design's service needs them (that file is in this lease).
- `derive_child_chain` returns the granted subset and fails when the request is not a subset of the parent's capabilities. `validate_chain` reuses the same subset rule plus hop linkage and grant existence.
- Tests: subset derivation, superset rejection, tampered link, missing grant, duplicate hop, and a proptest that any derived child set is a subset of the parent (P1). Use a real SQLite store in a temp root.

**Steps:**

- [ ] Write the failing tests first; confirm RED; implement; GREEN
- [ ] Run `cargo test -p identity -p kernel-store-sqlite` and gates
- [ ] Commit: `feat(identity): delegation chains and grant lineage [SEC-001]`

**Acceptance criteria:**

- [ ] R1.1-R1.5 and P1 — subset invariant, integrity validation, no ancestry-only inference
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p identity -p kernel-store-sqlite` passes; clippy and fmt clean

---

### Task SEC-002: Capability and permission engine

- status: done
- owner: agent-sec002
- depends_on: SEC-001
- files: `agent-os/crates/permissions/src/lib.rs`, `agent-os/crates/permissions/src/capabilities.rs`, `agent-os/crates/permissions/src/policy.rs`, `agent-os/crates/permissions/tests/policy.rs`
- requirements: R2.1, R2.2, R2.3, R2.4, R2.5, R2.6, P2, N2
- scope: large
- model: capable

**Objective:** One deterministic `evaluate` returns Allow, Deny, or RequireApproval over typed capabilities, scopes, chains, and joint risk.

**Context the implementer cannot infer:**

- Interfaces are in the design's `permissions :: policy.rs` block; `Capability` and `DelegationChain` come from `identity`.
- Families and actions come from `contracts/capabilities/security-capabilities.yaml`; encode them in `capabilities.rs` with `from_str`-style parsing that rejects unknown values.
- Scope rules: workspace paths are prefix-matched after normalization and reject traversal (`..`); network domains match exactly or by a documented suffix rule; secret targets carry an optional egress list.
- Confused deputy (R2.2): effective authority is the intersection of chain grants, the tool's allowed set when provided, ancestor constraints, and target policy.
- Joint risk (R2.4): secret `Use` or `SignOrAct` together with `Network Connect` (or a secret target's egress) upgrades `Allow` to `RequireApproval` with a draft from the design.
- Tests: decision table per family, scope accept/reject including traversal, confused deputy with a privileged helper, joint-risk upgrade, and a proptest that any input yields exactly one variant deterministically (P2). No I/O; the engine is pure.

**Steps:**

- [ ] Write the failing policy tests first; confirm RED; implement; GREEN
- [ ] Run `cargo test -p permissions` and gates
- [ ] Commit: `feat(permissions): capability engine and joint risk [SEC-002]`

**Acceptance criteria:**

- [ ] R2.1-R2.6 and P2 — one decision function, intersection semantics, joint upgrade, no secret content in reasons
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p permissions` passes; clippy and fmt clean

---

### Task SEC-003: Immutable approval flow

- status: pending
- owner: -
- depends_on: SEC-002
- files: `agent-os/crates/approvals/src/lib.rs`, `agent-os/crates/kernel-store-sqlite/src/repos/security.rs`, `agent-os/crates/approvals/tests/approvals.rs`
- requirements: R3.1, R3.2, R3.3, R3.4, R3.5, R3.6, P3, N1
- scope: large
- model: capable

**Objective:** Approvals bind to a canonical digest and cannot be replayed against changed content, with exactly one terminal response.

**Context the implementer cannot infer:**

- Interfaces are in the design's `approvals :: lib.rs` block. Digest framing is explicit: fields in the listed order, capabilities sorted by family then action, length-prefixed strings, SHA-256 lowercase hex; include a golden test with a fixed input.
- The existing security repo provides approval request insert/get and response insert/list. Row immutability means "unresolved" is "no response row"; add a read for responses by request if missing (the file is in this lease).
- `create_request` runs inside the coordinator's transaction and stages the catalogue event `ApprovalRequested` (check `proto/events/catalog.yaml` for the exact type and stream). Handlers decode their prost payloads and are registered in a `register_handlers(registry, deps)` helper for tests and later control-api wiring.
- Tests: digest mismatch, expired request, double response, wrong responder, changed extension/config digest invalidating a prior approval, and `is_satisfied` outcomes. Use a real coordinator plus SQLite store.

**Steps:**

- [ ] Write the failing tests first; confirm RED; implement; GREEN
- [ ] Run `cargo test -p approvals -p kernel-store-sqlite` and gates
- [ ] Commit: `feat(approvals): digest-bound approval flow [SEC-003]`

**Acceptance criteria:**

- [ ] R3.1-R3.6 and P3 — canonical digest, exact rejections, single response, invalidation, resolution from records
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p approvals` passes; clippy and fmt clean

---

### Task SEC-004: Secrets broker and macOS Keychain backend

- status: pending
- owner: -
- depends_on: SEC-003
- files: `agent-os/crates/secrets/src/lib.rs`, `agent-os/crates/secrets/src/broker.rs`, `agent-os/crates/secrets/src/keychain.rs`, `agent-os/crates/secrets/tests/broker.rs`, `agent-os/crates/secrets/tests/keychain.rs`
- requirements: R4.1, R4.2, R4.3, R4.4, R4.5, R4.6, R4.7, P4, N1, N2
- scope: large
- model: capable

**Objective:** Secret material leaves the daemon only through an authorized, audited broker that prefers scoped operations and never renders values.

**Context the implementer cannot infer:**

- Interfaces are in the design's `secrets` block. `SecretValue` wraps `zeroize::Zeroizing⟨Vec⟨u8⟩⟩`, exposes bytes explicitly, and implements a redacting `Debug`; no `Display`, no serialization.
- The broker evaluates permission with the identity chain via `permissions::evaluate` (secret `Use` and joint egress), denying or requiring approval before any store call; audit records go through the `observability` substrate with actor, run, uri, and outcome only.
- `InMemorySecretStore` is the documented test backend and carries the contract tests; `MacOsKeychainSecretStore` uses `security-framework` behind the target gate with a narrow wrapper and never shell-interpolates values.
- Keychain tests are opt-in: they run only when `AGENTD_KEYCHAIN_TEST=1`, use a dedicated service name, create and delete their own item, and otherwise return early with a printed note (document this in the test file).
- Tests: redaction (Debug and any error paths), unauthorized denial, joint-egress approval requirement, sign-or-act preference, in-memory contract, and the opt-in Keychain integration.

**Steps:**

- [ ] Write the failing broker tests first; confirm RED; implement; GREEN
- [ ] Add the opt-in Keychain test and run it locally with the env gate set
- [ ] Run `cargo test -p secrets` and full gates
- [ ] Commit: `feat(secrets): broker, in-memory store, and macOS Keychain [SEC-004]`

**Acceptance criteria:**

- [ ] R4.1-R4.7, P4, N1 — authorization before access, zeroizing redacted values, scoped operations, audit without values
- [ ] No file outside `files:` changed

**Verification:**

- [ ] From `agent-os/`: `cargo test -p secrets --features ''` passes; clippy and fmt clean
- [ ] Keychain test passes with `AGENTD_KEYCHAIN_TEST=1` on this machine

---

## Checkpoints

| After wave | Check | Command |
|---|---|---|
| 1 | Dependencies resolve | From `agent-os/`: `cargo check --workspace` |
| 2 | Identity green | From `agent-os/`: `cargo test -p identity -p kernel-store-sqlite` |
| 3 | Permissions green | From `agent-os/`: `cargo test -p permissions` |
| 4 | Approvals green | From `agent-os/`: `cargo test -p approvals` |
| 5 | Public gates | From `agent-os/`: fmt, clippy, full suite; repo root: validators and mirror check |

## Rulings

| # | Ruling | Why | Cost if wrong |
|---|---|---|---|
| 1 | `Capability` lives in identity; permissions re-exports it | Keeps the dependency direction inward | If a future crate needs capabilities without identity, a move is additive |
| 2 | Approval resolution derives from response rows; no request state column | Request rows are immutable by trigger | A future state column needs a schema change and pack correction |
| 3 | Audit is structured logs, not a catalogue event | The event catalogue is locked | A later module may add a secret-audit event |
| 4 | Keychain tests are env-gated opt-in | CI stability | Coverage relies on the in-memory backend plus one local run |
