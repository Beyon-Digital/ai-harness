#!/usr/bin/env python3
from pathlib import Path
import hashlib, json, re, sys, yaml

ROOT=Path(__file__).resolve().parents[1]
errors=[]

def err(msg): errors.append(msg)

# DAG consistency
try:
    y=yaml.safe_load((ROOT/'dag.yaml').read_text())
    j=json.loads((ROOT/'dag.json').read_text())
    if y!=j: err('dag.yaml and dag.json differ')
    tasks=y['tasks']; ids=[t['id'] for t in tasks]
    if len(ids)!=len(set(ids)): err('duplicate task IDs')
    idset=set(ids)
    for t in tasks:
        for d in t.get('depends_on',[]):
            if d not in idset: err(f'{t["id"]} missing dependency {d}')
        if not (ROOT/'tasks'/f'{t["id"]}.md').exists(): err(f'missing task doc {t["id"]}')
    # acyclic Kahn
    indeg={i:0 for i in idset}; children={i:[] for i in idset}
    for t in tasks:
        for d in t.get('depends_on',[]):
            indeg[t['id']]+=1; children[d].append(t['id'])
    q=[i for i,v in indeg.items() if v==0]; seen=[]
    while q:
        n=q.pop(); seen.append(n)
        for c in children[n]:
            indeg[c]-=1
            if indeg[c]==0:q.append(c)
    if len(seen)!=len(idset): err('DAG contains a cycle')
except Exception as e: err(f'DAG parse/validation failed: {e}')

# Markdown links
link_re=re.compile(r'\[[^\]]+\]\(([^)]+)\)')
for p in ROOT.rglob('*.md'):
    text=p.read_text(errors='replace')
    for target in link_re.findall(text):
        if target.startswith(('http://','https://','mailto:','#')): continue
        path=target.split('#',1)[0]
        if not path: continue
        q=(p.parent/path).resolve()
        if not q.exists(): err(f'broken link {p.relative_to(ROOT)} -> {target}')

# Contract lock
lock=ROOT/'contracts/contract-lock.sha256'
if not lock.exists(): err('missing contract lock')
else:
    for line in lock.read_text().splitlines():
        if not line.strip(): continue
        parts=line.split()
        if len(parts)<2: err(f'bad lock line {line}'); continue
        expected=parts[0]; rel=parts[-1].lstrip('./'); p=ROOT/'contracts'/rel
        if not p.exists(): err(f'locked contract missing {rel}'); continue
        actual=hashlib.sha256(p.read_bytes()).hexdigest()
        if actual!=expected: err(f'contract digest mismatch {rel}')

# JSON/YAML parse and inception versions
for p in ROOT.rglob('*.json'):
    try: json.loads(p.read_text())
    except Exception as e: err(f'invalid JSON {p.relative_to(ROOT)}: {e}')
for p in ROOT.rglob('*.yaml'):
    try: yaml.safe_load(p.read_text())
    except Exception as e: err(f'invalid YAML {p.relative_to(ROOT)}: {e}')
try:
    cfg=json.loads((ROOT/'contracts/config/agent-os.schema.json').read_text())
    if cfg['properties']['schema_version']['const']!=1: err('config schema inception const != 1')
    ext=json.loads((ROOT/'contracts/manifests/extension.schema.json').read_text())
    if ext['properties']['manifest_version']['const']!=1: err('extension manifest inception const != 1')
except Exception as e: err(f'schema version check failed: {e}')

# Required SQL entities
sql=(ROOT/'specs/kernel-store-schema.sql').read_text().lower()
for table in ['daemon_fence','runs','run_dependencies','effects','resource_reservations','timers','capability_grants','approval_requests','adapter_registrations','config_generations','idempotency_records','event_stream_heads','outbox_events','resolved_run_environments','resolved_bindings','workspace_leases']:
    if f'create table if not exists {table}' not in sql: err(f'missing SQL table {table}')
if 'run_edges' in sql: err('duplicate parent/dependency run_edges table should not exist')

# Status coverage
try:
    st=yaml.safe_load((ROOT/'TASK_STATUS.yaml').read_text())['tasks']
    if set(st)!=set(ids): err('TASK_STATUS task IDs do not match DAG')
except Exception as e: err(f'status validation failed: {e}')

if errors:
    print('BUILD PACK INVALID')
    for e in errors: print(' -',e)
    sys.exit(1)
print(f'BUILD PACK OK: {len(ids)} tasks, {len(list(ROOT.rglob("*.md")))} markdown files, contracts locked')
