#!/usr/bin/env python3
from pathlib import Path
import argparse, collections, csv, hashlib, heapq, io, json, re, sys, yaml

ROOT=Path(__file__).resolve().parents[1]
errors=[]

def err(msg): errors.append(msg)

def sha256(p): return hashlib.sha256(p.read_bytes()).hexdigest()

def rel(p): return p.relative_to(ROOT).as_posix()

def flat(node, prefix=''):
    out={}
    for key,value in (node or {}).items():
        path=f'{prefix}{key}'
        if isinstance(value, dict): out.update(flat(value, f'{path}.'))
        else: out[path]=value
    return out

def topological_order(tasks):
    by_id={t['id']:t for t in tasks}
    indegree={t['id']:len(t.get('depends_on',[])) for t in tasks}
    children={t['id']:[] for t in tasks}
    for t in tasks:
        for dep in t.get('depends_on',[]): children[dep].append(t['id'])
    ready=[task_id for task_id in indegree if indegree[task_id]==0]
    heapq.heapify(ready)
    order=[]
    while ready:
        current=heapq.heappop(ready)
        order.append(by_id[current])
        for child in children[current]:
            indegree[child]-=1
            if indegree[child]==0: heapq.heappush(ready,child)
    return order

def write_lock():
    lock=ROOT/'contracts/contract-lock.sha256'
    files=sorted((p for p in (ROOT/'contracts').rglob('*') if p.is_file() and p!=lock), key=rel)
    lines=[f'{sha256(p)}  ./{p.relative_to(ROOT/"contracts").as_posix()}' for p in files]
    lock.write_text('\n'.join(lines)+'\n')
    print(f'wrote contracts/contract-lock.sha256 ({len(lines)} entries)')

def write_manifest():
    manifest_path=ROOT/'MANIFEST.json'
    files=sorted((p for p in ROOT.rglob('*') if p.is_file() and p!=manifest_path
                  and '__pycache__' not in p.parts and p.name!='.DS_Store'),
                 key=lambda p: p.relative_to(ROOT).parts)
    entries=[]
    for p in files:
        data=p.read_bytes()
        entries.append({'path': rel(p), 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest()})
    manifest={'schema_version': 1, 'name': 'agent-os-microkernel-mvp-buildpack',
              'file_count': len(entries)+1, 'files': entries}
    manifest_path.write_text(json.dumps(manifest, indent=2)+'\n')
    print(f'wrote MANIFEST.json ({len(entries)} files + itself = {len(entries)+1})')

parser=argparse.ArgumentParser(description='Validate the agent-os microkernel MVP build pack.')
parser.add_argument('--update-lock', action='store_true', help='rewrite contracts/contract-lock.sha256 and exit')
parser.add_argument('--write-manifest', action='store_true', help='rewrite MANIFEST.json and exit')
args=parser.parse_args()
if args.update_lock: write_lock()
if args.write_manifest: write_manifest()
if args.update_lock or args.write_manifest: sys.exit(0)

# DAG consistency
tasks=None
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

