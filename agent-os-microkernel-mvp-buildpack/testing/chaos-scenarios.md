> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Crash and Chaos Scenarios

Inject a process abort/fault at these boundaries and assert recovery:

1. before KernelStore transaction;
2. after canonical row write but before commit;
3. after commit but before command response;
4. after outbox commit before Event Journal append;
5. after Event Journal append before outbox publication mark;
6. after live publish before client acknowledgement;
7. after effect `Prepared`;
8. after effect claim;
9. after `Dispatched` before adapter call;
10. after adapter side effect succeeds but before response received;
11. after response received before `Acknowledged` commit;
12. after `Acknowledged` before owning run transition commit;
13. during parent cancellation while child spawn races;
14. after timer claim while cancel races;
15. during workspace exclusive lease transfer;
16. after config generation test before activation CAS;
17. process adapter crashes during handshake;
18. daemon loses/changes fencing epoch while stale worker returns.

Every test must specify whether safe outcome is retry, reconciliation, `Unknown`, rejection, or deterministic recovery.
