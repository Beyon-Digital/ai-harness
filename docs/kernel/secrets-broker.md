> **Navigation:** [Home](../index.md) · [Architecture](../architecture/README.md) · [Kernel](../kernel/README.md) · [Runtime](../runtime/README.md) · [Ports](../ports/README.md) · [Adapters](../adapters/README.md) · [Extensions](../extensions/README.md) · [Configuration](../configuration/README.md) · [Security](../security/README.md) · [Operations](../operations/README.md) · [API](../api/README.md) · [Implementation Plan](../../PLAN.md)

# Secrets Broker

Prefer brokered actions over raw credential disclosure.

## Preferred modes

- `SignOrAct`: broker performs/signs the target request.
- Short-lived audience/scope/run-bound credentials.
- Raw secret injection only when unavoidable and explicitly permitted.

## Joint authority evaluation

Secret access and egress authority are evaluated together. A secret that can only be used with `api.github.com` must not be combined with unrestricted egress by accident.

## Audit

Every secret operation records principal, actor, run, delegation chain, target audience/destination, secret reference/version, and approval/grant IDs without logging secret material.
