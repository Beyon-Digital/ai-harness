# Task SEC-003 — Immutable approval flow

- Status: review-ready
- Agent: agent-sec003
- Spec: agentd-security
- Commit: `d33c591` — feat(approvals): digest-bound approval flow [SEC-003]
- Branch: feat/agentd-microkernel-mvp

## What changed

Implements the design's `approvals :: lib.rs` interface on top of the
immutable `approval_requests` / `approval_responses` rows: canonical digest
binding, exactly one terminal response, digest/expiry/resolution/responder
rejections, invalidation on changed extension/config digests, and
`is_satisfied` resolution from persisted records.

| File | Change |
| --- | --- |
| `agent-os/crates/approvals/src/lib.rs` | new implementation: `ApprovalDigest`, `DigestInput`, `canonical_digest`, `create_request`, `respond`, `ApprovalDecision`, `Responder`, `ApprovalOutcome`, `is_satisfied`, `ApprovalDeps`, `CreateApprovalRequestHandler`, `RespondApprovalHandler`, `register_handlers` |
| `agent-os/crates/approvals/tests/approvals.rs` | new: golden digest, create + catalogue event, digest mismatch, expiry, double response, wrong responder, extension/config invalidation, `is_satisfied` outcomes, malformed payload — all over a real coordinator + SQLite store |
| `agent-os/crates/kernel-store-sqlite/src/repos/security.rs` | unchanged: `list_approval_responses` already provided the responses-by-request read, so no repo addition was needed |

Digest framing (pinned by `canonical_digest_matches_the_golden_framing`,
fixed input → `c81be7b01d7be688821a379513a67c5497b86fdf5b15466543a58651d4dc0b7b`):

- fields in the design's order: request id, principal, actor, run, operation,
  target, capabilities, extension digest, config digest, expiry, nonce;
- every string is a `u32` big-endian length plus UTF-8 bytes; optional fields
  carry a one-byte presence marker before their text; `expiry_ms` is a signed
  64-bit big-endian integer;
- capabilities are sorted by family then action, deduplicated, and rendered as
  `family.action` tokens, so permutations bind one digest;
- SHA-256 over the framed buffer, rendered as 64 lowercase hex characters;
  `ApprovalDigest::from_str` accepts only the canonical lowercase form.

Flow semantics:

- `create_request` derives a missing request id from the command identity,
  assigns the canonical nonce (the request id) in place of the permissions
  template nonce, canonicalizes capabilities, persists the pending immutable
  request, and stages `ApprovalRequested` on `security/principal/<principal>`
  with the catalogue's `confidential`/`audit` classification inside the
  coordinator transaction.
- `respond` recomputes the digest from the persisted row, then rejects a
  digest mismatch (`Conflict`/`Never`), an expired request
  (`FailedPrecondition`/`Never`), a second response
  (`Conflict`/`Never`), a responder that is not the approval principal
  (`FailedPrecondition`/`Never`), and a missing device identity
  (`InvalidArgument`/`Never`), persisting nothing in every case.
- `is_satisfied` returns `Unknown` without a request row, `Invalidated` when
  the persisted digest differs from `expected` (changed extension/config
  digest), `Approved`/`Denied` from the single response row, `Expired` for an
  unresolved request past `expires_at_ms`, and `Pending` otherwise.
- Errors carry static messages only; no target, secret, or payload content is
  echoed. No `unwrap()`/`expect()` outside tests and no sleeps.

## Acceptance criteria

### R3.1 — canonical digest + immutable persistence + catalogue event — MET

`create_request_persists_the_request_and_stages_the_catalogued_event` asserts
the persisted row (pending, null `resolved_at_ms`, digest recomputable from
its fields), the assigned nonce, the canonical capability blob, and the
staged `ApprovalRequested` event (type, version 1, principal stream,
confidential/audit, prost `ApprovalRequest` payload with matching digest,
actor, run, operation, target, expiry, nonce, sorted capabilities).

### R3.2/R3.3 — echoed digest/expiry/resolution/responder checks, nothing persisted — MET

- `response_with_a_mismatched_digest_is_rejected_and_persists_nothing`
- `expired_requests_are_rejected_and_persist_nothing`
- `a_wrong_responder_is_rejected_and_persists_nothing` (wrong principal and
  missing device, then the bound responder commits)

Each asserts the stable code, `RetryClass::Never`, an empty response list, and
an unchanged outbox; mismatch/double-response are `Conflict`, expiry and wrong
principal `FailedPrecondition`, missing device `InvalidArgument`.

### R3.4/P3 — at most one terminal response — MET

`a_second_response_is_rejected_and_leaves_the_first` approves once, then
rejects a differing decision with `Conflict` and leaves exactly one `approve`
row bound to the digest, principal, and device.

