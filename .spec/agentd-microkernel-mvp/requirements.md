# Requirements — agentd-microkernel-mvp

**Status:** draft
**Date:** 2026-09-11
**Plan:** `plan.md` (approved)

**Scope of this spec.** Per the approved plan (capability map, and your answer 7), this
directory covers the first module only: the gap-closure wave (plan findings A1–A5, B1–B6,
C1–C10 as normative build-pack edits) plus the foundation module (FND-001 through FND-006
and the module-root ownership repair). The remaining 14 modules get their own spec
directories in dependency order, each reusing this plan as Phase 1 input. Finding C1
(chaos-suite ownership) is deliberately **not** covered here; plan stage 4 assigns it to the
verification module. Plan findings are referenced in each requirement under **Addresses**.

## Glossary

| Term | Definition |
|---|---|
| Build pack | The `agent-os-microkernel-mvp-buildpack/` tree: docs, component specs, contracts, task briefs, and the task DAG. |
| Gap closure | The first foundation wave: normative edits to the build pack that resolve the audit findings in `plan.md`, with no Rust code. |
| Contract snapshot | The locked protobuf and JSON-schema files under the build pack `contracts/` tree, installed into the implementation repo for code generation. |
| Contract lock | `contracts/contract-lock.sha256`, the file listing the expected hash of every snapshot file. |
| Inception schema | The single initial SQLite schema for `kernel.db`. The MVP has no database migration lifecycle. |
| Kernel database | `kernel.db`, the authoritative store opened by `agentd`. |
| Command Coordinator | The single linearization path through which every state-changing command flows. |
| Effect | A potentially costly or external mutation represented by a durable record and a stable operation id. |
| RunGraph | The transactional graph of parent and dependency edges over agent runs. |
| Loop decision | A turn result produced by an external agent loop process, accepted only when its fencing tuple is current. |
| Fencing tuple | Run revision, loop epoch, step sequence, input cursor, turn id, and decision id. |
| Recovery disposition | The persisted classification, separate from run state, that says whether a restored run may resume, must reconcile, or needs a human. |
| Idempotency key | A caller-supplied key that makes a command replay-safe when presented with the same request digest. |
| Module root | A `lib.rs`, `main.rs`, or `mod.rs` file that declares a crate's or module tree's children. |
| Spec-tracked task graph | The spec-flow graph in `tasks.md`, distinct from the build pack `dag.yaml` task graph. |
| Build-pack validator | `agent-os-microkernel-mvp-buildpack/scripts/validate_buildpack.py`. |
| Repo validator | `tools/validate_repo.py`. |
| Digest | A SHA-256 hash over defined canonical bytes. |

---

## Requirement R1: The contract snapshot compiles and has a single source

**User story:** As a kernel engineer, I want one compilable, single-sourced contract
snapshot, so that code generation and cross-crate interfaces have exactly one definition of
every symbol.

**Addresses:** A1, N3.

**Acceptance criteria (EARS):**

1. WHEN the contract snapshot is compiled by the protobuf compiler THE SYSTEM SHALL report no duplicate symbol errors.
2. IF two files in the same protobuf package define the same message or service name THEN snapshot validation SHALL fail and SHALL name both files.
3. THE SYSTEM SHALL expose exactly one Control API service definition from the snapshot, with `mvp_control.proto` normative for the MVP.
4. WHEN a snapshot file changes without a regenerated contract lock THE SYSTEM SHALL fail build-pack validation.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| empty snapshot | validation fails, naming the missing manifest entry |
| one duplicate symbol pair | validation fails, naming both files |
| malformed proto | compile step reports file and line |
| concurrent lock regeneration | only a fully verified hash set passes; a mismatch fails closed |

**Non-goals for R1:** adding or removing RPCs; editing the canonical contracts under the
repo-root `spec/` tree.

---

## Requirement R2: Every command and loop decision has a canonical wire schema

**User story:** As an operator, I want every command and every loop decision to have a
canonical wire schema, so that the daemon, the CLI, and fixture loop processes interoperate
without private conventions.

**Addresses:** A2.

**Acceptance criteria (EARS):**

