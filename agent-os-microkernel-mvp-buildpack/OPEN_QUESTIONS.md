# Open Questions

No blocking architectural questions are open at pack creation time.

Coding agents append here only when a task exposes a genuinely unspecified behavior that cannot be resolved from `DECISIONS.md`, `/specs`, or `/contracts`.

## EFF-001 cancellation_semantics literal

- **Task:** EFF-001
- **Contract:** `contracts/domain/effects.proto` `EffectContract.cancellation`
- **Interpretation:** freeform string field. Chose the closed vocabulary
  `before_dispatch | cooperative | provider_specific | unsupported` mirroring
  proto comments; unknown/absent resolves to `unsupported` (most conservative).
- **Blocking:** no — resolver treats unrecognized values as `unsupported`.
