//! `MvpControlApi` service over the local UDS endpoint.
//!
//! Authentication is the kernel's local identity model: the OS tells us who
//! connected (`SO_PEERCRED` uid) and a configured map resolves the uid to a
//! principal — never a self-asserted principal header. A `principal_id`
//! claim in the command body that disagrees with the peer's mapped principal
//! is rejected, so handlers see only verified identity. Handlers never see
//! a raw `KernelStore`: the write path goes through `CommandCoordinator`,
//! and read endpoints for API-002 take a narrow query port (not the store).
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::task::{Context, Poll};

use command_coordinator::CommandCoordinator;
use command_coordinator::envelope::CommandEnvelope;
use command_coordinator::envelope::RequestDigest;
use domain::ids::{CommandId, DeviceId, EffectId, IdempotencyKey, PrincipalId, RunId, TaskId};
use domain::provider::IdProvider;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::KernelStore;
use prost::Message;
use tokio::net::UnixStream;
use tonic::transport::server::Connected;
use tonic::{Code, Request, Response, Status};

use crate::uds::ControlSocket;

/// Generated service stubs; message types resolve to `domain`'s contract.
pub mod generated {
    tonic::include_proto!("agentos.spec.v1");
}

use domain::generated::contract::{
    ApprovalResponseRequest, ApprovalResponseResult, CommandRequest, CommandResponse,
    GetActiveConfigRequest, GetActiveConfigResponse, GetEffectRequest, GetEffectResponse,
    GetRunGraphRequest, GetRunGraphResponse, GetRunRequest, GetRunResponse, GetTaskRequest,
    GetTaskResponse, HealthRequest, HealthResponse, ListAdaptersRequest, ListAdaptersResponse,
};
use generated::mvp_control_api_server::{MvpControlApi, MvpControlApiServer};

/// OS-verified peer identity attached to each accepted connection.
#[derive(Clone, Copy, Debug)]
pub struct PeerCreds {
    /// Peer user id from `SO_PEERCRED`.
    pub uid: u32,
    /// Peer process id.
    pub pid: Option<u32>,
}

/// A UDS connection carrying its verified peer credentials.
#[derive(Debug)]
pub struct UdsConn {
    stream: UnixStream,
    creds: PeerCreds,
}

impl Connected for UdsConn {
    type ConnectInfo = PeerCreds;

    fn connect_info(&self) -> PeerCreds {
        self.creds
    }
}

impl tokio::io::AsyncRead for UdsConn {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for UdsConn {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
}

/// Maps a peer uid to the configured local principal (API auth boundary).
pub trait PeerPrincipalMap: Send + Sync {
    /// The principal owning this uid, or `None` for an unknown peer.
    fn principal_for_uid(&self, uid: u32) -> Option<PrincipalId>;
}

/// Static uid → principal map from daemon config.
#[derive(Debug, Default)]
pub struct UidPrincipalMap {
    principals: HashMap<u32, PrincipalId>,
}

impl UidPrincipalMap {
    /// Builds the map; duplicate uids fail closed.
    pub fn new(entries: impl IntoIterator<Item = (u32, PrincipalId)>) -> errors::Result<Self> {
        let mut principals = HashMap::new();
        for (uid, principal) in entries {
            if principals.insert(uid, principal).is_some() {
                return Err(KernelError::new(
                    ErrorCode::InvalidArgument,
                    RetryClass::Never,
                    format!("uid {uid} is mapped to two principals"),
                ));
            }
        }
        Ok(Self { principals })
    }
}

impl PeerPrincipalMap for UidPrincipalMap {
    fn principal_for_uid(&self, uid: u32) -> Option<PrincipalId> {
        self.principals.get(&uid).copied()
    }
}

/// Verified local caller bound to a connection — the bootstrap actor
/// context every handler resolves before dispatch.
#[derive(Clone, Copy, Debug)]
pub struct LocalActor {
    /// Kernel principal resolved from the peer uid.
    pub principal_id: PrincipalId,
}

/// Static daemon metadata surfaced by `Health`.
#[derive(Clone, Debug)]
pub struct HealthInfo {
    /// `running`/`draining` literal.
    pub status: String,
    /// Owning daemon instance.
    pub daemon_instance_id: String,
    /// Daemon fencing epoch.
    pub daemon_fencing_epoch: u64,
    /// Active config generation id, if any.
    pub active_config_generation_id: String,
    /// Unpublished outbox events.
    pub outbox_unpublished_count: u64,
}

/// The control service: coordinator for writes, read-only queries through
/// the store's `begin_read` port — handlers never mutate repositories
/// directly.
pub struct ControlApiService {
    coordinator: Arc<CommandCoordinator>,
    store: Arc<dyn KernelStore>,
    ids: Arc<dyn IdProvider>,
    principals: Arc<dyn PeerPrincipalMap>,
    health: HealthInfo,
}

impl ControlApiService {
    /// Binds the service to the coordinator (commands), the read port
    /// (queries), and the peer map (auth).
    pub fn new(
        coordinator: Arc<CommandCoordinator>,
        store: Arc<dyn KernelStore>,
        ids: Arc<dyn IdProvider>,
        principals: Arc<dyn PeerPrincipalMap>,
        health: HealthInfo,
    ) -> Self {
        Self {
            coordinator,
            store,
            ids,
            principals,
            health,
        }
    }

