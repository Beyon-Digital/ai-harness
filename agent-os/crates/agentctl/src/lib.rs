//! Local operator CLI that talks only to the public Control/Event APIs
//! over the daemon's Unix socket — never to the store directly.
//!
//! Every command renders a `serde_json::Value`, so output is stable JSON
//! for automation regardless of the command.

#![forbid(unsafe_code)]

use std::path::Path;

use domain::generated::contract::{
    self, ActivateConfigGeneration, ApprovalResponseRequest, CancelRun, CommandRequest,
    CreateSession, CreateTaskRun, GetActiveConfigRequest, GetEffectRequest, GetRunGraphRequest,
    GetRunRequest, GetRunResponse, GetTaskRequest, HealthRequest, ListAdaptersRequest,
    ListApprovalsRequest, MarkConfigTested, ProposeConfigGeneration, PutAgentSpecRevision,
    ReadEventStreamRequest, ResolveUnknownEffect, RollbackConfigGeneration, SubscribeEventsRequest,
};
use domain::ids::{ActorId, CommandId, IdempotencyKey};
use domain::provider::SystemIdProvider;
use prost::Message;
use serde_json::{Value, json};

use control_api::event_service::generated::mvp_event_api_client::MvpEventApiClient;
use control_api::generated::mvp_control_api_client::MvpControlApiClient;

/// CLI failure surfaced as JSON on stderr with a non-zero exit code.
#[derive(Debug)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CliError {}

fn err(message: impl Into<String>) -> CliError {
    CliError(message.into())
}

/// Both API clients share one UDS channel.
pub struct Daemon {
    /// Control API client.
    pub control: MvpControlApiClient<tonic::transport::Channel>,
    /// Event API client.
    pub events: MvpEventApiClient<tonic::transport::Channel>,
}

/// Connects to the daemon socket at `path`.
pub async fn connect(path: &Path) -> Result<Daemon, CliError> {
    let socket = path.to_path_buf();
    let channel = tonic::transport::Endpoint::from_static("http://[::1]:0")
        .connect_with_connector(tower::service_fn(move |_| {
            let socket = socket.clone();
            async move {
                tokio::net::UnixStream::connect(socket)
                    .await
                    .map(hyper_util::rt::TokioIo::new)
                    .map_err(std::io::Error::other)
            }
        }))
        .await
        .map_err(|e| err(format!("cannot reach daemon at {}: {e}", path.display())))?;
    Ok(Daemon {
        control: MvpControlApiClient::new(channel.clone()),
        events: MvpEventApiClient::new(channel),
    })
}

struct Flags {
    positional: Vec<String>,
    options: std::collections::BTreeMap<String, String>,
    switches: std::collections::BTreeSet<String>,
}

fn parse_flags(args: &[String]) -> Result<Flags, CliError> {
    let mut flags = Flags {
        positional: Vec::new(),
        options: Default::default(),
        switches: Default::default(),
    };
    let mut it = args.iter().peekable();
    while let Some(arg) = it.next() {
        if let Some(name) = arg.strip_prefix("--") {
            if let Some((name, value)) = name.split_once('=') {
                flags.options.insert(name.to_owned(), value.to_owned());
            } else if it.peek().is_some_and(|v| !v.starts_with("--")) {
                flags
                    .options
                    .insert(name.to_owned(), it.next().expect("peeked").clone());
            } else {
                flags.switches.insert(name.to_owned());
            }
        } else {
            flags.positional.push(arg.clone());
        }
    }
    Ok(flags)
}

