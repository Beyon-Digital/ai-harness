# INT-005 — Security and protocol invariant suite

## Deliverable
`crates/agentd/tests/security/main.rs` — 10 executable boundary checks:

1. `forged_adapter_hello_fields_rejected` — `ExpectedIdentity::verify_hello`
   rejects hello frames with a forged adapter_instance_id, adapter_id,
   version, bundle_digest, or protocol_version (control case passes).
2. `inbound_frame_order_enforced` — kernel-bound `Bootstrap`/`Shutdown`
   frames arriving inbound are rejected in both `AwaitingHello` and `Ready`
   phases; a no-body frame is rejected outright.
3. `t2_requirement_never_served_by_untrusted_adapter` — resolver with
   `sandbox_tier: T2` fails closed when the only candidate is `Untrusted`
   (even carrying all `REQUIRED_FOR_T2` capabilities); the `Trusted`
   registration wins when present.
4. `secret_material_never_leaks_into_output` — `SecretValue` Debug is
   `[REDACTED]`; grantless `use_secret` denial errors and audit records carry
   no plaintext.
5. `approval_digest_binding_rejects_mutated_request` — `respond` with a
   digest over a mutated target is rejected and persists nothing;
   `is_satisfied` returns `Invalidated` for the mutated digest, `Pending`
   for the true digest.
6. `confused_deputy_cannot_widen_past_parent` — `persist_hop` rejects a child
   hop whose grant set exceeds its parent's (Integrity), and rejects
   non-contiguous hop ordering.
7. `confused_deputy_tool_restriction_denies` — `permissions::evaluate`
   intersects the tool allow-list over the chain: a workspace-only helper
   cannot exercise the chain's `secret.use` grant (Deny).
8. `resource_uri_attacks_rejected` — 18 attack forms (raw/percent-encoded
   `..`, decoded separators, empty/double segments, scheme confusion,
   embedded NUL) all rejected; 5 legitimate forms still parse.
9. `workspace_scope_never_covers_foreign_or_traversal_targets` —
   workspace-write grant scoped to `team-a` does not cover `team-b` or a
   `../` target; in-scope write allowed.
10. `insecure_socket_directory_is_corrected_at_bind` — binding over a 0777
    runtime dir yields dir 0700 and socket 0600.

## Implementation fix surfaced by the suite
`crates/resource-uri/src/parser.rs` — `secret://<namespace>/<name>` tokens
were percent-decoded without the post-decode rejection that workspace path
segments get; `secret://vault/%2e%2e%2froot` decoded to name `../root`.
Added `decode_token` (rejects empty/`.`/`..`/decoded `/`,`\`,NUL) applied to
secret namespace and name.

## Run
- `cargo test -p agentd --test security` → 10/10.
- `cargo test -p resource-uri -p secrets -p identity -p permissions` → all
  green (parser change regresses nothing).
- `cargo fmt --check`, `cargo clippy --workspace --all-targets` clean.
