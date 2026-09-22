//! Logical resource URI parsing and capability-checked resolution.
#![forbid(unsafe_code)]

pub mod parser;
pub mod resolver;

pub use parser::ResourceUri;
pub use resolver::{
    CapabilityGate, DenyAllGate, PermitSetGate, ResolvedResource, ResourceResolver,
};
