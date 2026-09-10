> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Built-in Native Adapters

Recommended initial trusted adapters: SQLite KernelStore, SQLite Event Journal, in-memory queue, trusted local-process sandbox (T0 only), strong local container sandbox, local filesystem/Git-worktree workspace, local artifact store, macOS Keychain secret store, local transport, and at least one OpenAI-compatible model adapter.

Built-ins implement the same public port semantics; third parties should not need secret private APIs to be functionally equivalent.