impl Flags {
    fn opt(&self, name: &str) -> Option<&str> {
        self.options.get(name).map(String::as_str)
    }
    fn req(&self, name: &str) -> Result<&str, CliError> {
        self.opt(name)
            .ok_or_else(|| err(format!("missing required --{name}")))
    }
    fn at(&self, index: usize) -> Result<&str, CliError> {
        self.positional
            .get(index)
            .map(String::as_str)
            .ok_or_else(|| err(format!("missing positional argument {index}")))
    }
    fn at_or(&self, index: usize) -> String {
        self.positional.get(index).cloned().unwrap_or_default()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn read_file_arg(value: &str) -> Result<Vec<u8>, CliError> {
    if let Some(rest) = value.strip_prefix('@') {
        std::fs::read(rest).map_err(|e| err(format!("read {rest}: {e}")))
    } else if value == "-" {
        use std::io::Read;
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|e| err(format!("read stdin: {e}")))?;
        Ok(buf)
    } else {
        std::fs::read(value).map_err(|e| err(format!("read {value}: {e}")))
    }
}

/// One command envelope carrying a typed contract payload.
fn envelope(command_type: &str, payload: Vec<u8>, key: &str) -> CommandRequest {
    let ids = SystemIdProvider;
    let digest = sha256_hex(format!("{command_type}\n{key}").as_bytes());
    CommandRequest {
        command_id: CommandId::new(&ids).to_string(),
        idempotency_key: IdempotencyKey::new(key)
            .map(|k| k.to_string())
            .unwrap_or_else(|_| format!("key-{}", sha256_hex(key.as_bytes()))),
        principal_id: String::new(), // resolved from peer credentials
        actor_id: ActorId::new(&ids).to_string(),
        device_id: String::new(),
        request_digest: digest,
        correlation_id: String::new(),
        causation_id: String::new(),
        deadline_unix_ms: 0,
        command_type: command_type.to_owned(),
        payload,
    }
}

async fn submit(
    daemon: &mut Daemon,
    command_type: &str,
    payload: impl Message,
    key: &str,
) -> Result<Value, CliError> {
    let request = envelope(command_type, payload.encode_to_vec(), key);
    let response = daemon
        .control
        .submit_command(request)
        .await
        .map_err(status_err)?
        .into_inner();
    Ok(json!({
        "command_id": response.command_id,
        "outcome_code": response.outcome_code,
        "payload": String::from_utf8_lossy(&response.payload),
    }))
}

fn status_err(status: tonic::Status) -> CliError {
    err(format!("{}: {}", status.code(), status.message()))
}

/// Runs one parsed command and returns its JSON result.
pub async fn run(args: &[String], socket: &Path) -> Result<Value, CliError> {
    let flags = parse_flags(args)?;
    let command = flags.at(0).map(str::to_owned)?;
    let mut daemon = connect(socket).await?;
    dispatch(&mut daemon, &command, &flags).await
}

async fn dispatch(daemon: &mut Daemon, command: &str, flags: &Flags) -> Result<Value, CliError> {
    match command {
        "health" => {
            let r = daemon
                .control
                .health(HealthRequest {})
                .await
                .map_err(status_err)?
                .into_inner();
            Ok(json!({
                "status": r.status,
                "daemon_instance_id": r.daemon_instance_id,
                "daemon_fencing_epoch": r.daemon_fencing_epoch,
                "active_config_generation_id": r.active_config_generation_id,
                "outbox_unpublished_count": r.outbox_unpublished_count,
            }))
        }
        "create-session" => {
            let metadata = flags
                .opt("metadata")
                .map(read_file_arg)
                .transpose()?
                .unwrap_or_default();
            let key = format!("session.{}", sha256_hex(&metadata));
            submit(
                daemon,
                "agentos.spec.v1.CreateSession",
                CreateSession {
                    session_id: flags.at_or(1),
                    metadata,
                },
                &key,
            )
            .await
        }
        "put-agent-spec" => {
            let body = read_file_arg(flags.req("body")?)?;
            submit(
                daemon,
                "agentos.spec.v1.PutAgentSpecRevision",
                PutAgentSpecRevision {
                    agent_spec_id: flags.at_or(1),
                    version: flags.req("version")?.to_owned(),
                    body_bytes: body.clone(),
                    body_digest: sha256_hex(&body),
                },
                &format!("spec.{}.{}", flags.at_or(1), flags.req("version")?),
            )
            .await
        }
        "create-run" => {
            let task_payload = flags
                .opt("payload")
                .map(read_file_arg)
                .transpose()?
                .unwrap_or_default();
            submit(
                daemon,
                "agentos.spec.v1.CreateTaskRun",
                CreateTaskRun {
                    task_id: flags.opt("task-id").unwrap_or_default().to_owned(),
                    run_id: flags.opt("run-id").unwrap_or_default().to_owned(),
                    session_id: flags.req("session-id")?.to_owned(),
                    task_kind: flags.req("task-kind")?.to_owned(),
                    task_payload,
                    agent_spec_ref: match (flags.opt("agent-spec-id"), flags.opt("spec-version")) {
                        (Some(id), Some(version)) => Some(contract::VersionedRef {
                            id: id.to_owned(),
                            version: version.to_owned(),
                            digest: flags.opt("spec-digest").unwrap_or_default().to_owned(),
                        }),
                        _ => None,
                    },
                    parent_run_id: flags.opt("parent-run-id").unwrap_or_default().to_owned(),
                    observed_parent_cancellation_epoch: 0,
                    requested_profile: flags.opt("profile").unwrap_or_default().to_owned(),
                    workspace_uri: flags.opt("workspace-uri").unwrap_or_default().to_owned(),
                    requested_capabilities: flags
                        .opt("capabilities")
                        .map(|v| v.split(',').map(str::to_owned).collect())
                        .unwrap_or_default(),
                    requested_budget: Vec::new(),
                },
                &format!(
                    "run.{}.{}",
                    flags.req("session-id")?,
                    sha256_hex(&serde_json::to_vec(&flags.positional).unwrap_or_default())
                ),
            )
            .await
        }
        "get-run" => {
            let r = daemon
                .control
                .get_run(GetRunRequest {
                    run_id: flags.at(1)?.to_owned(),
                })
                .await
                .map_err(status_err)?
                .into_inner();
            Ok(run_json(r))
        }
        "get-task" => {
            let r = daemon
                .control
                .get_task(GetTaskRequest {
                    task_id: flags.at(1)?.to_owned(),
                })
                .await
                .map_err(status_err)?
                .into_inner();
            let t = r.task.ok_or_else(|| err("empty task"))?;
            Ok(json!({
                "task_id": t.task_id, "session_id": t.session_id,
                "created_by_actor_id": t.created_by_actor_id,
            }))
        }
        "graph" => {
            let r = daemon
                .control
                .get_run_graph(GetRunGraphRequest {
                    task_id: flags.at(1)?.to_owned(),
                })
                .await
                .map_err(status_err)?
                .into_inner();
            let graph = r.graph.ok_or_else(|| err("empty graph"))?;
            Ok(json!({
                "task_id": graph.task_id,
                "graph_revision": graph.graph_revision,
                "runs": graph.runs.iter().map(|r| run_json(GetRunResponse {
                    run: Some(r.clone()),
                    resolved_environment: None,
                })).collect::<Vec<_>>(),
                "dependencies": graph.dependencies.iter().map(|d| json!({
                    "dependency_id": d.dependency_id, "task_id": d.task_id,
                    "source_run_id": d.source_run_id, "target_run_id": d.target_run_id,
                    "condition": d.condition,
                })).collect::<Vec<_>>(),
            }))
        }
        "cancel-run" => {
            submit(
                daemon,
                "agentos.spec.v1.CancelRun",
                CancelRun {
                    run_id: flags.at(1)?.to_owned(),
                    expected_run_revision: flags
                        .opt("expected-revision")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(u64::MAX),
                    reason: flags.opt("reason").unwrap_or_default().to_owned(),
                },
                &format!("cancel.{}", flags.at(1)?),
            )
            .await
        }
        "get-effect" => {
            let r = daemon
                .control
                .get_effect(GetEffectRequest {
                    effect_id: flags.at(1)?.to_owned(),
                })
                .await
                .map_err(status_err)?
                .into_inner();
            let e = r.effect.ok_or_else(|| err("empty effect"))?;
            Ok(json!({
                "effect_id": e.effect_id, "run_id": e.run_id, "step_sequence": e.step_sequence,
                "operation": e.operation, "state": e.state, "adapter_id": e.adapter_id,
                "result_ref": e.result_ref, "error_code": e.error_code,
                "executor_fencing_token": e.executor_fencing_token,
            }))
        }
        "resolve-effect" => {
            submit(
                daemon,
                "agentos.spec.v1.ResolveUnknownEffect",
                ResolveUnknownEffect {
                    effect_id: flags.req("effect-id")?.to_owned(),
                    expected_effect_state: flags
                        .opt("expected-state")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(8),
                    action: flags.req("action")?.to_owned(),
                    result_ref: flags.opt("result-ref").unwrap_or_default().to_owned(),
                    reason: flags.opt("reason").unwrap_or_default().to_owned(),
                    approval_request_id: flags
                        .opt("approval-request-id")
                        .unwrap_or_default()
                        .to_owned(),
                },
                &format!("resolve.{}", flags.req("effect-id")?),
            )
            .await
        }
        "adapters" => {
            let r = daemon
                .control
                .list_adapters(ListAdaptersRequest {
                    port_id: flags.opt("port-id").unwrap_or_default().to_owned(),
                })
                .await
                .map_err(status_err)?
                .into_inner();
            Ok(json!({
                "adapters": r.adapters.iter().map(|a| json!({
                    "adapter": a.adapter.as_ref().map(|v| json!({"id": v.id, "version": v.version})),
                    "manifest_digest": a.manifest_digest,
                    "runtime_type": a.runtime_type,
                    "implemented_ports": a.implemented_ports,
                    "trust_state": a.trust_state,
                    "conformance_state": a.conformance_state,
                })).collect::<Vec<_>>(),
            }))
        }
        "config" => config_cmd(daemon, flags).await,
        "approvals" => approvals_cmd(daemon, flags).await,
        "events" => events_cmd(daemon, flags).await,
        other => Err(err(format!(
            "unknown command '{other}'; expected health|create-session|put-agent-spec|create-run|get-run|graph|cancel-run|get-effect|resolve-effect|adapters|config|approvals|events"
        ))),
    }
}

async fn config_cmd(daemon: &mut Daemon, flags: &Flags) -> Result<Value, CliError> {
    match flags.at(1)? {
        "show" => {
            let r = daemon
                .control
                .get_active_config(GetActiveConfigRequest {})
                .await
                .map_err(status_err)?
                .into_inner();
            let g = r.generation.ok_or_else(|| err("empty generation"))?;
            Ok(json!({
                "generation_id": g.generation_id,
                "digest": g.digest,
                "validation_state": g.validation_state,
                "test_state": g.test_state,
                "active_revision": r.active_revision,
                "document": String::from_utf8_lossy(&g.document),
            }))
        }
        "propose" => {
            let document = read_file_arg(flags.req("file")?)?;
            submit(
                daemon,
                "agentos.spec.v1.ProposeConfigGeneration",
                ProposeConfigGeneration {
                    generation_id: String::new(),
                    document_bytes: document.clone(),
                    digest: format!("sha256:{}", sha256_hex(&document)),
                },
                &format!("config.propose.{}", sha256_hex(&document)),
            )
            .await
        }
        "test" => {
            submit(
                daemon,
                "agentos.spec.v1.MarkConfigTested",
                MarkConfigTested {
                    generation_id: flags.req("generation-id")?.to_owned(),
                    digest: flags.opt("digest").unwrap_or_default().to_owned(),
                    test_report_digest: String::new(),
                    test_result: "passed".to_owned(),
                },
                &format!("config.test.{}", flags.req("generation-id")?),
            )
            .await
        }
        "activate" => {
            submit(
                daemon,
                "agentos.spec.v1.ActivateConfigGeneration",
                ActivateConfigGeneration {
                    generation_id: flags.req("generation-id")?.to_owned(),
                    expected_active_revision: flags
                        .opt("expected-revision")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                },
                &format!("config.activate.{}", flags.req("generation-id")?),
            )
            .await
        }
        "rollback" => {
            submit(
                daemon,
                "agentos.spec.v1.RollbackConfigGeneration",
                RollbackConfigGeneration {
                    generation_id: flags.req("generation-id")?.to_owned(),
                    expected_active_revision: flags
                        .opt("expected-revision")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0),
                    reason: flags.opt("reason").unwrap_or_default().to_owned(),
                },
                &format!("config.rollback.{}", flags.req("generation-id")?),
            )
            .await
        }
        other => Err(err(format!(
            "unknown config command '{other}'; expected show|propose|test|activate|rollback"
        ))),
    }
}

