//! The validated v1 config model (CFG-001): parse + semantic checks.
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};

pub use crate::schema::ConfigDocument;
use crate::schema::{ProfileSection, SCHEMA_VERSION, ServicesSection};

fn config_error(code: ErrorCode, msg: impl Into<String>) -> KernelError {
    KernelError::new(code, RetryClass::Never, msg.into())
}

/// Parse + validate a v1 YAML document into a [`ConfigDocument`].
///
/// Rejects: `schema_version != 1`, malformed YAML, unknown fields in
/// `kernel`/`services`/`limits` (serde `deny_unknown_fields`), profile
/// `extends` cycles and missing parents, and empty required sections.
pub fn parse_document(yaml: &str) -> errors::Result<ConfigDocument> {
    let doc: ConfigDocument = serde_yaml::from_str(yaml).map_err(|e| {
        config_error(
            ErrorCode::InvalidArgument,
            format!("config document is not valid v1 YAML schema: {e}"),
        )
    })?;
    if doc.schema_version != SCHEMA_VERSION {
        return Err(config_error(
            ErrorCode::InvalidArgument,
            format!(
                "unsupported schema_version {} (expected {SCHEMA_VERSION})",
                doc.schema_version
            ),
        ));
    }
    validate_profiles(&doc.profiles)?;
    Ok(doc)
}

/// Cycle + missing-parent check for `extends` chains.
fn validate_profiles(profiles: &BTreeMap<String, ProfileSection>) -> errors::Result<()> {
    for (name, profile) in profiles {
        if let Some(parent) = &profile.extends
            && !profiles.contains_key(parent)
        {
            return Err(config_error(
                ErrorCode::InvalidArgument,
                format!("profile '{name}' extends unknown profile '{parent}'"),
            ));
        }
    }
    for name in profiles.keys() {
        let mut seen = BTreeSet::new();
        let mut cursor = name.as_str();
        while let Some(next) = profiles[cursor].extends.as_deref() {
            if !seen.insert(next) {
                return Err(config_error(
                    ErrorCode::InvalidArgument,
                    format!("profile inheritance cycle at '{name}' (revisits '{next}')"),
                ));
            }
            cursor = next;
        }
    }
    Ok(())
}

/// The generation-global services block — the daemon-frozen comparison
/// surface for activation.
pub fn generation_services(doc: &ConfigDocument) -> ServicesSection {
    doc.services.clone()
}

/// sha256 hex digest of the exact document bytes.
pub fn document_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    format!("sha256:{out}")
}
