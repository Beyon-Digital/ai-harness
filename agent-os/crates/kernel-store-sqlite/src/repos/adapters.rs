//! Adapter repository: registrations, instances, and conformance reports.
//!
//! Registrations and conformance reports are immutable rows; the durable
//! adapter instance is the only mutable record and changes through a state
//! CAS (R3.2).

use async_trait::async_trait;
use domain::ids::{AdapterId, AdapterInstanceId};
use domain::run::UnknownStateValue;
use domain::security::{ConformanceState, TrustState};
use kernel_store::models::{
    AdapterInstanceStatePatch, AdapterRegistrationRow, ConformanceReportRow, NewAdapterInstance,
    NewAdapterRegistration, NewConformanceReport,
};
use kernel_store::repositories::{AdapterRead, AdapterRepo};
use sqlx::sqlite::SqliteRow;

use crate::mapping;
use crate::repos::SharedConn;

/// SQLite view over the adapter tables.
pub(crate) struct SqliteAdapterRepo {
    conn: SharedConn,
}

impl SqliteAdapterRepo {
    /// Creates a repository view over a transaction connection.
    pub(crate) fn new(conn: SharedConn) -> Self {
        Self { conn }
    }
}

/// Parses the exact `adapter_instances.state` CHECK literal set.
fn instance_state_from_state(value: &str) -> Result<String, UnknownStateValue> {
    match value {
        "starting" | "ready" | "exited" | "failed" => Ok(value.to_owned()),
        _ => Err(UnknownStateValue {
            value: value.to_owned(),
            enum_name: "AdapterInstanceState",
        }),
    }
}

const SELECT_REGISTRATION: &str = "SELECT adapter_id, version, bundle_digest, manifest_digest, \
     runtime_type, implemented_ports, capabilities, trust_state, conformance_state, created_at_ms \
     FROM adapter_registrations";

fn decode_registration(row: &SqliteRow) -> errors::Result<AdapterRegistrationRow> {
    Ok(AdapterRegistrationRow {
        adapter_id: mapping::decode_id(
            "adapter_registrations.adapter_id",
            &mapping::text(row, "adapter_id")?,
        )?,
        version: mapping::text(row, "version")?,
        bundle_digest: mapping::text(row, "bundle_digest")?,
        manifest_digest: mapping::text(row, "manifest_digest")?,
        runtime_type: mapping::text(row, "runtime_type")?,
        implemented_ports: mapping::blob(row, "implemented_ports")?,
        capabilities: mapping::blob(row, "capabilities")?,
        trust_state: mapping::decode_state(
            "adapter_registrations.trust_state",
            &mapping::text(row, "trust_state")?,
            TrustState::from_state_str,
        )?,
        conformance_state: mapping::decode_state(
            "adapter_registrations.conformance_state",
            &mapping::text(row, "conformance_state")?,
            ConformanceState::from_state_str,
        )?,
        created_at_ms: mapping::int(row, "created_at_ms")?,
    })
}

fn decode_conformance_report(row: &SqliteRow) -> errors::Result<ConformanceReportRow> {
    Ok(ConformanceReportRow {
        adapter_id: mapping::decode_id(
            "conformance_reports.adapter_id",
            &mapping::text(row, "adapter_id")?,
        )?,
        adapter_version: mapping::text(row, "adapter_version")?,
        bundle_digest: mapping::text(row, "bundle_digest")?,
        report_digest: mapping::text(row, "report_digest")?,
        harness_version: mapping::text(row, "harness_version")?,
        result: mapping::text(row, "result")?,
        run_at_ms: mapping::int(row, "run_at_ms")?,
        details: mapping::opt_blob(row, "details")?,
    })
}

#[async_trait]
impl AdapterRead for SqliteAdapterRepo {
    async fn get_registration(
        &mut self,
        adapter_id: AdapterId,
        version: &str,
        bundle_digest: &str,
    ) -> errors::Result<Option<AdapterRegistrationRow>> {
        let mut guard = self.conn.lock().await;
        let query = format!(
            "{SELECT_REGISTRATION} WHERE adapter_id = ?1 AND version = ?2 AND bundle_digest = ?3"
        );
        let row = sqlx::query(&query)
            .bind(adapter_id.to_string())
            .bind(version)
            .bind(bundle_digest)
            .fetch_optional(guard.connection()?)
            .await
            .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_registration).transpose()
    }

    async fn get_conformance_report(
        &mut self,
        adapter_id: AdapterId,
        version: &str,
        bundle_digest: &str,
    ) -> errors::Result<Option<ConformanceReportRow>> {
        let mut guard = self.conn.lock().await;
        let row = sqlx::query(
            "SELECT adapter_id, adapter_version, bundle_digest, report_digest, harness_version, \
             result, run_at_ms, details FROM conformance_reports \
             WHERE adapter_id = ?1 AND adapter_version = ?2 AND bundle_digest = ?3",
        )
        .bind(adapter_id.to_string())
        .bind(version)
        .bind(bundle_digest)
        .fetch_optional(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        row.as_ref().map(decode_conformance_report).transpose()
    }
}

