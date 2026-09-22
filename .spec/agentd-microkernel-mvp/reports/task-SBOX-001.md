# Task SBOX-001 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** SBOX-001 — Sandbox manager: tier resolution, T0 local exec, coordinator-gated workspace access
- **Status:** DONE
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `sandbox/src/manager.rs`: `resolve_sandbox` maps `SandboxTier::T0` to
  the local process adapter (`is_security_boundary: false`) and T1/T2/T3
  to the registry resolver — none pass, so higher tiers fail closed as
  `CAPABILITY_UNSUPPORTED` (`FailedPrecondition`, no T2→T0 fallback).
- `sandbox/src/local_process.rs`: `exec` under an explicit env
  allowlist + cwd + deadline + cooperative cancel; stdout/stderr
  captured and drained past `output_cap_bytes` with a `truncated` flag
  (no writer deadlock). The child runs in its own process group
  (`process_group(0)`); timeout/cancel escalates to
  `kill_process_group(SIGKILL)` so orphaned grandchildren cannot hold
  the pipe open.
- `exec_in_workspace` calls `coordinator::verify_write_authority`
  before spawning — a revoked or stale-epoch lease blocks exec.

## Evidence

`cargo test -p sandbox` (5 tests): T0 exec captures stdout/stderr and
exit status; deadline + watch-channel cancel both terminate the process
group promptly; env allowlist is exact (child sees only granted vars);
T1/T2/T3 all fail closed; revoked lease blocks workspace exec.
