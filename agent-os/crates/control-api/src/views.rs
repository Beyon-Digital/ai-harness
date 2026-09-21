//! Row → contract view mappers for the read/query RPCs (API-002).
#![forbid(unsafe_code)]

use kernel_store::models::{
    AdapterRegistrationRow, ResolvedBindingRow, ResolvedEnvironmentRow, RunDependencyRow, RunRow,
    TaskRow,
};

use domain::generated::contract as generated;

/// `AgentRun` view of a run row.
pub fn run_view(row: &RunRow) -> generated::AgentRun {
    generated::AgentRun {
        run_id: row.run_id.to_string(),
        task_id: row.task_id.to_string(),
        session_id: row.session_id.map(|id| id.to_string()).unwrap_or_default(),
        parent_run_id: row
            .parent_run_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        state: row.state.to_wire(),
        recovery: row.recovery.to_wire(),
        run_revision: row.run_revision,
        loop_epoch: row.loop_epoch,
        step_sequence: row.step_sequence,
        input_event_cursor: row.input_event_cursor.to_string(),
        cancellation_epoch: row.cancellation_epoch,
        resolved_environment_id: row
            .resolved_environment_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
        output_ref: row.output_ref.clone().unwrap_or_default(),
        current_turn_id: row
            .current_turn_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
    }
}

/// `Task` view of a task row.
pub fn task_view(row: &TaskRow) -> generated::Task {
    generated::Task {
        task_id: row.task_id.to_string(),
        session_id: row.session_id.map(|id| id.to_string()).unwrap_or_default(),
        created_by_actor_id: row.created_by_actor_id.to_string(),
        task_kind: row.task_kind.clone(),
        payload: row.payload.clone(),
        created_unix_ms: row.created_at_ms,
    }
}

/// `EffectRecord` view of an effect row.
pub fn effect_view(row: &kernel_store::models::EffectRow) -> generated::EffectRecord {
    generated::EffectRecord {
        effect_id: row.effect_id.to_string(),
        run_id: row.run_id.to_string(),
        step_sequence: row.step_sequence,
        decision_id: row.decision_id.to_string(),
        operation: row.operation.clone(),
        request_hash: row.request_hash.clone(),
        effective_contract: Some(generated::EffectContract {
            effect_class: row.effect_class.to_wire(),
            idempotency: row.idempotency_semantics.to_wire(),
            reconciliation: row.reconciliation_semantics.to_wire(),
            cancellation_semantics: row.cancellation_semantics.clone(),
            compensation_capability: row.compensation_capability.clone().unwrap_or_default(),
        }),
        adapter_id: row.adapter_id.to_string(),
        adapter_version: row.adapter_version.clone(),
        adapter_digest: row.adapter_digest.clone(),
        state: row.state.to_wire(),
        executor_id: row.executor_id.clone().unwrap_or_default(),
        executor_fencing_token: row.executor_fencing_token.unwrap_or(0),
        lease_expires_unix_ms: row.lease_expires_ms.unwrap_or(0),
        result_ref: row.result_ref.clone().unwrap_or_default(),
        error_code: String::new(),
    }
}

/// `ResolvedRunEnvironment` view of an environment row + bindings.
pub fn environment_view(
    row: &ResolvedEnvironmentRow,
    bindings: &[ResolvedBindingRow],
) -> generated::ResolvedRunEnvironment {
    let decode_caps = |raw: &[u8]| -> std::collections::HashMap<String, String> {
        serde_json::from_slice::<Vec<String>>(raw)
            .unwrap_or_default()
            .into_iter()
            .map(|c| (c.clone(), String::new()))
            .collect()
    };
    generated::ResolvedRunEnvironment {
        id: row.environment_id.to_string(),
        run_id: row.run_id.to_string(),
        agent_spec: Some(generated::VersionedRef {
            id: row.agent_spec_id.to_string(),
            version: row.agent_spec_version.clone(),
            digest: row.agent_spec_digest.clone(),
        }),
        agent_loop: Some(generated::VersionedRef {
            id: row.agent_loop_id.clone(),
            version: row.agent_loop_version.clone(),
            digest: row.agent_loop_digest.clone(),
        }),
        config_generation_id: row.config_generation_id.to_string(),
        bindings: bindings
            .iter()
            .map(|b| generated::ResolvedBinding {
                port_id: b.port_id.clone(),
                adapter: Some(generated::VersionedRef {
                    id: b.adapter_id.to_string(),
                    version: b.adapter_version.clone(),
                    digest: b.adapter_digest.clone(),
                }),
                capabilities: decode_caps(&b.capabilities),
            })
            .collect(),
        workspace_uri: row
            .workspace_uri
            .clone()
            .map(|uri| generated::ResourceUri { uri }),
        workspace_base_revision: row.workspace_base_revision.clone().unwrap_or_default(),
        workspace_mode: row.workspace_mode.to_wire(),
        model_provider: row.model_provider.clone().unwrap_or_default(),
        model_id: row.model_id.clone().unwrap_or_default(),
        model_parameters_json: row.model_parameters.clone().unwrap_or_default(),
        kernel_version: row.kernel_version.clone(),
        protocol_versions: serde_json::from_slice::<Vec<u32>>(&row.protocol_versions)
            .unwrap_or_default()
            .into_iter()
            .map(|v| v.to_string())
            .collect(),
        capability_grant_ids: serde_json::from_slice(&row.capability_grant_ids).unwrap_or_default(),
        approval_request_ids: serde_json::from_slice(&row.approval_request_ids).unwrap_or_default(),
    }
}

/// `RunDependency` view.
pub fn dependency_view(row: &RunDependencyRow) -> generated::RunDependency {
    generated::RunDependency {
        dependency_id: row.dependency_id.to_string(),
        task_id: row.task_id.to_string(),
        source_run_id: row.source_run_id.to_string(),
        target_run_id: row.target_run_id.to_string(),
        condition: row.dependency_condition.to_wire(),
        created_graph_revision: row.created_graph_revision,
    }
}

/// `ConfigGeneration` view.
pub fn config_generation_view(
    row: &kernel_store::models::ConfigGenerationRow,
) -> generated::ConfigGeneration {
    generated::ConfigGeneration {
        generation_id: row.generation_id.to_string(),
        digest: row.digest.clone(),
        document: row.document.clone(),
        validation_state: row.validation_state.clone(),
        test_state: row.test_state.clone(),
        created_unix_ms: row.created_at_ms,
    }
}

/// `AdapterRegistrationView` — `implemented_ports`/`capabilities` are
/// stored as JSON blobs.
pub fn adapter_view(row: &AdapterRegistrationRow) -> generated::AdapterRegistrationView {
    let port_ids: Vec<String> =
        serde_json::from_slice::<Vec<serde_json::Value>>(&row.implemented_ports)
            .unwrap_or_default()
            .iter()
            .filter_map(|p| p.get("port_id").and_then(|v| v.as_str()).map(str::to_owned))
            .collect();
    generated::AdapterRegistrationView {
        adapter: Some(generated::VersionedRef {
            id: row.adapter_id.to_string(),
            version: row.version.clone(),
            digest: row.bundle_digest.clone(),
        }),
        manifest_digest: row.manifest_digest.clone(),
        runtime_type: row.runtime_type.clone(),
        implemented_ports: port_ids,
        capabilities_json: row.capabilities.clone(),
        trust_state: row.trust_state.as_str().to_owned(),
        conformance_state: row.conformance_state.as_str().to_owned(),
    }
}