1. WHEN a state-changing command is submitted THE SYSTEM SHALL decode its payload against the canonical protobuf message registered for its command type.
2. IF a submitted command type has no registered payload message THEN THE SYSTEM SHALL reject the request with the stable unknown-command error code and SHALL NOT mutate state.
3. WHEN an external loop process submits a decision THE SYSTEM SHALL decode each of the six decision variants (Complete, Fail, Wait, SpawnAgent, InvokeEffect, RequestApproval) against a canonical message.
4. WHEN a command is catalogued THE SYSTEM SHALL declare its command type string, payload message, idempotency semantics, and emitted event types.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| empty payload where fields are required | rejected as malformed, no mutation |
| unknown command type | rejected, no mutation |
| unknown decision variant | rejected, no mutation |
| oversize payload | rejected per the normative frame and message size limits |
| duplicate submission | replay or conflict per idempotency key and digest |

**Non-goals for R2:** defining an HTTP or JSON command surface; gRPC and the external
adapter protocol only.

---

## Requirement R3: Request digests are canonical and deterministic

**User story:** As a client author, I want the request digest algorithm fully specified, so
that my client and the daemon agree on replay identity byte for byte.

**Addresses:** A3, C9.

**Acceptance criteria (EARS):**

1. WHEN the request digest specification is validated THE SYSTEM SHALL define it as SHA-256 over an explicit canonical serialization of the command envelope and payload, with the canonicalization rule named.
2. WHEN two independent implementations follow the specification for the same logical request THE SYSTEM SHALL produce byte-identical digests.
3. THE SYSTEM SHALL specify which envelope fields the digest covers and which are excluded.
4. WHEN a digest is persisted or displayed THE SYSTEM SHALL use lowercase hexadecimal without a prefix.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| empty payload | digest of the empty canonical payload is defined and stable |
| one covered field changed | digest differs |
| same key and digest replayed | stored outcome returned without mutation |
| same key, different digest | conflict, no mutation |
| concurrent identical submissions | exactly one mutation commits |

**Non-goals for R3:** implementing replay storage; that arrives with the persistence and
command-core modules, which reuse this definition.

---

## Requirement R4: Identifier, version, and cursor encodings are canonical

**User story:** As an integrator, I want canonical encodings for identifiers, versions, and
event cursors, so that independently built components round-trip the same bytes.

**Addresses:** C5, C9.

**Acceptance criteria (EARS):**

1. WHEN an entity identifier is serialized THE SYSTEM SHALL use lowercase hyphenated UUIDv7 form.
2. IF a caller supplies an identifier that is not a valid UUIDv7 value THEN THE SYSTEM SHALL reject it with a stable invalid-identifier error and SHALL NOT mutate state.
3. WHEN a kernel API exchanges an identifier THE SYSTEM SHALL use a dedicated typed newtype, and raw string identifiers SHALL NOT cross internal kernel APIs.
4. WHEN an adapter or protocol version string is persisted THE SYSTEM SHALL use semantic versioning whose major component is unambiguously extractable.
5. WHEN a client resumes a durable stream from the last cursor it received THE SYSTEM SHALL deliver every event after that cursor and no event before it.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| empty identifier | rejected as invalid |
| non-v7 UUID | rejected as invalid |
| malformed cursor | rejected with a stable parse error |
| resume after a retention gap | the client is told to resume from its last delivered cursor; no silent skip |
| concurrent identifier generation | generated values are unique within the store |

**Non-goals for R4:** changing the persisted column types; all identifiers remain text in
the inception schema.

---

## Requirement R5: The inception schema covers every persisted entity

**User story:** As a kernel engineer, I want the inception schema to cover every entity the
MVP persists, so that no repository needs to invent or auto-create a table.

**Addresses:** B1, B2, B5.

**Acceptance criteria (EARS):**

1. WHEN `kernel.db` is created from the inception schema THE SYSTEM SHALL provide durable storage for every entity type the MVP persists, including artifacts, workspaces, loop turns, accepted decisions, supervised adapter instances, and conformance reports.
2. WHEN the database is opened THE SYSTEM SHALL verify the recorded inception schema version and SHALL fail closed when the version is unknown.
3. WHEN repositories initialize THE SYSTEM SHALL NOT create tables outside the inception schema.
4. WHEN a resolved run environment has been persisted THE SYSTEM SHALL reject mutation of its stored bytes.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| fresh database | schema version is recorded and every entity table exists |
| reopened database | version verifies and no table is recreated |
| unknown schema version | open fails closed |
| missing entity table | startup fails closed; no repository auto-creates it |
| update attempt on a resolved environment | rejected |

