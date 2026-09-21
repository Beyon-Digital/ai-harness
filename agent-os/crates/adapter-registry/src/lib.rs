//! Content-addressed adapter registry + capability resolver.
#![forbid(unsafe_code)]

pub mod capabilities;
pub mod conformance;
pub mod manifest;
pub mod registry;
pub mod resolver;

pub use capabilities::{CapabilitySet, REQUIRED_FOR_T2, SandboxTier};
pub use conformance::{CaseResult, ConformanceReport, HARNESS_VERSION, persist_report};
pub use manifest::{
    ExtensionManifest, LOCK_FILE, MANIFEST_FILE, MANIFEST_VERSION, PortImpl, manifest_digest,
    parse_manifest,
};
pub use registry::{
    LockEntry, VerifiedBundle, compute_bundle_digest, parse_lock, register, verify_for_spawn,
    write_lock,
};
pub use resolver::{Candidate, PortRequirement, ResolvedAdapter, resolve};