if tasks is not None:
    # Agent-side file ownership (R11.2-R11.4)
    owners=collections.defaultdict(list)
    for t in tasks:
        declared=set()
        for f in t.get('files',[]):
            if f in declared: err(f'{t["id"]} lists file twice: {f}')
            declared.add(f)
            owners[f].append(t['id'])
    for f,holders in owners.items():
        if re.search(r'[*?\s]', f):
            err(f'unbounded glob or prose in {holders[0]} files: {f}')
    by_id={t['id']:t for t in tasks}
    def ancestor_set(tid):
        acc=set(); stack=[tid]
        while stack:
            for dep in by_id[stack.pop()].get('depends_on',[]):
                if dep not in acc: acc.add(dep); stack.append(dep)
        return acc
    order=topological_order(tasks)
    levels={}
    if len(order)==len(tasks):
        for t in order:
            deps=t.get('depends_on',[])
            levels[t['id']]=0 if not deps else 1+max(levels[d] for d in deps)
    for f,holders in owners.items():
        if len(holders)<2: continue
        for i,a in enumerate(holders):
            for b in holders[i+1:]:
                if a in ancestor_set(b) or b in ancestor_set(a):
                    if levels and levels[a]==levels[b]:
                        err(f'path {f} written by same-wave tasks {a} and {b}')
                else:
                    err(f'path {f} written by incomparable tasks {a} and {b}')
    for f,holders in owners.items():
        if f.endswith('/Cargo.toml'):
            base=f.rsplit('/',1)[0]
            if not any(f'{base}/src/{m}.rs' in owners for m in ('lib','main')):
                err(f'crate manifest {f} has no declared module root')
        if re.search(r'/src/(lib|main)\.rs$', f):
            base=f.rsplit('/src/',1)[0]
            if f'{base}/Cargo.toml' not in owners:
                err(f'module root {f} has no owning crate manifest')
        if f.endswith('/mod.rs') and len(holders)!=1:
            err(f'module root {f} must have exactly one owning task, has {holders}')

    # DAG source equality (R11.1, G2)
    ids=[t['id'] for t in tasks]
    idmap={t['id']:t for t in tasks}
    docs={p.stem for p in (ROOT/'tasks').glob('*.md') if p.name!='README.md'}
    for tid in sorted(set(ids)-docs): err(f'missing task doc tasks/{tid}.md')
    for tid in sorted(docs-set(ids)): err(f'unknown task doc tasks/{tid}.md')
    for t in tasks:
        text=(ROOT/'tasks'/f'{t["id"]}.md').read_text()
        title=re.match(r'#\s+(\S+)\s+[—-]\s+(.+)', text)
        if not title:
            err(f'task doc title line malformed tasks/{t["id"]}.md')
        elif title.group(1)!=t['id'] or title.group(2).strip()!=t['title']:
            err(f'task doc title mismatch tasks/{t["id"]}.md')
        dep_line=re.search(r'^\*\*Depends on:\*\*\s*(.*)$', text, re.M)
        deps=[]
        if dep_line:
            raw=dep_line.group(1).strip()
            if raw.lower() not in ('', 'none'):
                deps=re.findall(r'\[([A-Z]+-\d+)\]\([^)]*\)', raw)
        if deps!=t.get('depends_on',[]): err(f'task doc dependencies mismatch tasks/{t["id"]}.md')
        tests_block=re.search(r'^## Required tests\s*\n(.*?)(?=^## |\Z)', text, re.M|re.S)
        md_tests=[]
        if tests_block:
            for line in tests_block.group(1).splitlines():
                line=line.strip()
                if line.startswith(('-','*')):
                    md_tests.append(line.lstrip('-* ').strip().strip('`'))
        if md_tests!=t.get('tests',[]): err(f'task doc test list mismatch tasks/{t["id"]}.md')

    md=(ROOT/'DAG.md').read_text()
    node_pairs=re.findall(r'^\s*(\w+)\["([^"]+)"\]$', md, re.M)
    nodes=dict(node_pairs)
    if len(node_pairs)!=len(nodes): err('DAG.md contains duplicate mermaid nodes')
    exp_nodes={t['id'].replace('-','_'):f"{t['id']}: {t['title']}" for t in tasks}
    for node in sorted(set(exp_nodes)-set(nodes)): err(f'DAG.md missing node {node}')
    for node in sorted(set(nodes)-set(exp_nodes)): err(f'DAG.md unknown node {node}')
    for node in sorted(set(nodes)&set(exp_nodes)):
        if nodes[node]!=exp_nodes[node]: err(f'DAG.md title mismatch {node}: {nodes[node]!r}')
    edges=[(a,b) for a,b in re.findall(r'^\s*(\w+)\s*-->\s*(\w+)$', md, re.M)]
    exp_edges=[(d.replace('-','_'),t['id'].replace('-','_')) for t in tasks for d in t.get('depends_on',[])]
    if collections.Counter(edges)!=collections.Counter(exp_edges):
        for edge in sorted(set(exp_edges)-set(edges)): err(f'DAG.md missing edge {edge[0]} -> {edge[1]}')
        for edge in sorted(set(edges)-set(exp_edges)): err(f'DAG.md unknown edge {edge[0]} -> {edge[1]}')
    order=topological_order(tasks)
    topo=re.findall(r'^\d+\.\s+\[([A-Z]+-\d+)\s+—\s+([^\]]+)\]\(tasks/([A-Z]+-\d+)\.md\)$', md, re.M)
    if len(topo)!=len(tasks): err(f'DAG.md topology lists {len(topo)} tasks, expected {len(tasks)}')
    for entry,t in zip(topo,order):
        if entry[0]!=t['id'] or entry[1]!=t['title'] or entry[2]!=t['id']:
            err(f'DAG.md topological order mismatch at {t["id"]}')
    csv_text=(ROOT/'tasks.csv').read_text()
    reader=csv.DictReader(io.StringIO(csv_text))
    if reader.fieldnames!=['id','phase','title','depends_on']: err('tasks.csv header mismatch')
    rows=list(reader)
    if len(rows)!=len(tasks): err(f'tasks.csv has {len(rows)} rows, expected {len(tasks)}')
    for row,t in zip(rows,tasks):
        if row['id']!=t['id']: err(f'tasks.csv ID order mismatch at {t["id"]}')
        if row['phase']!=t['phase']: err(f'tasks.csv phase mismatch {t["id"]}')
        if row['title']!=t['title']: err(f'tasks.csv title mismatch {t["id"]}')
        if (row['depends_on'] or '').split()!=t.get('depends_on',[]): err(f'tasks.csv dependencies mismatch {t["id"]}')