#[async_trait]
impl AdapterRepo for SqliteAdapterRepo {
    async fn insert_registration(
        &mut self,
        registration: NewAdapterRegistration,
    ) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO adapter_registrations (adapter_id, version, bundle_digest, \
             manifest_digest, runtime_type, implemented_ports, capabilities, trust_state, \
             conformance_state, created_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )
        .bind(registration.adapter_id.to_string())
        .bind(registration.version)
        .bind(registration.bundle_digest)
        .bind(registration.manifest_digest)
        .bind(registration.runtime_type)
        .bind(registration.implemented_ports)
        .bind(registration.capabilities)
        .bind(registration.trust_state.as_str())
        .bind(registration.conformance_state.as_str())
        .bind(registration.created_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn insert_instance(&mut self, instance: NewAdapterInstance) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO adapter_instances (adapter_instance_id, adapter_id, adapter_version, \
             bundle_digest, daemon_instance_id, pid, process_start_identity, state, exit_reason, \
             last_heartbeat_ms, started_at_ms, ended_at_ms) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )
        .bind(instance.adapter_instance_id.to_string())
        .bind(instance.adapter_id.to_string())
        .bind(instance.adapter_version)
        .bind(instance.bundle_digest)
        .bind(instance.daemon_instance_id.to_string())
        .bind(instance.pid)
        .bind(instance.process_start_identity)
        .bind(instance.state)
        .bind(instance.exit_reason)
        .bind(instance.last_heartbeat_ms)
        .bind(instance.started_at_ms)
        .bind(instance.ended_at_ms)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }

    async fn cas_instance_state(
        &mut self,
        id: AdapterInstanceId,
        expect_state: &str,
        patch: AdapterInstanceStatePatch,
    ) -> errors::Result<bool> {
        let mut guard = self.conn.lock().await;
        let conn = guard.connection()?;

        // Validate the persisted literal before mutating; an unknown value
        // fails closed instead of being compared as an opaque string.
        let persisted: Option<String> = sqlx::query_scalar(
            "SELECT state FROM adapter_instances WHERE adapter_instance_id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(&mut *conn)
        .await
        .map_err(mapping::from_sqlx)?;
        let Some(persisted) = persisted else {
            return Ok(false);
        };
        mapping::decode_state(
            "adapter_instances.state",
            &persisted,
            instance_state_from_state,
        )?;

        let result = sqlx::query(
            "UPDATE adapter_instances SET \
             state = COALESCE(?1, state), \
             exit_reason = COALESCE(?2, exit_reason), \
             last_heartbeat_ms = COALESCE(?3, last_heartbeat_ms), \
             ended_at_ms = COALESCE(?4, ended_at_ms) \
             WHERE adapter_instance_id = ?5 AND state = ?6",
        )
        .bind(patch.state)
        .bind(patch.exit_reason)
        .bind(patch.last_heartbeat_ms)
        .bind(patch.ended_at_ms)
        .bind(id.to_string())
        .bind(expect_state)
        .execute(conn)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(result.rows_affected() == 1)
    }

    async fn insert_conformance_report(
        &mut self,
        report: NewConformanceReport,
    ) -> errors::Result<()> {
        let mut guard = self.conn.lock().await;
        sqlx::query(
            "INSERT INTO conformance_reports (adapter_id, adapter_version, bundle_digest, \
             report_digest, harness_version, result, run_at_ms, details) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(report.adapter_id.to_string())
        .bind(report.adapter_version)
        .bind(report.bundle_digest)
        .bind(report.report_digest)
        .bind(report.harness_version)
        .bind(report.result)
        .bind(report.run_at_ms)
        .bind(report.details)
        .execute(guard.connection()?)
        .await
        .map_err(mapping::from_sqlx)?;
        Ok(())
    }
}
