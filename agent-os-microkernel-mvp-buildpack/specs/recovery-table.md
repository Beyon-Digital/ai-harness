> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)

# Recovery Decision Table

| Persisted condition after restart | Safe action |
|---|---|
| Command absent from idempotency table | Caller may retry command with same request/idempotency key. |
| Command idempotency outcome present | Return stored outcome; do not execute command again. |
| Outbox event not journal-published | Append same event ID/sequence to Event Journal. |
| Outbox event journaled but publication mark absent | Reappend; exact duplicate is success; then mark. |
| Run `Running`, no in-flight effect, old run claim expired | Set `Recovering`, validate frozen loop/adapter availability, reacquire claim/loop epoch, then `Normal`. |
| Run binding adapter digest unavailable | `BlockedMissingResource`; do not silently resolve a replacement. |
| Effect `Prepared` | Safe to claim; external operation was not dispatched by protocol. |
| Effect `Claimed`, lease expired, no `Dispatched` record | Reclaim with higher fencing token. |
| Effect `Dispatched`, reconciliation=Status/Result lookup | Query same operation ID/provider ref. |
| Effect `Dispatched`, idempotency safe and lookup unavailable | Redispatch same operation ID only under configured policy. |
| Effect `Dispatched`, neither reconciliation nor safe idempotency | Mark `Unknown`; block run or require explicit decision. |
| Effect stale executor returns after higher token claimed | Reject authoritative commit; retain diagnostic trace only. |
| Timer `Scheduled` and overdue | Claim normally. |
| Timer `Claimed` by stale daemon/worker | Recover/reclaim according to fencing/lease; do not mark Fired without command outcome. |
| Resource `reserved` with owning run terminal | Release if resource is purely logical; reconcile first if external allocation may exist. |
| Resource external allocation uncertain | Mark `unknown`; do not assume release. |
| Workspace lease owner run terminal | Coordinator may release logical lease after verifying no active effect/process requires it. |
| External process from prior daemon instance | Treat as stale/untrusted; terminate/reconcile; do not accept messages under old daemon epoch. |

## Recovery ordering

Recover authoritative records before issuing new loop turns. Effect/timer/resource ambiguity is resolved before a run's `RecoveryDisposition` returns to `Normal`.
