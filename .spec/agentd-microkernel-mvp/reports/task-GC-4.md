# Task GC-4 — Pin encodings and complete the recovery matrix

- Status: DONE
- Owner: agent-gc4
- Commit: `4d2d7f5` — `docs(specs): pin encodings and complete recovery matrix [GC-4]`
- Requirements: R3.1, R3.2, R3.3, R3.4, R4.1, R4.2, R4.3, R4.4, R4.5, R7.1, R7.2, R7.3, R7.4

## What was implemented

A prior agent left both leased files at their pre-task state (timestamps and `git diff`
confirmed no partial work). I wrote both deliverables from the task block and design
decisions D7, D8, D9, D15.

1. `specs/types-and-ids.md` — rewrote the encodings:
   - Full 28-newtype list from the design interfaces (the 13 existing plus `PrincipalId`,
     `ActorId`, `DeviceId`, `CommandId`, `DecisionId`, `TurnId`, `OperationId`,
     `AgentSpecId`, `AdapterId`, `DependencyId`, `DelegationChainId`, `ArtifactId`,
     `SandboxId`, `EnvironmentId`, `DaemonInstanceId`).
   - UUIDv7 canonical form: lowercase hyphenated, version-nibble validation on parse,
     stable invalid-identifier error, byte-identical `Display`/`FromStr` round-trip, no raw
     strings across internal APIs.
   - `IdempotencyKey` bounds; SemVer 2.0.0 for adapter/protocol/`VersionedRef` versions with
     an unambiguously extractable major.
   - Digest encoding: lowercase hex SHA-256, no prefix, 64 chars.
   - Request digest: `SHA-256(digest_input)` with the canonical envelope byte layout (the
     seven covered fields in fixed order, each `u32be`-length-prefixed) and protobuf
     deterministic serialization for the payload. Exhaustive coverage table including the
     excluded `deadline_unix_ms`, `correlation_id`, `causation_id`.
   - Cursor grammar `v1:{stream_key}:{sequence}` with canonical stream keys, decimal `u64`
     with no leading zeros, malformed input rejected with the stable parse error, and
     resumption semantics via `LagNotice` (`v1:{stream_key}:{resume_sequence}`, no silent
     skip).
2. `specs/recovery-table.md` — rewrote as a decision matrix over the six dispositions:
   - Matrix A (no in-flight effect) covers all 11 non-`UNSPECIFIED` run states.
   - Matrix B (in-flight effect) covers `PREPARED`, `CLAIMED` (expired lease; live lease
     after daemon epoch change; live lease, epoch unchanged), `DISPATCHED` (reconciliation
     lookup; safe-idempotent redispatch; neither), `ACKNOWLEDGED` (owning run transition
     uncommitted and committed), and `UNKNOWN`.
   - Matrix C covers effects with a terminal owning run, including `PREPARED` with a
     terminal owner.
   - Matrix D plus the fail-closed section cover `BLOCKED_MISSING_RESOURCE` and the D7 rule
     that unmapped combinations select no default and abort startup.
   - `ResolveBlockedRun { run_id, action = resume | cancel, reason }` is documented as the
     only exit for `BLOCKED_MISSING_RESOURCE`, including the no-substitution rule for the
     frozen environment.
   - R7.4 is stated: `UNKNOWN` effects are never redispatched; `ResolveUnknownEffect` with a
     recorded decision is the only exit.
   - D8 is stated: no disposition references `RunPaused`/`RunResumed`.
   - Non-run recovery rows (idempotency, outbox, timers, resources, workspace lease, stale
     external process) were preserved with an explicit disposition classification.

## Acceptance criteria

