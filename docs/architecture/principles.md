> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Design Principles

1. **Mechanism over policy in Rust.** Planning/context/memory behavior is not kernel policy.
2. **Abstract only genuine substitution boundaries.** No interface-per-function architecture.
3. **Canonical semantics, variable implementations.** Adapters cannot weaken a port silently.
4. **Capability negotiation, not vendor conditionals.** Requirements are semantic.
5. **Single transactional authority.** KernelStore is the correctness center.
6. **Effect uncertainty is explicit.** `Unknown` is not “failed.”
7. **Process/sandbox isolation for untrusted code.** Native plugins are trusted-only.
8. **Logical resource URIs.** Workflows do not depend on physical paths/buckets.
9. **Immutable run bindings.** Audit/reproduction never consults mutable current config to infer history.
10. **Config is transactional.** Test proposed generations before activation and preserve the previous healthy generation for safe config rollback.
11. **Authority is delegatable but cannot be amplified.** Parent → child → tool chains are explicit.
12. **Replay means reconstruction.** Never promise deterministic re-execution of LLM/web/API calls.
