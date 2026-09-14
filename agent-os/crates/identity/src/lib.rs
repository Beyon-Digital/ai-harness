//! Principal, actor, and delegation types.
#![forbid(unsafe_code)]

pub mod daemon;
pub mod delegation;

pub use delegation::load_grant_scopes;