| Criterion | Status | Evidence |
|---|---|---|
| R3.1 — digest algorithm defined over explicit canonical serialization | met | `types-and-ids.md` "Request digest": `SHA-256(digest_input)`, `u32be`-length-prefixed envelope, protobuf deterministic payload |
| R3.2 — two independent implementations produce byte-identical digests | met | fixed field order + length prefixes + deterministic payload rule; stated as a property |
| R3.3 — covered and excluded envelope fields specified | met | coverage table lists all 7 covered and all 5 excluded fields |
| R3.4 — lowercase hex, no prefix | met | "exactly 64 `[0-9a-f]` characters (R3.4)" |
| R4.1 — lowercase hyphenated UUIDv7 | met | identifier encoding section |
| R4.2 — invalid identifier rejected with stable error, no mutation | met | parse rules (`InvalidId`) |
| R4.3 — typed newtypes, no raw strings | met | full newtype list + boundary conversion rule |
| R4.4 — SemVer versions with extractable major | met | versions section |
| R4.5 — cursor resumable, no silent skip | met | cursor grammar + `LagNotice` resumption paragraph |
| R7.1 — every enumerated run state in the matrix | met | enumeration check prints `[]`; all 11 states appear in matrix rows (extra check below) |
| R7.2 — unmapped combinations fail closed | met | "Unmapped combinations fail closed" section plus Matrix D |
| R7.3 — `ResolveBlockedRun` is the blocked-run exit | met | "Blocked-run exit: `ResolveBlockedRun`" section; requires `BLOCKED_MISSING_RESOURCE`, `resume`/`cancel` |
| R7.4 — `Unknown` effects never redispatched without a decision | met | Matrix B/C rows and "Unknown effects never redispatch silently" section |
| No file outside `files:` changed | met | `git show --stat 4d2d7f5` lists only the two leased paths |

## Verification output (actual)

Task run-state enumeration check:

```
$ python3 -c "import re; p=open('agent-os-microkernel-mvp-buildpack/specs/recovery-table.md').read(); e=open('agent-os-microkernel-mvp-buildpack/contracts/domain/core.proto').read(); states=re.findall(r'[A-Z_]{3,}', re.search(r'enum RunState \{(.*?)\}', e, re.S).group(1)); print([s for s in states if not s.endswith('UNSPECIFIED') and s not in p])"
[]
```

Disposition-assignment grep:

```
$ grep -n 'NEEDS_RECONCILIATION\|REQUIRES_HUMAN_DECISION' agent-os-microkernel-mvp-buildpack/specs/recovery-table.md
34:| `WAITING_HUMAN` | approval expired | `REQUIRES_HUMAN_DECISION` | ... |
35:| `SUSPENDED` | — | `REQUIRES_HUMAN_DECISION` | ... |
49,50:| `RUNNING` / `WAITING_TOOL` / `CANCELLING` | `DISPATCHED` | ... | `NEEDS_RECONCILIATION` | ... |
55,56:| `SUSPENDED` | ... | `REQUIRES_HUMAN_DECISION` | ... |
68:| `COMPLETED` / `FAILED` / `CANCELLED` | `DISPATCHED` | — | `NEEDS_RECONCILIATION` | ... |
142:| External allocation uncertain | `NEEDS_RECONCILIATION` | ... |
```

Extra structural check (inline `python3` heredoc):

```
run states in matrix rows: {'CREATED': 'in-row', 'READY': 'in-row', 'RUNNING': 'in-row',
'WAITING_TOOL': 'in-row', 'WAITING_CHILD': 'in-row', 'WAITING_HUMAN': 'in-row',
'SUSPENDED': 'in-row', 'CANCELLING': 'in-row', 'COMPLETED': 'in-row', 'FAILED': 'in-row',
'CANCELLED': 'in-row'}
dispositions used in rows: {'NORMAL': 17, 'NEEDS_RECONCILIATION': 4, 'RECOVERING': 13,
'BLOCKED_UNKNOWN_EFFECT': 5, 'BLOCKED_MISSING_RESOURCE': 2, 'REQUIRES_HUMAN_DECISION': 4}
digest covered fields present: True
excluded fields present: True
cursor literal present: True
unknown-never-redispatch sentence: True
```

Repo validator:

```
$ python3 tools/validate_repo.py
OK: 230 markdown, 13 canonical ports, no link/schema/catalog errors
```

Commit scope:

```
$ git show --stat --oneline 4d2d7f5
4d2d7f5 docs(specs): pin encodings and complete recovery matrix [GC-4]
 .../specs/recovery-table.md                        | 163 +++++++++++++++++---
 .../specs/types-and-ids.md                         | 167 ++++++++++++++++++---
 2 files changed, 288 insertions(+), 42 deletions(-)
```

## Files changed

- `agent-os-microkernel-mvp-buildpack/specs/types-and-ids.md`
- `agent-os-microkernel-mvp-buildpack/specs/recovery-table.md`

## Concerns

- Non-blocking: audit finding B6 notes `examples/default-config.yaml:24` sets
  `effects.unknown_default: require_human_decision` while `effect-coordinator.md` says the
  same condition yields `BlockedUnknownEffect`. I resolved this in favour of
  `BLOCKED_UNKNOWN_EFFECT` as the persisted run disposition and documented that the policy
  value governs the required action (a recorded `ResolveUnknownEffect` decision). If the
  config/coordinator owner intends the config value to force `REQUIRES_HUMAN_DECISION`, the
  `UNKNOWN` row changes; no other criterion is affected.
