# Source Contract Corrections Applied in This Build Pack

The canonical inception docs describe both the configuration schema and extension manifest as version 1, but the source JSON Schemas contained stale `const: 2` values.

For this inception implementation pack:

- `config.schema_version == 1`.
- `extension.manifest_version == 1`.

These are inception-contract consistency corrections only.
