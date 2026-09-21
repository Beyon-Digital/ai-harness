//! Conformance reports and the activation gate (`specs/adapter-registry.md`).
//!
//! A `ConformanceReport` is bound to the exact registration identity
//! `(adapter_id, version, bundle_digest)`; persisting a `pass`/`fail`
//! reflects onto `adapter_registrations.conformance_state`. The resolver
//! excludes `Failed` registrations and can additionally require `Passed`
//! (the "required-review" gate).
#![forbid(unsafe_code)]

use domain::ids::AdapterId;
use domain::security::ConformanceState;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::NewConformanceReport;
use kernel_store::txn::KernelTxn;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::hex;

/// Version of the conformance harness that produced a report.
pub const HARNESS_VERSION: &str = "agentos-conformance/1";

/// One machine-readable case outcome.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaseResult {
    /// Stable test identifier (e.g. `protocol.handshake_identity`).
    pub test_id: String,
    /// Whether the case passed.
    pub passed: bool,
    /// Human-readable detail/assertion output.
    pub details: String,
}

/// A conformance report bound to one registration identity.
#[derive(Clone, Debug)]
pub struct ConformanceReport {
    /// Registered adapter id.
    pub adapter_id: AdapterId,
    /// Registered version.
    pub adapter_version: String,
    /// Registered bundle digest — the report is void for any other digest.
    pub bundle_digest: String,
    /// Per-case results.
    pub cases: Vec<CaseResult>,
    /// `pass` iff every case passed.
    pub result: String,
    /// Digest over the canonical report content.
    pub report_digest: String,
    /// Harness version string.
    pub harness_version: String,
    /// Wall-clock run time.
    pub run_at_ms: i64,
}

impl ConformanceReport {
    /// Builds a report (computing `result` + `report_digest`) from cases.
    pub fn from_cases(
        adapter_id: AdapterId,
        adapter_version: &str,
        bundle_digest: &str,
        cases: Vec<CaseResult>,
        run_at_ms: i64,
    ) -> Self {
        let result = if cases.iter().all(|c| c.passed) {
            "pass".to_owned()
        } else {
            "fail".to_owned()
        };
        let details = serde_json::to_vec(&cases).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(b"agentos.conformance.v1\n");
        hasher.update(adapter_id.to_string().as_bytes());
        hasher.update(b"\n");
        hasher.update(adapter_version.as_bytes());
        hasher.update(b"\n");
        hasher.update(bundle_digest.as_bytes());
        hasher.update(b"\n");
        hasher.update(&details);
        Self {
            adapter_id,
            adapter_version: adapter_version.to_owned(),
            bundle_digest: bundle_digest.to_owned(),
            cases,
            result,
            report_digest: format!("sha256:{}", hex(hasher.finalize())),
            harness_version: HARNESS_VERSION.to_owned(),
            run_at_ms,
        }
    }
}

/// Persists the report row and reflects its outcome onto the
/// registration's `conformance_state` — one transaction, so the gate can
/// never observe a report without its state.
pub async fn persist_report(
    txn: &mut dyn KernelTxn,
    report: &ConformanceReport,
) -> errors::Result<()> {
    txn.adapters()
        .insert_conformance_report(NewConformanceReport {
            adapter_id: report.adapter_id,
            adapter_version: report.adapter_version.clone(),
            bundle_digest: report.bundle_digest.clone(),
            report_digest: report.report_digest.clone(),
            harness_version: report.harness_version.clone(),
            result: report.result.clone(),
            run_at_ms: report.run_at_ms,
            details: Some(serde_json::to_vec(&report.cases).map_err(|e| {
                KernelError::new(
                    ErrorCode::Internal,
                    RetryClass::Never,
                    "conformance report details are not serializable",
                )
                .with_source(e)
            })?),
        })
        .await?;
    let state = if report.result == "pass" {
        ConformanceState::Passed
    } else {
        ConformanceState::Failed
    };
    txn.adapters()
        .set_conformance_state(
            report.adapter_id,
            &report.adapter_version,
            &report.bundle_digest,
            state,
        )
        .await
}
