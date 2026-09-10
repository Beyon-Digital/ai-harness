from pathlib import Path
import json,re,sys
try:
    import yaml
except ImportError:
    print('PyYAML required for validation', file=sys.stderr); sys.exit(2)
root=Path(__file__).resolve().parents[1]
errors=[]
for p in root.rglob('*.md'):
    for target in re.findall(r'\[[^\]]+\]\(([^)]+)\)',p.read_text(encoding='utf-8')):
        if target.startswith(('http://','https://','#','mailto:')): continue
        t=target.split('#',1)[0]
        if t and not (p.parent/t).resolve().exists(): errors.append(f'broken link: {p.relative_to(root)} -> {t}')
for p in root.rglob('*.json'):
    try: json.loads(p.read_text())
    except Exception as e: errors.append(f'invalid JSON {p.relative_to(root)}: {e}')
for p in root.rglob('*.yaml'):
    try: yaml.safe_load(p.read_text())
    except Exception as e: errors.append(f'invalid YAML {p.relative_to(root)}: {e}')
catalog=yaml.safe_load((root/'spec/catalog.yaml').read_text())
for pid,meta in catalog['ports'].items():
    doc=root/'docs/ports'/f'{pid}.md'
    if not doc.exists(): errors.append(f'missing port doc for {pid}')
    else:
        txt=doc.read_text()
        if f'`{pid}@{meta["version"]}`' not in txt: errors.append(f'port doc version mismatch: {pid}')
if errors:
    print('\n'.join(errors)); sys.exit(1)
print(f'OK: {len(list(root.rglob("*.md")))} markdown, {len(catalog["ports"])} canonical ports, no link/schema/catalog errors')
