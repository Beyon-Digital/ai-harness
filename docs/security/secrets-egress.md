> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Secrets and Network Egress

Prefer brokered `SignOrAct` or short-lived audience/scope/run-bound credentials. Raw env/file/FD injection is fallback-only.

Secret and network grants are composed during authorization. Example: GitHub token authority may be constrained to GitHub API destinations; granting arbitrary egress does not automatically widen the secret's allowed audience.

Do not rely on log redaction to prevent deliberate encoding/exfiltration once plaintext is exposed.