**Non-goals for R5:** schema migrations or version upgrade paths.

---

## Requirement R6: Storage enforces state domains and fencing

**User story:** As a reliability engineer, I want the store itself to enforce state domains
and fencing, so that a bug in application code cannot persist an impossible state.

**Addresses:** B3, B4.

**Acceptance criteria (EARS):**

1. IF a write sets a state or enum column outside its declared domain THEN THE SYSTEM SHALL reject the write at the storage layer, not only through application pre-checks.
2. WHEN an effect executor, timer worker, or run claimant acts on a leased resource THE SYSTEM SHALL verify that the lease carries the daemon fencing epoch that issued it, and SHALL reject actions from a stale epoch.
3. WHEN the database is opened THE SYSTEM SHALL apply the normative pragma set with the values fixed by the build pack: foreign keys enforced, write-ahead journal, synchronous level, and busy timeout.
4. WHEN the kernel database file is created THE SYSTEM SHALL set owner-only file permissions as fixed by the normative limits.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| out-of-domain state value | storage rejects the write |
| stale daemon epoch | the lease action is rejected |
| wrong pragma outcome | startup fails closed |
| non-normative file permission | database creation fails closed |

**Non-goals for R6:** replacing pre-check validation; checks and constraints both exist.

---

## Requirement R7: Recovery dispositions are total and actionable

**User story:** As an operator, I want every restart situation to map to a defined recovery
disposition, so that the daemon never guesses whether an interrupted run may resume.

**Addresses:** B6.

**Acceptance criteria (EARS):**

1. WHEN the daemon starts and finds a non-terminal run or an in-flight effect THE SYSTEM SHALL assign exactly one recovery disposition from the frozen enum for every reachable combination of run state and effect state.
2. IF a reachable combination has no disposition rule THEN startup SHALL fail closed rather than choose a default.
3. WHEN a run is blocked for a missing resource THE SYSTEM SHALL provide a catalogued command that resolves or cancels the block.
4. WHEN an effect has the Unknown disposition THE SYSTEM SHALL NOT dispatch it again without an explicit recorded decision.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| run with an in-flight effect at restart | reconciliation or blocked disposition, never blind retry |
| run waiting on a child at restart | disposition resolves through the matrix |
| unmapped state combination | startup fails closed |
| blocked run for a missing resource | the resolution command clears or cancels the block |

**Non-goals for R7:** automatic resolution of ambiguous external effects.

---

## Requirement R8: The command catalog is the single authority

**User story:** As a kernel engineer, I want one authoritative command catalog, so that the
Control API, workers, and documentation never disagree about the command set.

**Addresses:** C2.

**Acceptance criteria (EARS):**

1. WHEN the command catalog is validated THE SYSTEM SHALL map every state-changing Control API method to exactly one catalogued command.
2. WHEN an operator needs to undo a bad configuration generation THE SYSTEM SHALL provide a catalogued rollback command that reactivates a prior known-good generation and SHALL NOT alter resolved environments of running runs.
3. WHEN a child run is created THE SYSTEM SHALL do so through exactly one authoritative command path, and loop decision spawn requests SHALL route through it.
4. WHEN a command is catalogued THE SYSTEM SHALL declare its payload shape, idempotency semantics, and emitted events.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| state-changing method with no catalogued command | catalog validation fails |
| child spawn from a loop decision | routed through the single authoritative command |
| rollback of a bad generation | prior generation reactivates; running runs unaffected |
| catalogued command with no emitted-events list | catalog validation fails |

**Non-goals for R8:** removing or renaming any currently catalogued command.

---

## Requirement R9: The event catalog is complete and classified

**User story:** As an integrator, I want every event type fully declared, so that consumers
know its classification, retention, and stream before it is ever emitted.

**Addresses:** C3.

**Acceptance criteria (EARS):**

