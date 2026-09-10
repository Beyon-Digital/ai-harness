> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Crash Recovery

Acquire new daemon fence; load canonical KernelStore state; reconstruct graph/runs; invalidate/reconcile stale executor claims; inspect sandboxes/workspaces/resources; reconcile effects by operation ID; assign RecoveryDisposition; resume only when safe.

Never infer “external operation did not happen” merely because acknowledgement is absent. Non-reconcilable unknown effects remain blocked until policy/human decision.