### R3.5 — changed extension/config digest invalidates — MET

`changed_extension_or_config_digest_invalidates_a_prior_approval` approves a
request, proves the matching digest resolves `Approved`, then recomputes the
digest with a changed extension (and separately config) digest and resolves
`Invalidated`.

### R3.6 — resolution from request + response records — MET

`is_satisfied_reports_each_persisted_outcome` covers all six outcomes:
`Unknown` (no row), `Pending`, `Expired` (unresolved past expiry), `Approved`,
`Denied`, and (above) `Invalidated`; a terminal response wins over later
expiry because the response was validated while the request was live.

### P3 + N1 — no content in errors — MET

Every rejection message is a static string; tests assert the target token
never appears in digest, mismatch, and expiry errors, and the malformed
payload test asserts raw bytes are not echoed.

### Files — MET

Only `agent-os/crates/approvals/src/lib.rs` and
`agent-os/crates/approvals/tests/approvals.rs` changed in the commit
(`git show --stat HEAD`); `security.rs` is in the lease but needed no edit.

## Commands and evidence

### RED (tests first, before implementation)

```
$ cargo test -p approvals
   Compiling approvals v0.1.0 (...)
error[E0432]: unresolved imports `approvals::ApprovalDecision`, `approvals::ApprovalDeps`,
`approvals::ApprovalDigest`, `approvals::ApprovalOutcome`, `approvals::DigestInput`,
`approvals::Responder`, `approvals::CMD_CREATE_APPROVAL_REQUEST`,
`approvals::CMD_RESPOND_APPROVAL`, `approvals::canonical_digest`,
`approvals::is_satisfied`, `approvals::register_handlers`
  --> crates/approvals/tests/approvals.rs:10:5
   |
10 |     ApprovalDecision, ApprovalDeps, ApprovalDigest, ApprovalOutcome, DigestInput, Responder,
   |     ^^^^^^^^^^^^^^^^  ^^^^^^^^^^^^  ^^^^^^^^^^^^^^  ^^^^^^^^^^^^^^^  ^^^^^^^^^^  ^^^^^^^^^ no `Responder` in the root
   ...
error: could not compile `approvals` (test "approvals") due to 6 previous errors
```

### GREEN

```
$ cargo test -p approvals
running 9 tests
test canonical_digest_matches_the_golden_framing ... ok
test create_request_persists_the_request_and_stages_the_catalogued_event ... ok
test a_second_response_is_rejected_and_leaves_the_first ... ok
test a_wrong_responder_is_rejected_and_persists_nothing ... ok
test changed_extension_or_config_digest_invalidates_a_prior_approval ... ok
test malformed_respond_payloads_are_rejected_without_rows ... ok
test expired_requests_are_rejected_and_persist_nothing ... ok
test response_with_a_mismatched_digest_is_rejected_and_persists_nothing ... ok
test is_satisfied_reports_each_persisted_outcome ... ok
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.60s
```

### Gates (from `agent-os/`)

```
$ cargo test -p approvals -p kernel-store-sqlite
  approvals: 9 passed; 0 failed
  kernel-store-sqlite: 68 passed; 0 failed across bootstrap/cas/contention/fence/
  idempotency/immutability/outbox/outbox_concurrent/props/repos_remaining/rollback
exit=0

$ cargo test --workspace
exit=0; 97 test groups reported "ok"; 0 failures

$ cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 39.93s
exit=0

$ cargo fmt --check
exit=0
```

## Concerns

1. `ApprovalResolved` (catalogue `produced_by: RespondApproval`) is not staged
   by `respond`. The task specifies staging only `ApprovalRequested`, and no
   contract message defines an `ApprovalResolved` payload, so asserting a
   schema would be invented behavior. Control-api wiring should add it when a
   payload contract exists.
2. A terminal response wins over expiry in `is_satisfied` (`Approved` stays
   `Approved` after `expires_at_ms`); expiry applies to unresolved requests.
   This is the documented reading of "resolution derives from the response
   row" (R3.6) — a deny is terminal and an approve cannot be un-answered.
3. `Responder.device_id` is typed `Option` but must be present to respond:
   `approval_responses.device_id` is `NOT NULL` and R3.2 requires the responder
   identity present, so absence is `InvalidArgument`.
4. `create_request` replaces the permissions template nonce with the assigned
   request id (canonical and replay-stable). The digest binds the assigned
   nonce, so template nonces never leak into persisted requests.
5. `respond` verifies the persisted row by recomputing its digest and fails
   `Internal` on any mismatch, so a tampered immutable row can never authorize
   or resolve an approval.
