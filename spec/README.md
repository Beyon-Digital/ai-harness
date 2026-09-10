# Canonical Machine-Readable Specification

`spec/` is the normative inception contract for public IDs, schemas, protocol fields, port versions, event names, extension manifests, capability names, and configuration shapes. The initial specification version is `1`.

Human-readable docs explain semantics but must not introduce contradictory wire contracts. SDKs, JSON/OpenAPI output, conformance fixtures, and generated reference docs should be derived from this tree and validated for freshness in CI.

## Layout

- `catalog.yaml` — canonical identifiers and binding scopes.
- `domain/*.proto` — core records/enums.
- `ports/*.proto` — port service contracts.
- `events/` — event envelope and catalog.
- `control-api/` — client command/query service.
- `protocols/` — external adapter, loop, and effect protocol messages.
- `config/*.schema.json` — configuration JSON Schema.
- `manifests/*.schema.json` — extension manifest JSON Schema.
- `capabilities/*.yaml` — capability/trust-tier catalogs.