- Non-blocking: the task's digest coverage list leaves `delegation_chain_id` unclassified. I
  listed it as excluded (authenticated context, re-verified per request) so the coverage
  table is exhaustive for every envelope field.
- Non-blocking: `SUSPENDED` maps to `REQUIRES_HUMAN_DECISION` because D8 removed the
  pause/resume commands, even though `runtime-manager.md` still lists `Suspended -> Running`
  in the user-level state machine. Recovery does not invent that transition; the state-machine
  document is outside this task's lease.
- The report file and the concurrent modifications by other agents (`ledger.md`, `tasks.md`,
  untracked `agent-os/`) were deliberately left uncommitted.

## Fix pass — review findings

Commit `09f743f` — `docs(specs): fix recovery enum names and terminal-run precedence [GC-4]`,
scoped to the two leased files only.

Changes against the review findings:

1. `specs/recovery-table.md:50,51` — `UNKNOWN` → `UNKNOWN_RECONCILIATION`, the exact
   `ReconciliationSemantics` token from `contracts/domain/effects.proto:6`.
2. `specs/recovery-table.md:126` — `expected_effect_state = Unknown` → `= UNKNOWN`, the exact
   `EffectState` literal from `contracts/domain/effects.proto:7`.
3. `specs/recovery-table.md:62-65` — added an explicit precedence rule: Matrix C (terminal
   owning run) takes precedence over Matrix D, because a terminal run has no disposition to
   block and a `PREPARED`/`CLAIMED` orphan is cancelled or fenced locally without resolving
   the frozen adapter. `specs/recovery-table.md:84` Matrix D adapter-unavailable row scoped
   to "any non-terminal run" so no terminal pair matches two rows.
4. `specs/recovery-table.md:20` — `effects.lease_expires_unix_ms` →
   `effects.lease_expires_ms`, matching the durable column in
   `specs/kernel-store-schema.sql:195`.
5. `specs/recovery-table.md:142` — restored the dropped stale-executor row ("Effect result
   from a stale executor after a higher fencing token claimed" → `NORMAL`, reject the commit,
   diagnostic trace only); the rule also remains in `tasks/EFF-003.md:24`.
6. `specs/types-and-ids.md:152-155` — split the bullet so persisted cursor columns and
   `contracts/protocols/agent_loop.proto` fencing fields are listed separately.

Commands run and their real output:

```
$ python3 -c "import re; p=open('agent-os-microkernel-mvp-buildpack/specs/recovery-table.md').read(); e=open('agent-os-microkernel-mvp-buildpack/contracts/domain/core.proto').read(); states=re.findall(r'[A-Z_]{3,}', re.search(r'enum RunState \{(.*?)\}', e, re.S).group(1)); print([s for s in states if not s.endswith('UNSPECIFIED') and s not in p])"
[]
```

```
$ python3 tools/validate_repo.py; echo "exit=$?"
OK: 234 markdown, 13 canonical ports, no link/schema/catalog errors
exit=0
```

