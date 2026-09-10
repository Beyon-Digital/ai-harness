# Frozen MVP Decisions

| ID | Decision |
|---|---|
| D-001 | `agentd` is the sole authoritative runtime/control-plane daemon. |
| D-002 | The Kernel Metadata Store is the only authoritative correctness store. |
| D-003 | SQLite is the inception `KernelStore` implementation. |
| D-004 | Canonical mutation + graph changes + reservations + effect preparation + idempotency outcome + outbox events commit in one DB transaction. |
| D-005 | Event Journal is downstream; durable live delivery occurs only after journal acceptance. |
| D-006 | Every potentially costly/external mutation is represented by an `EffectRecord`. |
| D-007 | `Unknown` non-reconcilable effects are never blindly retried. |
| D-008 | Effect executors use lease + fencing token. |
| D-009 | Agent loop decisions are fenced by run revision, loop epoch, step sequence, event cursor, turn ID, and decision ID. |
| D-010 | RunGraph mutation, cycle validation, readiness, and claiming are transactional. |
| D-011 | Parent cancellation uses a cancellation epoch checked during child creation. |
| D-012 | Recovery disposition is separate from normal run state. |
| D-013 | Started runs persist immutable `ResolvedRunEnvironment`; config changes affect only new runs. |
| D-014 | Bootstrap-global, generation-global, and run-scoped bindings are separate. |
| D-015 | `KernelStore` is bootstrap-global and not selectable through runtime profiles. |
| D-016 | Parallel write-capable coding children default to isolated workspace forks. |
| D-017 | Exclusive workspace write authority may be transferred to a child; transfer must revoke/enforce old writer authority. |
| D-018 | Shared concurrent writes require explicit adapter capability and coordination semantics. |
| D-019 | T0 local process is not a security boundary. Untrusted code requires T2+. |
| D-020 | Principal → actor/run → child → tool delegation chains are persisted and capability-subset constrained. |
| D-021 | Remote/local approvals bind to an immutable request digest, not a boolean alone. |
| D-022 | Secret and network authority are jointly evaluated. Prefer SignOrAct/short-lived credentials over raw long-lived secrets. |
| D-023 | External process identity is established by supervisor-owned private IPC and expected bundle digest. |
| D-024 | Extension/adaptor executable identity is content-addressed. |
| D-025 | Event/log payloads carry sensitivity and retention classification. |
| D-026 | Historical replay means reconstruction/audit/reproducibility metadata, not deterministic re-execution. |
| D-027 | Local Control API uses gRPC/protobuf over a Unix-domain socket for the MVP. |
| D-028 | Internal crates are libraries in one process, not microservices. |
| D-029 | Schema/manifest inception version is `1`. Any stale source constant `2` is corrected in this build pack. |
| D-030 | Generation-global service bindings are frozen for a daemon instance; live config activation cannot hot-rebind Event Journal, queue, Secret Store, or transport in the MVP. |
