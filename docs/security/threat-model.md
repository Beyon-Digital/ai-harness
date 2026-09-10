> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Threat Model

Primary threats: prompt-injected or malicious generated code, compromised adapters/dependencies, secret exfiltration, workspace escape, host-socket abuse, confused-deputy delegation, duplicate external effects after crashes, stale daemon/worker split-brain, mutable-extension TOCTOU, spoofed local adapter identity, forged/replayed remote approvals, memory poisoning, and telemetry leakage.

Trust domains: audited kernel/native adapters; user-installed external adapters; generated extensions; remote/untrusted content; cloud relay; remote client devices.
