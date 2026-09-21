//! Run-scoped profile resolution (CFG-003 step 1): `extends` chains
//! flattened into one binding map, child keys overriding parents.
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};

use crate::model::ConfigDocument;

/// Port slots a profile can bind.
pub const PROFILE_PORTS: &[&str] = &[
    "sandbox",
    "workspace",
    "artifact_store",
    "memory_store",
    "model_provider",
    "tool_runtime",
    "agent_loop",
];

/// A fully resolved profile: `extends` flattened in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResolvedProfile {
    /// Final per-port bindings (child overrides parent).
    pub bindings: BTreeMap<String, String>,
}

/// Resolve `name`'s bindings: root-most ancestors apply first, child
/// overrides by key. Missing profile -> `NotFound`.
pub fn resolve_profile(doc: &ConfigDocument, name: &str) -> errors::Result<ResolvedProfile> {
    if !doc.profiles.contains_key(name) {
        return Err(KernelError::new(
            ErrorCode::NotFound,
            RetryClass::Never,
            format!("profile '{name}' not defined"),
        ));
    }
    let mut chain = vec![name];
    let mut cursor = name;
    let mut seen = BTreeSet::from([cursor]);
    while let Some(parent) = doc.profiles[cursor].extends.as_deref() {
        if !seen.insert(parent) {
            return Err(KernelError::new(
                ErrorCode::InvalidArgument,
                RetryClass::Never,
                format!("profile inheritance cycle at '{name}'"),
            ));
        }
        chain.push(parent);
        cursor = parent;
    }
    let mut bindings = BTreeMap::new();
    for name in chain.into_iter().rev() {
        let p = &doc.profiles[name];
        for (slot, value) in [
            ("sandbox", &p.sandbox),
            ("workspace", &p.workspace),
            ("artifact_store", &p.artifact_store),
            ("memory_store", &p.memory_store),
            ("model_provider", &p.model_provider),
            ("tool_runtime", &p.tool_runtime),
            ("agent_loop", &p.agent_loop),
        ] {
            if let Some(v) = value {
                bindings.insert(slot.to_owned(), v.clone());
            }
        }
    }
    Ok(ResolvedProfile { bindings })
}