1. WHEN a durable event is emitted THE SYSTEM SHALL require its event type to be declared in the catalog with an event version.
2. WHEN an event type is declared THE SYSTEM SHALL declare its default sensitivity class, retention class, and stream key mapping.
3. WHEN an event type is declared THE SYSTEM SHALL name the transition that produces it, and IF no transition produces it THEN catalog validation SHALL fail.
4. WHEN a payload raises sensitivity above the type default THE SYSTEM SHALL record the raised class, and SHALL reject a downgrade below the type minimum.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| event type not in catalog | emission rejected |
| declared without classification | catalog validation fails |
| event with no producing transition | catalog validation fails |
| payload classified above the default | raise recorded |
| payload classification below the type minimum | rejected |

**Non-goals for R9:** defining delivery guarantees; that is the events module.

---

## Requirement R10: Every asserted threshold has a normative value

**User story:** As a test author, I want every threshold to have one declared value, so that
two agents cannot write incompatible assertions for the same limit.

**Addresses:** A5.

**Acceptance criteria (EARS):**

1. WHEN a test or specification asserts a numeric threshold THE SYSTEM SHALL source the value from a single normative limits section covering frame size, queue capacity, lease duration, claim TTL, approval TTL, drain deadline, handshake timeout, retry budget, buffer bound, stream read limit, and file permissions.
2. IF a required limit has no declared value THEN build-pack validation SHALL fail.
3. WHEN a configuration document is loaded THE SYSTEM SHALL validate limits and timeouts against the configuration schema, and SHALL reject unknown limit keys.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| threshold asserted with no value | build-pack validation fails |
| zero or negative duration | configuration validation rejects |
| over-maximum value | configuration validation rejects per the schema bound |
| unknown limit key | configuration load rejects |

**Non-goals for R10:** tuning values at runtime without a new config generation.

---

## Requirement R11: DAG and task sources agree, and every file has one owner

**User story:** As a build orchestrator, I want task sources to agree and file ownership to
be explicit, so that parallel agents cannot race on the same file.

**Addresses:** A4, C4, C7, C8.

**Acceptance criteria (EARS):**

1. WHEN any two build-pack DAG sources are compared THE SYSTEM SHALL show identical task identifiers, dependency edges, titles, and test lists, with `dag.yaml` as the single machine-readable source.
2. WHEN a task declares files THE SYSTEM SHALL list every path the task writes, including the module root of every module it introduces, and SHALL NOT use prose or unbounded recursive globs.
3. WHEN two tasks in the same wave would write the same path THE SYSTEM SHALL require a dependency edge between them or a split of the file.
4. WHEN a module root is introduced or changed THE SYSTEM SHALL assign exactly one owning task for that path.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| file list missing a module root | validation fails |
| prose or unbounded glob in a file list | validation fails |
| two same-wave tasks sharing a path | validation fails unless a dependency edge is added |
| task id sets diverge across sources | validation fails |

**Non-goals for R11:** renumbering or dropping existing task identifiers.

---

## Requirement R12: Validation tooling catches the gap class

**User story:** As a maintainer, I want the build-pack validator to fail on the defect
classes this audit found, so that they cannot silently return.

**Addresses:** C6, C7, C8.

**Acceptance criteria (EARS):**

1. WHEN the build-pack validator runs THE SYSTEM SHALL fail on a task file list that omits a module root it needs, on a threshold asserted without a normative value, and on manifest count or hash drift.
2. WHEN `MANIFEST.json` is generated THE SYSTEM SHALL record the true file count and a verifying hash for every listed file.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| task with no concrete files | build-pack validation fails |
| threshold with no normative value | build-pack validation fails |
| manifest count drift | build-pack validation fails |
| hash mismatch | build-pack validation fails |

**Non-goals for R12:** validating the implementation workspace; the foundation module has
its own quality commands.

---

## Requirement R13: The workspace builds and the quality gates run

**User story:** As a developer, I want a compilable workspace with enforced quality gates,
so that every later module starts from a green baseline.

**Addresses:** plan stage 2, FND-001.

**Acceptance criteria (EARS):**

