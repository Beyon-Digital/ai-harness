# Task GC-8 — Resolve duplicate protobuf enum value symbols

**Status:** DONE
**Owner:** agent-gc8
**Commit:** d4bff7a — `docs(contracts): rename colliding effect enum values [GC-8]`

## What was implemented

protoc rejects duplicate value names inside a single proto package. In package
`agentos.spec.v1`, `READ_ONLY` existed in both `WorkspaceAccessMode`
(`contracts/domain/core.proto`) and `EffectClass`
(`contracts/domain/effects.proto`), and `FAILED` / `CANCELLED` existed in both
`RunState` (`core.proto`) and `EffectState` (`effects.proto`). Only the effects
side was renamed:

- `EffectClass.READ_ONLY` → `EFFECT_CLASS_READ_ONLY` (`contracts/domain/effects.proto:4`)
- `EffectState.FAILED` → `EFFECT_STATE_FAILED` (`contracts/domain/effects.proto:7`)
- `EffectState.CANCELLED` → `EFFECT_STATE_CANCELLED` (`contracts/domain/effects.proto:7`)

Every effect-side reference in the six leased files was updated:

| File | Change |
|---|---|
| `contracts/domain/effects.proto` | three value renames |
| `contracts/catalog.yaml` | `effect_classes` entry `READ_ONLY` → `EFFECT_CLASS_READ_ONLY` |
| `specs/kernel-store-schema.sql` | EffectClass comment `1=…`; EffectState comment `6=…, 7=…` |
| `specs/effect-coordinator.md` | `EffectClass` rendering `ReadOnly` → `EffectClassReadOnly` |
| `specs/recovery-table.md` | settled effect states `FAILED`, `CANCELLED` → prefixed names |
| `SOURCE_CORRECTIONS.md` | new "Effect enum value prefixing" section next to the version-const corrections |

`contracts/domain/core.proto`, `contract-lock.sha256`, and the repo-root `spec/`
and `docs/` trees were not touched. Run-state and workspace-mode references
(`kernel-store-schema.sql:80-81`, `:149`, `catalog.yaml` `workspace_modes`,
`recovery-table.md` run-state rows, `specs/workspace.md:12`,
`specs/error-model.md:43`) are unchanged, as the commit diff shows: the only
changed lines are the effect-side lines listed above.

On `specs/effect-coordinator.md:11` the value is a PascalCase rendering of the
enum with the other values rendered in the same style (`LocalMutation`,
`ExternalMutation`, `Opaque`), so the renamed value was rendered as
`EffectClassReadOnly`; the literal `EFFECT_CLASS_READ_ONLY` appears in the
protobuf, catalog, schema comments, recovery table, and `SOURCE_CORRECTIONS.md`.

## Acceptance criteria

| # | Criterion | Result | Evidence |
|---|---|---|---|
| R1.1 | No duplicate message, service, or enum value symbol within a package | met | duplicate scan prints `[]` |
| R1.2 | Effects-side values renamed; `core.proto` untouched | met | `grep` on `effects.proto` shows all three new names; `core.proto` has zero diff and its `READ_ONLY` count is 1 before and after |
| — | Effect-side references updated in all six leased files; run-state/workspace-mode references untouched | met | new names present in all six files; commit diff touches only the effect-side lines; run-state/workspace-mode lines absent from the diff |
| — | No file outside `files:` changed | met | commit `d4bff7a` contains exactly the six leased paths |

## Verification output

Duplicate scan, exactly as written in the task:

```text
$ python3 -c "import re,pathlib,collections; vals=collections.defaultdict(list); [ [vals[(m.group(1) if (m:=re.search(r'^package\\s+([\\w.]+)\\s*;', p.read_text(), re.M)) else '', v.group(1))].append(str(p)) for em in re.finditer(r'enum\\s+(\\w+)\\s*\\{(.*?)\\}', p.read_text(), re.S) for v in re.finditer(r'([A-Z][A-Z0-9_]*)\\s*=\\s*\\d+', em.group(2))] for p in pathlib.Path('agent-os-microkernel-mvp-buildpack/contracts').rglob('*.proto')]; print([k for k,l in vals.items() if len(l)>1])"
[]
```

Renamed values present in `effects.proto`:

