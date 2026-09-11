# Task GC-5 — Publish the normative limits

- Status: DONE
- Owner: agent-gc5
- Commit: `92d524e` — `docs(config): publish normative limits [GC-5]`
- Requirements: R10.1, R10.2, R10.3

## What was implemented

A prior agent was interrupted after writing the deliverables. I inspected every
partial file against the task's `Normative limits` block and D6, found the edits
complete and correct, and verified them with stronger checks than the task's
minimum (exact structural equality against the design, schema meta-validation,
and jsonschema-based rejection tests). No corrections were required; I committed
the verified state.

1. `specs/limits.yaml` — created with exactly the design's keys and values.
   Flat, machine-readable, all numeric values integers, file modes quoted strings.
2. `contracts/config/agent-os.schema.json` — added a typed `limits` object with
   the same key paths, `additionalProperties: false` at every level (`required`
   key lists too), `minimum: 1` for durations/capacities, `minimum: 0` for counts
   (`health_missed_allowed`, `supervisor_max_restarts`), and `^0[0-7]{3}$`
   patterns for the three file modes. `policies` retained and marked deprecated
   in its description.
3. `examples/default-config.yaml` — added a `limits:` block mirroring every value
   from `limits.yaml`.
4. `specs/config-engine.md` — added the `## Normative limits` section naming
   `limits.yaml` as the single source of truth (typed in the schema, mirrored in
   the example config, enforced by the GC-7 validator) and documented
   `RollbackConfigGeneration` with its capability gate, `expected_active_revision`
   CAS, `ConfigRolledBack` event, and no effect on running runs.
5. `architecture/persistence.md` — replaced "Recommended inception settings" with
   "These inception settings are normative" and added a pointer to
   `specs/limits.yaml` for the file-mode values.

## Acceptance criteria

| Criterion | Status | Evidence |
|---|---|---|
| R10.1 — every design limit key exists with its exact value | met | `exact-match: limits.yaml == design Normative limits` (structural dict equality, including `schema_version: 1`) |
| R10.2 — machine-readable so the GC-7 validator can fail on a missing key | met | `python3 -c "import yaml; d=yaml.safe_load(...)..."` parses cleanly; mirror check output below; `limits.yaml` has no anchors/aliases/duplicate keys |
| R10.3 — config schema types the limits object and rejects unknown keys | met | jsonschema 4.23.0: `default-config validates against schema`; mutating `limits.queue.bogus_key` fails; deleting `limits.approvals` fails; schema check asserts per-key type/minimum and `additionalProperties: false` on `limits` and every section |
| No file outside `files:` changed | met | `git show --stat 92d524e` lists only the five leased paths |

## Verification output (actual)

Task verification assertion:

```
$ python3 -c "import yaml; d=yaml.safe_load(open('agent-os-microkernel-mvp-buildpack/specs/limits.yaml')); assert d['adapters']['max_frame_bytes']==4194304 and d['effects']['lease_ms']==30000 and d['approvals']['ttl_ms']==900000 and d['shutdown']['drain_deadline_ms']==30000; print('limits ok')"
limits ok
```

Mirror check (exact command from the task):

```
$ python3 -c "import yaml; l=yaml.safe_load(open('agent-os-microkernel-mvp-buildpack/specs/limits.yaml')); d=yaml.safe_load(open('agent-os-microkernel-mvp-buildpack/examples/default-config.yaml'))['limits']; flat=lambda o,p='':{k if not p else p+'.'+k:(flat(v,p+'.'+k) if isinstance(v,dict) else v) for k,v in o.items()}; fl=flat(l); fd=flat(d); fl.pop('schema_version',None); print('missing-in-default', sorted(set(fl)-set(fd))); print('mismatched', sorted(k for k in fl if fl[k]!=fd.get(k)))"
missing-in-default []
mismatched []
```

JSON parse and repo validator:

```
$ python3 -m json.tool agent-os-microkernel-mvp-buildpack/contracts/config/agent-os.schema.json > /dev/null && echo "json ok"
json ok
$ python3 tools/validate_repo.py
OK: 229 markdown, 13 canonical ports, no link/schema/catalog errors
```

Extra checks (run as inline `python3 - <<'EOF'` heredoc scripts):

1. Structural equality of `limits.yaml` against the design block:
   `exact-match: limits.yaml == design Normative limits`
2. Schema walk over the `limits` properties asserting key-path parity with
   `limits.yaml`, `integer` type and `minimum` (0 for `health_missed_allowed`
   and `supervisor_max_restarts`, else 1), `^0[0-7]{3}$` for `fs.*`,
   `additionalProperties: false` on `limits` and all ten sections, and the
   `policies` deprecation description:
   `schema: key paths, types/minima, unknown-key rejection, deprecated policies all ok`
3. `jsonschema.Draft202012Validator` run:
   `default-config validates against schema`, then the same config with
   `limits.queue.bogus_key` added is rejected (`unknown limits key rejected`),
   and with `limits.approvals` deleted is rejected (`missing limits key rejected`).

## Files changed

- `agent-os-microkernel-mvp-buildpack/specs/limits.yaml` (new)
- `agent-os-microkernel-mvp-buildpack/contracts/config/agent-os.schema.json`
- `agent-os-microkernel-mvp-buildpack/examples/default-config.yaml`
- `agent-os-microkernel-mvp-buildpack/specs/config-engine.md`
- `agent-os-microkernel-mvp-buildpack/architecture/persistence.md`

## Concerns

- None blocking. `RollbackConfigGeneration` is documented in `config-engine.md`
  only; the command/event catalog alignment is owned by GC-2, so the description
  is written to match the task's exact command name without editing contract
  files outside this task's lease.
- The untracked repo-root `agent-os/` directory is another agent's work product
  and was deliberately left untouched and uncommitted by this task.
