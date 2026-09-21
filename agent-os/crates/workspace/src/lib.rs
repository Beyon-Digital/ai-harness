//! Workspace coordination, leases, and the local adapter.
#![forbid(unsafe_code)]

pub mod git;
pub mod local;

pub use local::{
    AdapterCapabilities, capabilities, fork, kind, list, read, record_lease, record_workspace,
    remove, resolve_relative, write,
};