1. WHEN a developer runs the documented quality commands THE SYSTEM SHALL complete check, format check, lint with warnings denied, and tests for every workspace crate.
2. WHEN `agentd` starts with only the placeholder composition root THE SYSTEM SHALL exit cleanly without opening the kernel database or the Control API socket.
3. THE SYSTEM SHALL contain the crate set declared by the crate map, with no dependency cycle between crates.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| empty workspace | quality commands fail clearly |
| full crate set | check, lint, format, and tests pass |
| cycle introduced between crates | build fails |
| placeholder `agentd` startup | exits cleanly, no database or socket opened |

**Non-goals for R13:** implementing runtime behaviour in any crate.

---

## Requirement R14: Contract code generation is hermetic and deterministic

**User story:** As a build engineer, I want code generation to be hermetic and
reproducible, so that builds do not depend on machine state.

**Addresses:** plan stage 1, FND-002, N4.

**Acceptance criteria (EARS):**

1. WHEN the workspace is built on a machine without a system protobuf compiler THE SYSTEM SHALL generate Rust types from the contract snapshot using a vendored compiler.
2. WHEN the same snapshot and compiler version are processed twice THE SYSTEM SHALL produce byte-identical generated output.
3. WHEN protobuf messages are needed in Rust THE SYSTEM SHALL use the generated types, and a second hand-written copy of message structs SHALL NOT exist.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| no system protobuf compiler | the vendored compiler is used |
| repeated generation | output is byte-identical |
| snapshot changed | regenerated types follow; no hand-written duplicate exists |
| concurrent generation runs | identical output; no partial files remain |

**Non-goals for R14:** generating Python or TypeScript bindings in this module.

---

## Requirement R15: Domain types fail safely and round-trip exactly

**User story:** As a kernel engineer, I want typed domain values that never guess, so that
unknown wire values cannot become plausible-looking internal state.

**Addresses:** FND-003, P1.

**Acceptance criteria (EARS):**

1. WHEN a protobuf enum value is converted and the value is unknown to this build THE SYSTEM SHALL fail the conversion safely rather than map it to a default variant.
2. WHEN a valid domain value is converted to protobuf and back THE SYSTEM SHALL preserve it exactly.
3. WHEN an identifier is generated THE SYSTEM SHALL produce time-sortable UUIDv7 values unique within the store.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| unknown enum value | conversion fails safely |
| valid round trip | value preserved exactly |
| generated identifier | UUIDv7, unique, time-sortable |
| malformed identifier input | rejected, no mutation |

**Non-goals for R15:** business validation of domain field combinations.

---

## Requirement R16: Errors are typed, stable, and retry-classified

**User story:** As an API client author, I want stable machine-readable error codes and
retry classification, so that callers can react programmatically.

**Addresses:** FND-004.

**Acceptance criteria (EARS):**

1. WHEN a kernel operation fails THE SYSTEM SHALL return a typed error carrying a stable machine-readable code and a structural retry-safety classification.
2. WHEN a kernel error crosses the Control API THE SYSTEM SHALL preserve the machine-readable code, and distinct kernel failures SHALL NOT collapse into one status.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| unknown error from a dependency | mapped to a typed internal error, never a silent success |
| error crosses the gRPC boundary | code preserved |
| retry classification absent | treated as not retry-safe |

**Non-goals for R16:** defining the final public gRPC status surface; that is the
control-api module.

---

## Requirement R17: Tests control time, identifiers, and faults deterministically

**User story:** As a test author, I want deterministic control of time, ids, and failure
injection, so that crash and concurrency behaviour is reproducible.

**Addresses:** FND-005, N2.

**Acceptance criteria (EARS):**

1. WHEN a test needs the current time, identifier generation, or a disposable daemon host THE SYSTEM SHALL provide deterministic test doubles.
2. WHEN a named fault point is armed THE SYSTEM SHALL trigger it exactly once at the declared boundary.
3. WHEN crash or concurrency behaviour is tested THE SYSTEM SHALL express synchronization with fault points and barriers rather than wall-clock sleeps.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| clock unset | the deterministic clock starts at a fixed instant |
| fault armed but never reached | the test fails loudly rather than silently passing |
| two faults at one boundary | each triggers exactly once in declaration order |
| temporary host teardown | all child processes and files are removed |

**Non-goals for R17:** replacing the production clock or id provider.

---

