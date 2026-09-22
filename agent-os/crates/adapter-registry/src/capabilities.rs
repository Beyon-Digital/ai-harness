//! Canonical capability catalog (`proto/capabilities/adapter-capabilities.yaml`)
//! and the sandbox-tier capability gates.
#![forbid(unsafe_code)]

use std::collections::BTreeSet;

/// Sandbox trust tier a run requests (`specs/sandbox.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SandboxTier {
    /// Trusted local process — not a security boundary.
    T0,
    /// Constrained (not required for MVP).
    T1,
    /// Untrusted — kernel contract enforced.
    T2,
    /// Hostile — out of scope for the MVP.
    T3,
}

/// Capabilities the canonical catalog lists as `required_for_t2`.
/// Kept in lockstep with `proto/capabilities/adapter-capabilities.yaml`
/// — a test parses the YAML and asserts this set.
pub const REQUIRED_FOR_T2: &[&str] = &[
    "host_fs_default_deny",
    "network_default_deny",
    "host_socket_deny",
    "process_tree_containment",
    "cpu_limit",
    "memory_limit",
    "pid_limit",
    "disk_quota",
    "wall_clock_limit",
    "descendant_cleanup",
];

/// A parsed capability set (adapter-declared or requirement-specified).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapabilitySet {
    set: BTreeSet<String>,
}

impl CapabilitySet {
    /// Builds from raw capability names.
    pub fn new(capabilities: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            set: capabilities.into_iter().map(Into::into).collect(),
        }
    }

    /// Every required capability is present.
    pub fn satisfies(&self, required: &[String]) -> bool {
        required.iter().all(|r| self.set.contains(r))
    }

    /// The adapter can host the requested sandbox tier: T2/T3 demands
    /// every `required_for_t2` capability (fail closed — never silently
    /// fall back to a weaker tier).
    pub fn hosts_tier(&self, tier: SandboxTier) -> bool {
        match tier {
            SandboxTier::T0 | SandboxTier::T1 => true,
            SandboxTier::T2 | SandboxTier::T3 => self.satisfies(
                &REQUIRED_FOR_T2
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect::<Vec<_>>(),
            ),
        }
    }

    /// Sorted capability names.
    pub fn names(&self) -> Vec<String> {
        self.set.iter().cloned().collect()
    }
}
