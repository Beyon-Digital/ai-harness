//! Sandbox tier resolution and the T0 local-process adapter.
#![forbid(unsafe_code)]

pub mod local_process;
pub mod manager;

pub use local_process::{ExecOutcome, ExecSpec, ExecStatus};
pub use manager::{SandboxInstance, exec_in_workspace, resolve_sandbox};
