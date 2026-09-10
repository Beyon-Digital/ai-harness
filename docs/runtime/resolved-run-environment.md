> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# ResolvedRunEnvironment

Immutable persisted snapshot of the logical execution environment resolved at run start.

## Required metadata

- AgentSpec version/digest.
- AgentLoop ID/version/bundle digest.
- Runtime profile/config generation.
- Every run-scoped adapter ID/version/bundle digest + negotiated capabilities.
- Workspace URI, base snapshot/commit, access mode, lease/delegation metadata.
- Sandbox adapter/trust tier.
- Artifact/memory/context/tool bindings.
- Model provider/model identifier/model parameters/router version.
- Tool/skill bundle digests.
- Security grants, approval request digests, delegation chain.
- Kernel build/version and protocol/spec versions.

This record is the authoritative answer to “what exactly did this run use?”

## Frozen identity versus per-operation facts

“Frozen” means the logical component/bundle/config identity does not silently rebind. It does **not** require embedding long-lived secret plaintext or pretending external providers are immutable.

- Secret material may rotate; each secret action records the concrete secret version/issued credential metadata used at that time.
- A pinned ModelRouter may choose among models according to its frozen policy; each ModelInvocation records the concrete provider/model and provider response metadata.
- Provider model aliases may change outside the system. Record provider snapshot/version identifiers when exposed; otherwise record the exact alias plus provider response metadata and acknowledge the reproducibility limit.
- Stateful resources such as sandbox/workspace are never silently failed over to a different adapter mid-run.
- Any permitted stateless failover must be part of a versioned router/group policy pinned in the resolved environment, and each effect records the actual adapter/provider used.
