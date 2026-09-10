> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Approval Requests

Approval is bound to immutable request content.

## Digest input

Canonical approval digest includes:

- request ID;
- principal/actor/run;
- operation;
- target resource;
- exact sorted capability request;
- extension bundle digest if applicable;
- config generation digest if applicable;
- expiry;
- nonce.

A response must echo `request_id` + `request_digest`. Mismatch, expiry, resolved request, wrong principal/device, or changed underlying target is rejected.

The MVP local CLI can act as the approval client. Remote device signing is a later layer, but the record format must already preserve device/responder identity.
