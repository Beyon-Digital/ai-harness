//! Inception extension manifest v1 (`proto/manifests/extension.schema.json`).
//!
//! Parsed strictly: `manifest_version` must be `1`, `kind` must be
//! `adapter`, `runtime.type` `process` (the only spawnable kind in this
//! MVP), and every field is required by the schema's `required` list.
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// `manifest_version` this kernel understands.
pub const MANIFEST_VERSION: u32 = 1;

/// File name of the manifest inside a bundle directory.
pub const MANIFEST_FILE: &str = "adapter.manifest.json";

/// File name of the deterministic bundle lock inside a bundle directory.
pub const LOCK_FILE: &str = "bundle.lock";

/// One `implements` entry: a port id plus its major version.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PortImpl {
    /// Canonical port id, e.g. `workspace`.
    pub port_id: String,
    /// Major version of the port contract.
    pub port_version: u32,
}

/// Runtime section of the manifest.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSection {
    /// `process` is the only spawnable runtime in this MVP.
    #[serde(rename = "type")]
    pub runtime_type: String,
    /// Entrypoint path relative to the bundle root.
    pub entrypoint: String,
    /// Optional implementation language marker.
    #[serde(default)]
    pub language: Option<String>,
}

/// Parsed extension manifest v1.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionManifest {
    /// Must equal [`MANIFEST_VERSION`].
    pub manifest_version: u32,
    /// Adapter id — the registered identity's name component.
    pub id: String,
    /// Version string — the registered identity's version component.
    pub version: String,
    /// Must be `adapter` for registry insertion.
    pub kind: String,
    /// Runtime descriptor.
    pub runtime: RuntimeSection,
    /// Implemented ports as `port_id@major` strings
    /// (schema: `implements` items are strings).
    #[serde(default)]
    pub implements: Vec<String>,
    /// Free-form exported identifiers.
    #[serde(default)]
    pub exports: Vec<String>,
    /// Capability declarations keyed by port family
    /// (`adapter-capabilities.yaml` families: `sandbox`, `workspace`, ...).
    #[serde(default)]
    pub requested_capabilities: BTreeMap<String, Vec<String>>,
}

impl ExtensionManifest {
    /// Flat capability set across families.
    pub fn capabilities(&self) -> Vec<String> {
        self.requested_capabilities
            .values()
            .flatten()
            .cloned()
            .collect()
    }

    /// Parsed `implements` entries — each must be `port_id@major`
    /// (e.g. `workspace@1`). A missing major is a manifest error.
    pub fn implemented_ports(&self) -> errors::Result<Vec<PortImpl>> {
        self.implements
            .iter()
            .map(|raw| {
                let (port_id, major) = raw.rsplit_once('@').ok_or_else(|| {
                    invalid(format!("implements entry '{raw}' lacks a '@major' version"))
                })?;
                let port_version = major.parse::<u32>().map_err(|_| {
                    invalid(format!("implements entry '{raw}' has a non-numeric major"))
                })?;
                Ok(PortImpl {
                    port_id: port_id.to_owned(),
                    port_version,
                })
            })
            .collect()
    }
}

/// Parses manifest bytes; rejects wrong version/kind/runtime and
/// unknown fields.
pub fn parse_manifest(bytes: &[u8]) -> errors::Result<ExtensionManifest> {
    let manifest: ExtensionManifest = serde_json::from_slice(bytes).map_err(|e| {
        KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            "extension manifest is not valid manifest v1 JSON",
        )
        .with_source(e)
    })?;
    if manifest.manifest_version != MANIFEST_VERSION {
        return Err(invalid(format!(
            "manifest_version {} unsupported (kernel understands {MANIFEST_VERSION})",
            manifest.manifest_version
        )));
    }
    if manifest.kind != "adapter" {
        return Err(invalid(format!(
            "manifest kind '{}' is not 'adapter'",
            manifest.kind
        )));
    }
    if manifest.runtime.runtime_type != "process" {
        return Err(invalid(format!(
            "runtime type '{}' is not 'process' (only process bundles are spawnable)",
            manifest.runtime.runtime_type
        )));
    }
    Ok(manifest)
}

/// `sha256:` digest of raw manifest bytes — the persisted
/// `manifest_digest` column.
pub fn manifest_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex(Sha256::digest(bytes)))
}

/// `sha256:<hex>` lowercase helper.
pub(crate) fn hex(digest: impl AsRef<[u8]>) -> String {
    digest.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

fn invalid(message: String) -> KernelError {
    KernelError::new(ErrorCode::InvalidArgument, RetryClass::Never, message)
}
