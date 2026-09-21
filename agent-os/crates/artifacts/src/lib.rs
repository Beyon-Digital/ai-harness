//! Durable artifact storage with `artifact://` logical URIs.
#![forbid(unsafe_code)]

pub mod local;

pub use local::{LocalArtifactStore, PutMetadata};
