//! Capability catalogue, scoped targets, and the deterministic permission
//! engine shared by every component that authorizes an operation.
#![forbid(unsafe_code)]

pub mod capabilities;
pub mod policy;

pub use identity::delegation::{Capability, CapabilityAction, CapabilityFamily};
pub use policy::{ApprovalDraft, Decision, DenyReason, PermissionRequest, ScopedTarget, evaluate};
