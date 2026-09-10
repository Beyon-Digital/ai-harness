> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Local-First Deployment

Recommended: Rust `agentd`, SQLite KernelStore, SQLite Event Journal, in-memory queue, macOS Keychain, local Git-worktree workspace, local artifact store, trusted local-process sandbox only for trusted code plus strong container sandbox for generated/untrusted extensions, outbound remote relay, and whichever model adapters are configured.
