//! Config parsing, generations, profile resolution, and activation.
#![forbid(unsafe_code)]

pub mod model;

pub mod schema;

pub use model::{ConfigDocument, document_digest, generation_services, parse_document};
pub use profile::{PROFILE_PORTS, ResolvedProfile, resolve_profile};