## Requirement R18: Observability attaches classification and correlation

**User story:** As an operator, I want logs and spans that carry identifiers and data
classification, so that failures are diagnosable without leaking secrets.

**Addresses:** FND-006, N1.

**Acceptance criteria (EARS):**

1. WHEN kernel code emits a log record or span THE SYSTEM SHALL attach the identifiers needed for diagnosis and a data classification.
2. WHEN a value is classified as secret THE SYSTEM SHALL NOT render it through debug or display formatting.
3. WHEN a classified payload exceeds a sink's sensitivity threshold THE SYSTEM SHALL redact or omit the payload in that sink.

**Boundary conditions:**

| Input / condition | Expected behaviour |
|---|---|
| secret value passed to a log call | redacted or omitted |
| malformed classification | the record is rejected |
| payload over the sink threshold | redacted or omitted |
| debug formatting of a secret value | renders without content |

**Non-goals for R18:** dashboards, metrics exports, and retention infrastructure.

---

## Non-functional requirements

| Id | Category | Requirement (measurable) |
|---|---|---|
| N1 | Security | No secret value appears in logs, errors, or debug output; external processes receive only explicitly scoped secret material. |
| N2 | Testability | Every crash and concurrency test boundary is expressible as a named fault point; no test synchronizes with a wall-clock sleep. |
| N3 | Compatibility | Any contract snapshot change fails validation unless the contract lock is regenerated; generated-type freshness is checked in CI. |
| N4 | Reproducibility | For a fixed snapshot and compiler version, generated Rust is byte-identical across repeated runs and machines. |

## Invariants (property-test candidates)

| Id | Invariant | Derived from |
|---|---|---|
| P1 | For any protobuf enum value, conversion never silently maps an unknown value to a valid variant. | R15.1 |
| P2 | For any crate set in the workspace, the crate dependency graph contains no cycle. | R13.3 |
| P3 | For any armed fault point, the fault triggers exactly once. | R17.2 |

## Regression guards

| Id | WHEN … THE SYSTEM SHALL CONTINUE TO … |
|---|---|
| G1 | WHEN repo validation runs THE SYSTEM SHALL CONTINUE TO report no broken links, schema, or catalog errors across the documentation set. |
| G2 | WHEN the task graph is edited for ownership repair THE SYSTEM SHALL CONTINUE TO contain all 59 declared task identifiers, and SHALL NOT remove an existing dependency edge. |

## Requirements self-analysis

- [x] **Contradictions** — checked against the approved plan; no two requirements are collectively impossible
- [x] **Ambiguity** — every previously unquantified term now has a value in R10 or an encoding in R3 and R4
- [x] **Conflicts** — R13 quality gates and R14 hermetic codegen are compatible; both are build-time
- [x] **Unstated assumptions** — every referenced concept is in the Glossary
- [x] **Missing edge cases** — each requirement carries boundary rows including failure and concurrency cases
- [x] **Testability** — every EARS line maps to a validator check, unit test, or the quality commands
- [x] **Coverage** — every in-scope plan item maps to a requirement; deferred modules are excluded by design per plan answer 7

**Findings and resolutions:**

| Finding | Requirements involved | Resolution |
|---|---|---|
| Catalogued `RunPaused`/`RunResumed` have no producing transition or command | R9.3 | Proposed: remove both from the MVP catalog; re-add only with a future suspend command. Needs your call |
| Digest encoding, identifier string form, and version grammar were unspecified in the pack | R3.4, R4.1, R4.4, N3 | Proposed: lowercase hex digests without prefix; hyphenated lowercase UUIDv7; semantic versioning |
| Two needed commands are absent from the pack's 16: config rollback and blocked-resource resolution | R7.3, R8.2 | Proposed: add both; the `config: rollback` capability grant already exists in the pack |
| Unmapped recovery combinations have no stated default | R7.2 | Proposed: fail closed at startup; a human-decision disposition is a state, not a fallback |
| R15.1 and P1 state the same truth at different strengths | R15.1, P1 | Intentional: the invariant is the property-test form of the requirement |

---

## Approval

**Decision:** approved
**Approved by:** jainamshah (recorded from explicit chat instruction)
**Date:** 2026-09-11