async fn approvals_cmd(daemon: &mut Daemon, flags: &Flags) -> Result<Value, CliError> {
    match flags.at(1)? {
        "list" => {
            let r = daemon
                .control
                .list_approvals(ListApprovalsRequest {
                    run_id: flags.opt("run-id").unwrap_or_default().to_owned(),
                })
                .await
                .map_err(status_err)?
                .into_inner();
            Ok(json!({
                "approvals": r.approvals.iter().map(|a| json!({
                    "request_id": a.request_id, "request_digest": a.request_digest,
                    "principal_id": a.principal_id, "actor_id": a.actor_id,
                    "run_id": a.run_id, "operation": a.operation,
                    "target_resource": a.target_resource, "capability_ids": a.capability_ids,
                    "expires_at_ms": a.expires_at_ms, "state": a.state,
                })).collect::<Vec<_>>(),
            }))
        }
        "respond" => {
            let r = daemon
                .control
                .respond_approval(ApprovalResponseRequest {
                    request_id: flags.req("request-id")?.to_owned(),
                    request_digest: flags.req("digest")?.to_owned(),
                    decision: flags.req("decision")?.to_owned(),
                    device_id: flags.req("device-id")?.to_owned(),
                    responder_principal_id: String::new(),
                })
                .await
                .map_err(status_err)?
                .into_inner();
            Ok(json!({"status": r.status}))
        }
        other => Err(err(format!(
            "unknown approvals command '{other}'; expected list|respond"
        ))),
    }
}