    /// Resolves the verified local actor for this request, then enforces
    /// that any claimed `principal_id` matches the peer's principal.
    fn actor_for<T>(&self, request: &Request<T>, claimed: &str) -> Result<LocalActor, Status> {
        let creds = request
            .extensions()
            .get::<PeerCreds>()
            .copied()
            .ok_or_else(|| Status::unauthenticated("missing peer credentials"))?;
        let principal = self
            .principals
            .principal_for_uid(creds.uid)
            .ok_or_else(|| Status::permission_denied("peer uid has no mapped principal"))?;
        if !claimed.is_empty() {
            let claimed = PrincipalId::from_str(claimed)
                .map_err(|_| Status::invalid_argument("principal_id is malformed"))?;
            if claimed != principal {
                return Err(Status::permission_denied(
                    "principal_id does not match the peer principal",
                ));
            }
        }
        Ok(LocalActor {
            principal_id: principal,
        })
    }
}

fn parse_id<T: FromStr>(field: &str, value: &str) -> Result<T, Status>
where
    T::Err: std::fmt::Display,
{
    T::from_str(value).map_err(|e| Status::invalid_argument(format!("{field} is malformed: {e}")))
}

fn unavailable(e: KernelError) -> Status {
    let code = match e.code() {
        ErrorCode::InvalidArgument => Code::InvalidArgument,
        ErrorCode::NotFound => Code::NotFound,
        ErrorCode::Conflict => Code::AlreadyExists,
        ErrorCode::FailedPrecondition => Code::FailedPrecondition,
        ErrorCode::ResourceExhausted => Code::ResourceExhausted,
        ErrorCode::Unavailable => Code::Unavailable,
        ErrorCode::Internal => Code::Internal,
    };
    Status::new(code, e.to_string())
}

#[async_trait::async_trait]
impl MvpControlApi for ControlApiService {
    async fn submit_command(
        &self,
        request: Request<CommandRequest>,
    ) -> Result<Response<CommandResponse>, Status> {
        let actor = self.actor_for(&request, &request.get_ref().principal_id)?;
        let req = request.into_inner();
        let envelope = CommandEnvelope {
            command_id: parse_id("command_id", &req.command_id)?,
            idempotency_key: IdempotencyKey::from_str(&req.idempotency_key)
                .map_err(|_| Status::invalid_argument("idempotency_key is malformed"))?,
            principal_id: actor.principal_id,
            actor_id: parse_id("actor_id", &req.actor_id)?,
            device_id: if req.device_id.is_empty() {
                None
            } else {
                Some(
                    DeviceId::from_str(&req.device_id)
                        .map_err(|_| Status::invalid_argument("device_id is malformed"))?,
                )
            },
            delegation_chain_id: None,
            request_digest: parse_id("request_digest", &req.request_digest)?,
            correlation_id: if req.correlation_id.is_empty() {
                None
            } else {
                Some(req.correlation_id)
            },
            causation_id: if req.causation_id.is_empty() {
                None
            } else {
                Some(req.causation_id)
            },
            deadline_unix_ms: if req.deadline_unix_ms == 0 {
                None
            } else {
                Some(req.deadline_unix_ms)
            },
            command_type: req.command_type,
            payload: req.payload,
        };
        let command_id = envelope.command_id;
        let outcome = self
            .coordinator
            .execute(envelope)
            .await
            .map_err(unavailable)?;
        Ok(Response::new(CommandResponse {
            command_id: command_id.to_string(),
            outcome_code: outcome.code.as_str().to_owned(),
            payload: outcome.payload,
        }))
    }