# Normative limits (R10, design GC-5)
REQUIRED_LIMITS={
    'schema_version':'int',
    'queue.capacity_messages':'int',
    'adapters.max_frame_bytes':'int',
    'adapters.handshake_timeout_ms':'int',
    'adapters.request_deadline_ms':'int',
    'adapters.health_ping_ms':'int',
    'adapters.health_missed_allowed':'int',
    'adapters.supervisor_max_restarts':'int',
    'adapters.supervisor_backoff_initial_ms':'int',
    'adapters.supervisor_backoff_max_ms':'int',
    'effects.lease_ms':'int',
    'effects.lease_renew_ms':'int',
    'runs.claim_ttl_ms':'int',
    'daemon.fence_lease_ms':'int',
    'daemon.fence_renew_ms':'int',
    'approvals.ttl_ms':'int',
    'shutdown.drain_deadline_ms':'int',
    'events.live_buffer_events':'int',
    'events.dispatcher_poll_ms':'int',
    'events.scheduler_poll_ms':'int',
    'streams.read_default_limit':'int',
    'streams.read_max_limit':'int',
    'fs.db_file_mode':'mode',
    'fs.runtime_dir_mode':'mode',
    'fs.control_socket_mode':'mode',
}
try:
    limits=flat(yaml.safe_load((ROOT/'specs/limits.yaml').read_text()))
except Exception as e:
    limits=None; err(f'limits parse failed: {e}')
if limits is not None:
    for key,kind in REQUIRED_LIMITS.items():
        if key not in limits:
            err(f'missing limits key {key}')
        elif kind=='int' and (isinstance(limits[key], bool) or not isinstance(limits[key], int)):
            err(f'limits key {key} is not an integer')
        elif kind=='mode' and not (isinstance(limits[key], str) and re.fullmatch(r'0[0-7]{3}', limits[key])):
            err(f'limits key {key} is not a four-digit octal file mode string')
    if limits.get('schema_version')!=1: err('limits schema_version must be 1')

# Required SQL entities and CHECK constraints (R5, R6, design GC-3)
REQUIRED_TABLES=['kernel_meta','daemon_fence','agent_specs','sessions','tasks','resolved_run_environments',
                 'runs','resolved_bindings','run_graph_heads','run_dependencies','workspace_leases','effects',
                 'resource_reservations','timers','capability_grants','delegation_hops','approval_requests',
                 'approval_responses','adapter_registrations','config_generations','active_config_generation',
                 'idempotency_records','event_stream_heads','outbox_events','artifacts','workspaces','loop_turns',
                 'decisions','adapter_instances','conformance_reports']
REQUIRED_CHECKS=[('runs','state'),('runs','recovery_disposition'),('effects','state'),('effects','effect_class'),
                 ('effects','idempotency_semantics'),('effects','reconciliation_semantics'),
                 ('workspace_leases','mode'),('workspace_leases','enforcement_state'),
                 ('outbox_events','sensitivity'),('outbox_events','retention'),
                 ('adapter_registrations','trust_state'),('adapter_registrations','conformance_state'),
                 ('config_generations','validation_state'),('config_generations','test_state'),
                 ('run_dependencies','dependency_condition')]
sql=(ROOT/'specs/kernel-store-schema.sql').read_text()
blocks={}
for m in re.finditer(r'CREATE TABLE(?: IF NOT EXISTS)?\s+(\w+)\s*\((.*?)\)\s*;', sql, re.S|re.I):
    blocks[m.group(1).lower()]=m.group(2)
for table in REQUIRED_TABLES:
    if table not in blocks: err(f'missing SQL table {table}')
for table,column in REQUIRED_CHECKS:
    body=blocks.get(table)
    if body is None: continue
    if not re.search(rf'^\s*{column}\b[^\n]*\bCHECK\s*\(', body, re.M):
        err(f'missing CHECK constraint on {table}.{column}')
if 'run_edges' in blocks: err('duplicate parent/dependency run_edges table should not exist')

# Event catalog equality (R9)
try:
    catalog=yaml.safe_load((ROOT/'contracts/events/catalog.yaml').read_text())
    catalog_ids=[e['id'] for e in catalog['events']]
    event_md=(ROOT/'specs/event-catalog.md').read_text()
    md_ids=re.findall(r'^\|\s*`([A-Za-z0-9]+)`\s*\|', event_md, re.M)
    if len(catalog_ids)!=len(set(catalog_ids)): err('duplicate event IDs in contracts/events/catalog.yaml')
    if len(md_ids)!=len(set(md_ids)): err('duplicate event IDs in specs/event-catalog.md')
    for event in sorted(set(catalog_ids)-set(md_ids)): err(f'event {event} missing from specs/event-catalog.md')
    for event in sorted(set(md_ids)-set(catalog_ids)): err(f'event {event} missing from contracts/events/catalog.yaml')