```text
$ grep -n 'EFFECT_CLASS_READ_ONLY\|EFFECT_STATE_FAILED\|EFFECT_STATE_CANCELLED' agent-os-microkernel-mvp-buildpack/contracts/domain/effects.proto
4:enum EffectClass { EFFECT_CLASS_UNSPECIFIED=0; EFFECT_CLASS_READ_ONLY=1; LOCAL_MUTATION=2; EXTERNAL_MUTATION=3; OPAQUE=4; }
7:enum EffectState { EFFECT_STATE_UNSPECIFIED=0; PREPARED=1; CLAIMED=2; DISPATCHED=3; ACKNOWLEDGED=4; COMMITTED=5; EFFECT_STATE_FAILED=6; EFFECT_STATE_CANCELLED=7; UNKNOWN=8; }
```

`core.proto` untouched:

```text
$ git show HEAD:agent-os-microkernel-mvp-buildpack/contracts/domain/core.proto | grep -c 'READ_ONLY'
1
$ grep -c 'READ_ONLY' agent-os-microkernel-mvp-buildpack/contracts/domain/core.proto
1
$ git diff -- agent-os-microkernel-mvp-buildpack/contracts/domain/core.proto
(no output)
```

Run-state / workspace-mode references left alone:

```text
$ grep -n 'READ_ONLY' agent-os-microkernel-mvp-buildpack/contracts/catalog.yaml
56:- EFFECT_CLASS_READ_ONLY
77:- READ_ONLY
$ git show HEAD -- agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql | grep '^[+-][^+-]'
-  -- EffectClass (contracts/domain/effects.proto): 1=READ_ONLY, 2=LOCAL_MUTATION,
+  -- EffectClass (contracts/domain/effects.proto): 1=EFFECT_CLASS_READ_ONLY, 2=LOCAL_MUTATION,
-  -- 4=ACKNOWLEDGED, 5=COMMITTED, 6=FAILED, 7=CANCELLED, 8=UNKNOWN.
+  -- 4=ACKNOWLEDGED, 5=COMMITTED, 6=EFFECT_STATE_FAILED, 7=EFFECT_STATE_CANCELLED, 8=UNKNOWN.
```

(The only diff lines in the schema are the two effect-side comments; `RunState`
lines 79-81/83-85 and `WorkspaceAccessMode` line 149 are absent from the diff.)

Repo validator:

```text
$ python3 tools/validate_repo.py
OK: 240 markdown, 13 canonical ports, no link/schema/catalog errors
```

Commit contents:

```text
$ git show --stat --oneline HEAD
d4bff7a docs(contracts): rename colliding effect enum values [GC-8]
 agent-os-microkernel-mvp-buildpack/SOURCE_CORRECTIONS.md       | 10 ++++++++++
 agent-os-microkernel-mvp-buildpack/contracts/catalog.yaml      |  2 +-
 .../contracts/domain/effects.proto                             |  4 ++--
 agent-os-microkernel-mvp-buildpack/specs/effect-coordinator.md |  2 +-
 .../specs/kernel-store-schema.sql                              |  4 ++--
 .../specs/recovery-table.md                                    |  2 +-
 6 files changed, 17 insertions(+), 7 deletions(-)
```

## Files changed

- `agent-os-microkernel-mvp-buildpack/contracts/domain/effects.proto`
- `agent-os-microkernel-mvp-buildpack/contracts/catalog.yaml`
- `agent-os-microkernel-mvp-buildpack/specs/kernel-store-schema.sql`
- `agent-os-microkernel-mvp-buildpack/specs/effect-coordinator.md`
- `agent-os-microkernel-mvp-buildpack/specs/recovery-table.md`
- `agent-os-microkernel-mvp-buildpack/SOURCE_CORRECTIONS.md`

## Concerns

- The canonical `spec/` and `docs/` trees keep the unprefixed values, and the
  pack snapshot now diverges from them (`SOURCE_CORRECTIONS.md` documents this,
  per the task). GC-7 must regenerate `contracts/contract-lock.sha256` and
  `MANIFEST.json` against the new `effects.proto`/`catalog.yaml` bytes; this task
  intentionally did not touch the lock.
- `specs/effect-coordinator.md:11` renders the renamed value as
  `EffectClassReadOnly` to match the PascalCase rendering of the other values on
  that line; a strict literal grep for `EFFECT_CLASS_READ_ONLY` will not match
  that single line. Every other effect-side reference uses the literal.
