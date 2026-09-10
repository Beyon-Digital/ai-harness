# Instructions for AI Coding Agents

This file is normative for agents implementing the build pack.

## Execution rules

1. Read `README.md`, `DECISIONS.md`, `architecture/README.md`, and the task file before editing code.
2. Do not start a task until every ID in its `depends_on` list is complete.
3. Implement the task's **required outputs**, **invariants**, **tests**, and **acceptance criteria**. Passing compilation alone is not completion.
4. Do not weaken a kernel invariant to make a test pass.
5. Do not add abstractions unless the task/spec requires a substitution boundary.
6. Do not split internal Rust crates into network services.
7. Never execute untrusted/generated code in the T0 local-process sandbox.
8. Never retry an `Unknown` non-reconcilable effect automatically.
9. Never accept a loop decision that fails revision/epoch/step/cursor fencing.
10. Never hot-swap a running run's persisted `ResolvedRunEnvironment`.
11. Every state-changing Control API call must flow through `CommandCoordinator`.
12. Every authoritative mutation must occur inside `KernelStore` transaction semantics.
13. Durable live events become cursor-visible only after the Event Journal accepts them.
14. Same idempotency key + different request digest is a conflict, not a replay success.
15. Treat all external-process declarations as claims until validated by kernel policy/conformance.

## Task completion protocol

For each task:

- Run the exact task tests plus the full affected-crate tests.
- Run `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`.
- Update `TASK_STATUS.yaml` with status, commit SHA if available, tests run, and evidence.
- Do not mark complete if any acceptance criterion is unverified.

## When a spec is ambiguous

Do **not** invent behavior silently. Add a short entry to `OPEN_QUESTIONS.md` containing:

- task ID;
- ambiguous contract;
- two or more viable interpretations;
- the safest implementation default;
- whether the ambiguity blocks work.

Continue only when the default cannot violate a frozen decision. Otherwise stop that task.

## Prohibited shortcuts

- In-memory authoritative runtime state without durable DB record.
- `unwrap()`/`expect()` in request/runtime paths except proven impossible startup invariants with justification.
- raw global environment injection into external adapters.
- mutable extension code identified only by name/version.
- hidden child processes outside the supervisor.
- shared workspace writes without an enforceable lease/capability.
- emitting durable events directly to Live Bus before journal acceptance.
- writing a second persistence authority for canonical run state.
