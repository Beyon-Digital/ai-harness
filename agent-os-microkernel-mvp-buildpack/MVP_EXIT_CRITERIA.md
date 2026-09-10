# MVP Exit Criteria

The microkernel/control-plane MVP is complete only when all of the following pass:

1. Two `agentd` instances cannot concurrently own writer authority for the same local store.
2. Command replay with the same idempotency key/request digest returns the stored outcome without repeating mutation; same key/different digest is rejected.
3. A crash cannot leave canonical state committed without the corresponding outbox event records from the same command transaction.
4. Re-running the outbox publisher is safe and preserves per-stream event sequence/idempotency.
5. A live durable subscriber never receives a resumable cursor before the Event Journal contains that event.
6. Concurrent RunGraph edge insertions cannot create an undetected cycle.
7. Ready-run claim is single-winner under concurrency.
8. Parent cancellation racing child spawn cannot produce an uncancelled escaped child.
9. A stale loop decision is rejected on any revision/epoch/step/cursor mismatch.
10. Two effect executors cannot authoritatively commit the same effect under different fences.
11. A simulated crash after dispatch but before acknowledgement produces reconciled outcome or `Unknown`, never an unconditional duplicate dispatch.
12. Timer fire/cancel race has exactly one linearized winner.
13. Child resource delegation cannot exceed the parent's remaining budget.
14. Capability delegation cannot amplify ancestor authority.
15. An approval response with a mismatched request digest is rejected.
16. A T2 sandbox request is rejected when only T0 is installed.
17. Exclusive workspace transfer invalidates/enforces the previous writer lease.
18. Parallel forks can diverge independently and explicit merge can return useful child work to the parent.
19. Started runs retain identical persisted resolved bindings after a new config generation becomes active.
20. Live config activation that changes a daemon-instance-frozen generation-global binding is rejected without changing the active pointer.
21. External adapter handshake fails on wrong bundle digest, protocol version, or bootstrap identity.
22. Drain-first shutdown stops new work before final persistence close and does not accept late authoritative child results after fencing is lost.
23. End-to-end fixture flow works: create session/task/run → issue loop turn → accept decision → create child/effect → complete → query run/effects/events.
24. `cargo fmt`, clippy with warnings denied, unit tests, integration tests, concurrency tests, and chaos/recovery tests all pass.