async fn events_cmd(daemon: &mut Daemon, flags: &Flags) -> Result<Value, CliError> {
    match flags.at(1)? {
        "read" => {
            let r = daemon
                .events
                .read_stream(ReadEventStreamRequest {
                    stream_key: flags.req("stream-key")?.to_owned(),
                    from_sequence: flags.opt("from").and_then(|v| v.parse().ok()).unwrap_or(0),
                    limit: flags
                        .opt("limit")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(256),
                })
                .await
                .map_err(status_err)?
                .into_inner();
            Ok(json!({
                "retention_gap": r.retention_gap,
                "events": r.events.iter().map(event_json).collect::<Vec<_>>(),
            }))
        }
        "tail" => {
            let count: usize = flags
                .opt("count")
                .and_then(|v| v.parse().ok())
                .unwrap_or(10);
            let timeout_s: u64 = flags
                .opt("timeout")
                .and_then(|v| v.parse().ok())
                .unwrap_or(30);
            let mut stream = daemon
                .events
                .subscribe(SubscribeEventsRequest {
                    stream_key: flags.req("stream-key")?.to_owned(),
                    after_sequence: flags.opt("after").and_then(|v| v.parse().ok()).unwrap_or(0),
                })
                .await
                .map_err(status_err)?
                .into_inner();
            let mut out = Vec::new();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_s);
            while out.len() < count {
                let item = tokio::time::timeout_at(
                    tokio::time::Instant::from_std(deadline),
                    stream.message(),
                )
                .await;
                match item {
                    Ok(Ok(Some(frame))) => {
                        use contract::event_stream_frame::Frame;
                        match frame.frame {
                            Some(Frame::Event(e)) => out.push(event_json(&e)),
                            Some(Frame::Lag(l)) => out.push(json!({
                                "lag": true, "stream_key": l.stream_key,
                                "resume_sequence": l.resume_sequence,
                            })),
                            None => {}
                        }
                    }
                    Ok(Ok(None)) => break,
                    Ok(Err(status)) => return Err(status_err(status)),
                    Err(_) => break,
                }
            }
            Ok(json!({ "frames": out }))
        }
        other => Err(err(format!(
            "unknown events command '{other}'; expected read|tail"
        ))),
    }
}