    async fn health(
        &self,
        request: Request<HealthRequest>,
    ) -> Result<Response<HealthResponse>, Status> {
        // Health also resolves the actor — an unknown uid gets nothing.
        let _actor = self.actor_for(&request, "")?;
        Ok(Response::new(HealthResponse {
            status: self.health.status.clone(),
            daemon_instance_id: self.health.daemon_instance_id.clone(),
            daemon_fencing_epoch: self.health.daemon_fencing_epoch,
            active_config_generation_id: self.health.active_config_generation_id.clone(),
            outbox_unpublished_count: self.health.outbox_unpublished_count,
        }))
    }

    async fn get_run(
        &self,
        request: Request<GetRunRequest>,
    ) -> Result<Response<GetRunResponse>, Status> {
        let _actor = self.actor_for(&request, "")?;
        let run_id = parse_id::<RunId>("run_id", &request.get_ref().run_id)?;
        let mut txn = self.store.begin_read().await.map_err(unavailable)?;
        let run = txn
            .runs()
            .get(run_id)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| Status::not_found("run not found"))?;
        let environment = match run.resolved_environment_id {
            Some(env_id) => {
                let row = txn
                    .environments()
                    .get_environment(env_id)
                    .await
                    .map_err(unavailable)?;
                match row {
                    Some(row) => {
                        let bindings = txn
                            .environments()
                            .get_bindings(env_id)
                            .await
                            .map_err(unavailable)?;
                        Some(crate::views::environment_view(&row, &bindings))
                    }
                    None => None,
                }
            }
            None => None,
        };
        Ok(Response::new(GetRunResponse {
            run: Some(crate::views::run_view(&run)),
            resolved_environment: environment,
        }))
    }

    async fn get_task(
        &self,
        request: Request<GetTaskRequest>,
    ) -> Result<Response<GetTaskResponse>, Status> {
        let _actor = self.actor_for(&request, "")?;
        let task_id = parse_id::<TaskId>("task_id", &request.get_ref().task_id)?;
        let mut txn = self.store.begin_read().await.map_err(unavailable)?;
        let task = txn
            .tasks()
            .get(task_id)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| Status::not_found("task not found"))?;
        Ok(Response::new(GetTaskResponse {
            task: Some(crate::views::task_view(&task)),
        }))
    }

    async fn get_effect(
        &self,
        request: Request<GetEffectRequest>,
    ) -> Result<Response<GetEffectResponse>, Status> {
        let _actor = self.actor_for(&request, "")?;
        let effect_id = parse_id::<EffectId>("effect_id", &request.get_ref().effect_id)?;
        let mut txn = self.store.begin_read().await.map_err(unavailable)?;
        let effect = txn
            .effects()
            .get(effect_id)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| Status::not_found("effect not found"))?;
        Ok(Response::new(GetEffectResponse {
            effect: Some(crate::views::effect_view(&effect)),
        }))
    }

    async fn get_run_graph(
        &self,
        request: Request<GetRunGraphRequest>,
    ) -> Result<Response<GetRunGraphResponse>, Status> {
        let _actor = self.actor_for(&request, "")?;
        let task_id = parse_id::<TaskId>("task_id", &request.get_ref().task_id)?;
        let mut txn = self.store.begin_read().await.map_err(unavailable)?;
        let head = txn
            .graph()
            .get_head(task_id)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| Status::not_found("task graph not found"))?;
        let deps = txn
            .graph()
            .list_dependencies(task_id)
            .await
            .map_err(unavailable)?;
        let runs = txn
            .runs()
            .list_by_task(task_id)
            .await
            .map_err(unavailable)?;
        Ok(Response::new(GetRunGraphResponse {
            graph: Some(domain::generated::contract::RunGraphView {
                task_id: task_id.to_string(),
                graph_revision: head.graph_revision,
                runs: runs.iter().map(crate::views::run_view).collect(),
                dependencies: deps.iter().map(crate::views::dependency_view).collect(),
            }),
        }))
    }

    async fn get_active_config(
        &self,
        request: Request<GetActiveConfigRequest>,
    ) -> Result<Response<GetActiveConfigResponse>, Status> {
        let _actor = self.actor_for(&request, "")?;
        let mut txn = self.store.begin_read().await.map_err(unavailable)?;
        let active = txn
            .config()
            .get_active()
            .await
            .map_err(unavailable)?
            .ok_or_else(|| Status::not_found("no active config generation"))?;
        let generation = txn
            .config()
            .get_generation(active.generation_id)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| Status::not_found("active generation row missing"))?;
        Ok(Response::new(GetActiveConfigResponse {
            generation: Some(crate::views::config_generation_view(&generation)),
            active_revision: active.revision,
        }))
    }

    async fn list_adapters(
        &self,
        request: Request<ListAdaptersRequest>,
    ) -> Result<Response<ListAdaptersResponse>, Status> {
        let _actor = self.actor_for(&request, "")?;
        let port_filter = request.get_ref().port_id.clone();
        let mut txn = self.store.begin_read().await.map_err(unavailable)?;
        let mut rows = txn
            .adapters()
            .list_registrations()
            .await
            .map_err(unavailable)?;
        if !port_filter.is_empty() {
            rows.retain(|row| {
                serde_json::from_slice::<Vec<serde_json::Value>>(&row.implemented_ports)
                    .unwrap_or_default()
                    .iter()
                    .any(|p| {
                        p.get("port_id").and_then(|v| v.as_str()) == Some(port_filter.as_str())
                    })
            });
        }
        Ok(Response::new(ListAdaptersResponse {
            adapters: rows.iter().map(crate::views::adapter_view).collect(),
        }))
    }

    async fn respond_approval(
        &self,
        request: Request<ApprovalResponseRequest>,
    ) -> Result<Response<ApprovalResponseResult>, Status> {
        let actor = self.actor_for(&request, "")?;
        let req = request.into_inner();
        let payload = domain::generated::contract::RespondApproval {
            request_id: req.request_id.clone(),
            request_digest: req.request_digest.clone(),
            decision: req.decision.clone(),
            device_id: req.device_id.clone(),
            responder_principal_id: actor.principal_id.to_string(),
        };
        let request_id = parse_id::<domain::ids::ApprovalRequestId>("request_id", &req.request_id)?;
        let digest_hex = req.request_digest.clone();
        let envelope = CommandEnvelope {
            command_id: CommandId::new(self.ids.as_ref()),
            idempotency_key: IdempotencyKey::new(format!(
                "approval.respond.{request_id}.{digest_hex}"
            ))
            .map_err(|_| Status::invalid_argument("idempotency key invalid"))?,
            principal_id: actor.principal_id,
            actor_id: domain::ids::ActorId::new(self.ids.as_ref()),
            device_id: if req.device_id.is_empty() {
                None
            } else {
                Some(parse_id("device_id", &req.device_id)?)
            },
            delegation_chain_id: None,
            request_digest: RequestDigest::from_str(&digest_hex)
                .map_err(|_| Status::invalid_argument("request_digest is malformed"))?,
            correlation_id: None,
            causation_id: None,
            deadline_unix_ms: None,
            command_type: "agentos.spec.v1.RespondApproval".to_owned(),
            payload: payload.encode_to_vec(),
        };
        self.coordinator
            .execute(envelope)
            .await
            .map_err(unavailable)?;
        Ok(Response::new(ApprovalResponseResult {
            status: "recorded".to_owned(),
        }))
    }
}

