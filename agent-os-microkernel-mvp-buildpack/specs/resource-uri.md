> **Navigation:** [Home](../README.md) · [Plan](../PLAN.md) · [DAG](../DAG.md) · [Architecture](../architecture/README.md) · [Specs](../specs/README.md) · [Testing](../testing/README.md)


# Resource URI Resolver

Supported MVP schemes:

```text
workspace://<workspace-id>/<relative-path>
artifact://<artifact-id>
secret://<namespace>/<name>
sandbox://<sandbox-id>
run://<run-id>
task://<task-id>
session://<session-id>
adapter://<adapter-id>@<version>#<digest>
```

The parser must reject path traversal, malformed percent-encoding, empty IDs, and scheme confusion. Callers pass typed parsed URIs internally.

Resolution always checks the current execution context/capabilities before returning backend-native handles.
