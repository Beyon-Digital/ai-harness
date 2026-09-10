> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Render Documentation Site

The repository includes `mkdocs.yml` for Material for MkDocs.

```bash
python -m venv .venv
source .venv/bin/activate
pip install mkdocs-material
mkdocs serve
```

`mkdocs build --strict` should be a CI gate along with internal-link and `spec/` schema validation.
