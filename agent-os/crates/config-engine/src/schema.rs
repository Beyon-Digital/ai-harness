//! Inception config schema (CFG-001): the typed v1 document.
//!
//! `kernel` and `services` are security-sensitive — unknown fields there
//! are rejected (`deny_unknown_fields`). Profile bodies accept extra
//! fields (adapter-specific keys), but the known slot names are typed.
#![forbid(unsafe_code)]

use serde::Deserialize;
use std::collections::BTreeMap;

/// The only inception version this engine loads.
pub const SCHEMA_VERSION: u64 = 1;

/// Top-level v1 document.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ConfigDocument {
    /// Must be `1`.
    pub schema_version: u64,
    /// Bootstrap-global: store + daemon identity. Not changeable via
    /// runtime profiles or generation activation.
    pub kernel: KernelSection,
    /// Generation-global service bindings — frozen for one daemon
    /// instance; changing them requires `DAEMON_RESTART_REQUIRED`.
    #[serde(default)]
    pub services: ServicesSection,
    /// Run-scoped profiles referenced by `requested_profile`.
    pub profiles: BTreeMap<String, ProfileSection>,
    /// Legacy untyped policy bag (accepted, not interpreted).
    #[serde(default)]
    pub policies: Option<serde_yaml::Mapping>,
    /// Normative thresholds — every key must be present.
    pub limits: LimitsSection,
    /// Observability tuning (log level etc.).
    #[serde(default)]
    pub observability: Option<BTreeMap<String, serde_yaml::Value>>,
}

/// `kernel:` — bootstrap-global bindings.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct KernelSection {
    /// KernelStore selection — immutable at runtime.
    pub store: String,
    /// Daemon identity label.
    #[serde(default)]
    pub daemon_identity: Option<String>,
}

/// `services:` — generation-global service bindings.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ServicesSection {
    /// Event journal binding.
    pub event_journal: Option<String>,
    /// Message queue binding.
    pub message_queue: Option<String>,
    /// Secret store binding.
    pub secret_store: Option<String>,
    /// Transport/control binding.
    pub transport: Option<String>,
}

/// A run-scoped profile; `extends` chains resolve in `model.rs`.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct ProfileSection {
    /// Parent profile name.
    pub extends: Option<String>,
    /// Per-port adapter bindings (`id@version` or registered names).
    pub sandbox: Option<String>,
    /// Workspace adapter binding.
    pub workspace: Option<String>,
    /// Artifact store binding.
    pub artifact_store: Option<String>,
    /// Memory store binding.
    pub memory_store: Option<String>,
    /// Model provider binding.
    pub model_provider: Option<String>,
    /// Tool runtime binding.
    pub tool_runtime: Option<String>,
    /// Agent loop binding.
    pub agent_loop: Option<String>,
    /// `effect.execute` port binding (adapter `id@version`).
    pub effect_execute: Option<String>,
}

/// `limits:` — every normative key from `limits.yaml`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LimitsSection {
    /// Queue capacity.
    pub queue: QueueLimits,
    /// Adapter protocol limits.
    pub adapters: AdapterLimits,
    /// Effect lease limits.
    pub effects: EffectLimits,
    /// Run claim limits.
    pub runs: RunLimits,
    /// Daemon fence limits.
    pub daemon: DaemonLimits,
    /// Approval TTL.
    pub approvals: ApprovalLimits,
    /// Shutdown drain.
    pub shutdown: ShutdownLimits,
    /// Event bus limits.
    pub events: EventLimits,
    /// Stream read limits.
    pub streams: StreamLimits,
    /// Filesystem permission bits.
    pub fs: FsLimits,
}

/// `limits.queue`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QueueLimits {
    /// Max queued messages.
    pub capacity_messages: u64,
}

/// `limits.adapters`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AdapterLimits {
    /// Frame byte cap.
    pub max_frame_bytes: u64,
    /// Handshake timeout.
    pub handshake_timeout_ms: u64,
    /// Request deadline.
    pub request_deadline_ms: u64,
    /// Health ping interval.
    pub health_ping_ms: u64,
    /// Missed pings tolerated.
    pub health_missed_allowed: u64,
    /// Supervisor restart cap.
    pub supervisor_max_restarts: u64,
    /// Restart backoff floor.
    pub supervisor_backoff_initial_ms: u64,
    /// Restart backoff ceiling.
    pub supervisor_backoff_max_ms: u64,
}

/// `limits.effects`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EffectLimits {
    /// Effect lease duration.
    pub lease_ms: u64,
    /// Lease renew interval.
    pub lease_renew_ms: u64,
}

/// `limits.runs`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RunLimits {
    /// Claim TTL.
    pub claim_ttl_ms: u64,
}

/// `limits.daemon`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DaemonLimits {
    /// Fence lease duration.
    pub fence_lease_ms: u64,
    /// Fence renew interval.
    pub fence_renew_ms: u64,
}

/// `limits.approvals`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ApprovalLimits {
    /// Approval TTL.
    pub ttl_ms: u64,
}

/// `limits.shutdown`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ShutdownLimits {
    /// Drain deadline.
    pub drain_deadline_ms: u64,
}

/// `limits.events`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EventLimits {
    /// Live buffer capacity.
    pub live_buffer_events: u64,
    /// Dispatcher poll interval.
    pub dispatcher_poll_ms: u64,
    /// Scheduler poll interval.
    pub scheduler_poll_ms: u64,
}

/// `limits.streams`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StreamLimits {
    /// Default read page size.
    pub read_default_limit: u64,
    /// Max read page size.
    pub read_max_limit: u64,
}

/// `limits.fs`.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FsLimits {
    /// DB file mode string.
    pub db_file_mode: String,
    /// Runtime dir mode string.
    pub runtime_dir_mode: String,
    /// Control socket mode string.
    pub control_socket_mode: String,
}