```
$ grep -n 'UNKNOWN\|Unknown' agent-os-microkernel-mvp-buildpack/specs/recovery-table.md
9:`NEEDS_RECONCILIATION`, `RECOVERING`, `BLOCKED_UNKNOWN_EFFECT`, `BLOCKED_MISSING_RESOURCE`,
18:- `PREPARED`, `CLAIMED`, `DISPATCHED`, `ACKNOWLEDGED`, and `UNKNOWN` are the in-flight
50:| `RUNNING` / `WAITING_TOOL` / `CANCELLING` | `DISPATCHED` | reconciliation is `IMPOSSIBLE` or `UNKNOWN_RECONCILIATION` and idempotency is `NATURALLY_IDEMPOTENT` or `IDEMPOTENCY_KEY_SUPPORTED` | `NEEDS_RECONCILIATION` | Redispatch the same operation ID only under configured policy, then record the outcome. |
51:| `RUNNING` / `WAITING_TOOL` / `CANCELLING` | `DISPATCHED` | reconciliation is `IMPOSSIBLE` or `UNKNOWN_RECONCILIATION` and idempotency is `NOT_IDEMPOTENT` or `UNKNOWN_IDEMPOTENCY` | `BLOCKED_UNKNOWN_EFFECT` | Mark the effect `UNKNOWN`, block the run, and require a recorded `ResolveUnknownEffect` decision (R7.4). |
54:| `RUNNING` / `WAITING_TOOL` / `CANCELLING` | `UNKNOWN` | — | `BLOCKED_UNKNOWN_EFFECT` | Never dispatch again; the only exit is a recorded `ResolveUnknownEffect` decision (R7.4). |
57:| `SUSPENDED` | `UNKNOWN` | — | `BLOCKED_UNKNOWN_EFFECT` | Resolve the unknown effect with a recorded decision before anything else. |
73:| `COMPLETED` / `FAILED` / `CANCELLED` | `UNKNOWN` | — | `BLOCKED_UNKNOWN_EFFECT` | A recorded `ResolveUnknownEffect` decision is required before cleanup. The run state does not change. |
85:| any run | the same conditions as above and the effect state is `UNKNOWN` | `BLOCKED_UNKNOWN_EFFECT` | `BLOCKED_UNKNOWN_EFFECT` takes precedence because external side effects may already exist; resolve it first. |
111:  disposition is rejected as `FAILED_PRECONDITION`, so it cannot clear `BLOCKED_UNKNOWN_EFFECT`
122:## Unknown effects never redispatch silently
124:R7.4: an effect in state `UNKNOWN`, or any row that selects `BLOCKED_UNKNOWN_EFFECT`, is never
125:dispatched again by recovery. The recorded exit is `ResolveUnknownEffect` with
126:`expected_effect_state = UNKNOWN` and an explicit `action` (`mark_succeeded`, `mark_failed`,
```

Every remaining token is canonical (`EffectState.UNKNOWN`, `BLOCKED_UNKNOWN_EFFECT`,
`UNKNOWN_IDEMPOTENCY`, `UNKNOWN_RECONCILIATION`).

Task status left in `review`; controller re-reviews next.

## Fix pass 2 — global precedence across matrices

Commit `1d254d4` — `docs(specs): add global recovery precedence across matrices [GC-4]`,
scoped to `specs/recovery-table.md` only.

Changes:

- Added `## Precedence` (recovery-table.md:23-45): **B (unknown effects) > A (reconciliation)
  > C (terminal owner) > D (missing resource)**, with each class defined. An unknown effect is
  never ignorable; `DISPATCHED` reconciliation beats infrastructure blocking; a terminal owner
  does not block; Matrix D is applied last. The section also states that the ordinary rows
  (Matrix A, and Matrix B `PREPARED`/`CLAIMED`/`ACKNOWLEDGED`) only match when the run's
  frozen references resolve, so missing references fall through to D.
- Matrix A header note (`:47-49`) and Matrix B header note (`:65-68`) mark which rows are
  class B/A and which are ordinary.
- Matrix C paragraph (`:87-90`) now derives terminal-owner precedence from the global rule,
  and states a terminal run's `UNKNOWN` row is class B.
- Matrix D intro (`:101-104`) makes D last and assigns no pair that no Matrix A/B/C row
  covers; the fail-closed section (`:113-119`) repeats that D never maps those pairs.
- Fixed the cited overlaps: `RUNNING` + `DISPATCHED` with a missing frozen environment now
  resolves to `NEEDS_RECONCILIATION` (class A) over Matrix D; `CREATED`/`READY` + `PREPARED`
  is not covered by any A/B/C row, so Matrix D (`:107-109`) does not assign it and it stays
  unmapped and fails closed.
- Scoped the Matrix D effect rows (`:108-109`) to the runs they cover
  (`RUNNING`/`WAITING_TOOL`/`CANCELLING`/`SUSPENDED`) so they cannot capture
  `CREATED`/`READY` pairs.

Commands and real output:

```
$ python3 tools/validate_repo.py; echo "exit=$?"
OK: 234 markdown, 13 canonical ports, no link/schema/catalog errors
exit=0
```

```
$ python3 -c "import re; p=open('agent-os-microkernel-mvp-buildpack/specs/recovery-table.md').read(); e=open('agent-os-microkernel-mvp-buildpack/contracts/domain/core.proto').read(); states=re.findall(r'[A-Z_]{3,}', re.search(r'enum RunState \{(.*?)\}', e, re.S).group(1)); print([s for s in states if not s.endswith('UNSPECIFIED') and s not in p])"
[]
```

Task status left in `review`; controller re-reviews next.