fn run_json(r: contract::GetRunResponse) -> Value {
    let run = r.run.unwrap_or_default();
    json!({
        "run_id": run.run_id,
        "task_id": run.task_id,
        "session_id": run.session_id,
        "parent_run_id": run.parent_run_id,
        "state": run.state,
        "run_revision": run.run_revision,
        "loop_epoch": run.loop_epoch,
        "step_sequence": run.step_sequence,
        "resolved_environment_id": run.resolved_environment_id,
        "output_ref": run.output_ref,
        "resolved_environment": r.resolved_environment.map(|e| json!({
            "id": e.id, "config_generation_id": e.config_generation_id,
            "kernel_version": e.kernel_version,
            "bindings": e.bindings.iter().map(|b| json!({
                "port_id": b.port_id,
                "adapter": b.adapter.as_ref().map(|v| json!({"id": v.id, "version": v.version})),
            })).collect::<Vec<_>>(),
        })),
    })
}

fn event_json(e: &contract::EventEnvelope) -> Value {
    json!({
        "event_id": e.event_id,
        "event_type": e.event_type,
        "stream_key": e.stream_key,
        "sequence": e.sequence,
        "occurred_unix_ms": e.occurred_unix_ms,
        "sensitivity": e.sensitivity,
        "payload_len": e.payload.len(),
    })
}