except Exception as e: err(f'event catalog validation failed: {e}')

# Contract lock
lock=ROOT/'contracts/contract-lock.sha256'
locked=set()
if not lock.exists(): err('missing contract lock')
else:
    for line in lock.read_text().splitlines():
        if not line.strip(): continue
        parts=line.split()
        if len(parts)<2: err(f'bad lock line {line}'); continue
        expected=parts[0]; relpath=parts[-1]
        if relpath.startswith('./'): relpath=relpath[2:]
        locked.add(relpath)
        p=ROOT/'contracts'/relpath
        if not p.exists(): err(f'locked contract missing {relpath}'); continue
        actual=sha256(p)
        if actual!=expected: err(f'contract digest mismatch {relpath}')
    contract_files={p.relative_to(ROOT/'contracts').as_posix() for p in (ROOT/'contracts').rglob('*') if p.is_file() and p!=lock}
    for path in sorted(contract_files-locked): err(f'contract not covered by lock {path}')

# Manifest count and hashes (R12.2)
manifest_path=ROOT/'MANIFEST.json'
try:
    manifest=json.loads(manifest_path.read_text())
    entries=manifest['files']
    if manifest.get('file_count') not in (len(entries), len(entries)+1):
        err(f'MANIFEST.json file_count {manifest.get("file_count")} does not match {len(entries)} listed files')
    listed=[e['path'] for e in entries]
    if len(listed)!=len(set(listed)): err('MANIFEST.json lists a path twice')
    for entry in entries:
        p=ROOT/entry['path']
        if not p.exists(): err(f'MANIFEST.json lists missing file {entry["path"]}'); continue
        data=p.read_bytes()
        if entry.get('bytes')!=len(data): err(f'MANIFEST.json byte count mismatch for {entry["path"]}')
        if entry.get('sha256')!=hashlib.sha256(data).hexdigest(): err(f'MANIFEST.json digest mismatch for {entry["path"]}')
    disk={rel(p) for p in ROOT.rglob('*') if p.is_file() and p!=manifest_path
          and '__pycache__' not in p.parts and p.name!='.DS_Store'}
    for path in sorted(disk-set(listed)): err(f'file not recorded in MANIFEST.json: {path}')
    for path in sorted(set(listed)-disk): err(f'MANIFEST.json records unknown file {path}')
except Exception as e: err(f'MANIFEST.json validation failed: {e}')

# Duplicate proto symbols (R1.2)
symbols=collections.defaultdict(list)
for p in sorted((ROOT/'contracts').rglob('*.proto')):
    text=re.sub(r'/\*.*?\*/','',p.read_text(),flags=re.S)
    text=re.sub(r'//[^\n]*','',text)
    package=re.search(r'\bpackage\s+([\w.]+)\s*;', text)
    package=package.group(1) if package else ''
    position=0
    while True:
        m=re.search(r'\b(message|service|enum)\s+(\w+)\s*\{', text[position:])
        if not m: break
        start=position+m.start(); body_start=position+m.end()
        depth=1; i=body_start
        while i<len(text) and depth:
            if text[i]=='{': depth+=1
            elif text[i]=='}': depth-=1
            i+=1
        kind=m.group(1); name=m.group(2)
        if kind=='enum':
            for value in re.finditer(r'\b(\w+)\s*=\s*-?\d+', text[body_start:i-1]):
                symbols[(package,'enum value',value.group(1))].append(f'{rel(p)}:{name}')
        else:
            symbols[(package,kind,name)].append(rel(p))
        position=i
for (package,kind,name),locations in sorted(symbols.items()):
    if len(locations)>1:
        err(f'duplicate proto {kind} {name} in package {package or "(none)"}: {", ".join(locations)}')

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

# Status coverage
if tasks is not None:
    try:
        st=yaml.safe_load((ROOT/'TASK_STATUS.yaml').read_text())['tasks']
        if set(st)!=set(ids): err('TASK_STATUS task IDs do not match DAG')
    except Exception as e: err(f'status validation failed: {e}')

if errors:
    print('BUILD PACK INVALID')
    for e in errors: print(' -',e)
    sys.exit(1)
print(f'BUILD PACK OK: {len(ids)} tasks, {len(list(ROOT.rglob("*.md")))} markdown files, contracts locked')