/// Runs the service on a [`ControlSocket`] until `shutdown` resolves.
///
/// `socket` is moved into the accept task; when the server stops the task is
/// aborted and the drop unlinks the path.
pub async fn serve(
    socket: ControlSocket,
    service: ControlApiService,
    event_service: Option<crate::event_service::EventApiService>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> errors::Result<()> {
    let (tx, rx) = tokio::sync::mpsc::channel::<std::io::Result<UdsConn>>(64);
    let accept_task = tokio::spawn(async move {
        loop {
            match socket.accept().await {
                Ok(stream) => {
                    let creds = match stream.peer_cred() {
                        Ok(c) => PeerCreds {
                            uid: c.uid(),
                            pid: c.pid().map(|p| p.max(0) as u32),
                        },
                        Err(e) => {
                            if tx.send(Err(e)).await.is_err() {
                                return;
                            }
                            continue;
                        }
                    };
                    if tx.send(Ok(UdsConn { stream, creds })).await.is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
    });
    let incoming = tokio_stream::wrappers::ReceiverStream::new(rx);
    let mut builder =
        tonic::transport::Server::builder().add_service(MvpControlApiServer::new(service));
    if let Some(events) = event_service {
        builder = builder.add_service(
            crate::event_service::generated::mvp_event_api_server::MvpEventApiServer::new(events),
        );
    }
    let result = builder
        .serve_with_incoming_shutdown(incoming, shutdown)
        .await;
    accept_task.abort();
    let _ = accept_task.await;
    result.map_err(|e| {
        KernelError::new(
            ErrorCode::Unavailable,
            RetryClass::Safe,
            "control api server failed",
        )
        .with_source(e)
    })
}
