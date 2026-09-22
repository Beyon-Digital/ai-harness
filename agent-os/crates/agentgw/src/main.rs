//! Agent OS HTTP gateway (`agentgw`).
//!
//! Bridges the daemon's Unix-socket gRPC control/event APIs to a
//! localhost JSON REST + SSE surface so a browser GUI can drive the
//! kernel without speaking gRPC. Also serves the static web dashboard.
//!
//! Usage: `agentgw --socket <control.sock> [--listen 127.0.0.1:7740]`
//!
//! The gateway keeps a small JSON index (`<socket>.agentgw-index.json`)
//! of entity ids it has seen created, so the GUI can list sessions,
//! tasks and runs — the frozen MVP contract has no list RPCs.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::{Arc, RwLock};

use axum::Router;
use axum::extract::{DefaultBodyLimit, Path as AxPath, Query, State};
use axum::http::{Request, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio_stream::StreamExt;

use agentctl::Daemon;
use domain::generated::contract;
use domain::ids::{ActorId, CommandId, IdempotencyKey};
use domain::provider::SystemIdProvider;
use prost::Message;

/// The SPA bundle is committed under `web/dist` (built from `web-app/`)
/// so `cargo build` needs no Node toolchain.
#[derive(rust_embed::RustEmbed)]
#[folder = "web/dist/"]
struct WebDist;

mod devices;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("device") {
        return ExitCode::from(devices::cli(&args[1..]) as u8);
    }
    let mut socket = PathBuf::from("/tmp/agentd/control.sock");
    let mut listen = "127.0.0.1:7740".to_owned();
    let mut auth_token = std::env::var("AGENTGW_TOKEN").ok();
    let mut devices_file: Option<PathBuf> = std::env::var("AGENTGW_DEVICES_FILE")
        .ok()
        .map(PathBuf::from);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" if i + 1 < args.len() => {
                socket = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            arg if arg.starts_with("--socket=") => {
                socket = PathBuf::from(&arg[9..]);
                i += 1;
            }
            "--listen" if i + 1 < args.len() => {
                listen = args[i + 1].clone();
                i += 2;
            }
            arg if arg.starts_with("--listen=") => {
                listen = arg[9..].to_owned();
                i += 1;
            }
            "--auth-token" if i + 1 < args.len() => {
                auth_token = Some(args[i + 1].clone());
                i += 2;
            }
            arg if arg.starts_with("--auth-token=") => {
                auth_token = Some(arg[13..].to_owned());
                i += 1;
            }
            "--devices-file" if i + 1 < args.len() => {
                devices_file = Some(PathBuf::from(&args[i + 1]));
                i += 2;
            }
            arg if arg.starts_with("--devices-file=") => {
                devices_file = Some(PathBuf::from(&arg["--devices-file=".len()..]));
                i += 1;
            }
            "-h" | "--help" => {
                println!(
                    "agentgw --socket <control.sock> [--listen 127.0.0.1:7740] \
                     [--auth-token T] [--devices-file F]\n  \
                     agentgw device <add|revoke|list> --devices-file F [--label L] [id]\n  \
                     non-loopback --listen requires AGENTGW_TOKEN/--auth-token or --devices-file"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("agentgw: unknown argument {other}");
                return ExitCode::FAILURE;
            }
        }
    }
    if !is_loopback_addr(&listen) && auth_token.is_none() && devices_file.is_none() {
        eprintln!(
            "agentgw: refusing non-loopback --listen without \
             AGENTGW_TOKEN/--auth-token/--devices-file"
        );
        return ExitCode::FAILURE;
    }
    let loopback = is_loopback_addr(&listen);
    match serve(socket, listen, auth_token, devices_file, loopback) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("agentgw: {e}");
            ExitCode::FAILURE
        }
    }
}

fn is_loopback_addr(listen: &str) -> bool {
    let host = listen.rsplit(':').next_back().unwrap_or(listen);
    let host = listen
        .rsplit_once(':')
        .map(|(h, _)| h.trim_matches(['[', ']']))
        .unwrap_or(host);
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn serve(
    socket: PathBuf,
    listen: String,
    auth_token: Option<String>,
    devices_file: Option<PathBuf>,
    loopback: bool,
) -> std::io::Result<()> {
    let daemon = agentctl::connect(&socket)
        .await
        .map_err(|e| std::io::Error::other(format!("{e}")))?;
    let index_path = socket.with_extension("agentgw-index.json");
    let index = load_index(&index_path);
    let runtime_dir = socket.parent().map(Path::to_path_buf).unwrap_or_default();
    let state = Arc::new(AppState {
        daemon,
        index,
        index_path,
        auth_token,
        devices_file,
        runtime_dir: runtime_dir.clone(),
        loopback,
        local_device_id: local_device_id(&runtime_dir),
    });
    if !loopback {
        // Bearer tokens and control-plane traffic over plaintext HTTP are
        // interceptable — remote binds belong behind a TLS terminator.
        tracing::warn!(
            %listen,
            "agentgw serving plaintext HTTP on a non-loopback address; put a TLS terminator in front"
        );
    }

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/api/health", get(api_health))
        .route("/api/index", get(api_index))
        .route("/api/sessions", post(api_create_session))
        .route("/api/specs", post(api_put_spec))
        .route("/api/runs", post(api_create_run))
        .route("/api/runs", get(api_runs_list))
        .route("/api/runs/{run_id}", get(api_get_run))
        .route("/api/runs/{run_id}/decisions", get(api_run_decisions))
        .route("/api/runs/{run_id}/environment", get(api_run_environment))
        .route("/api/runs/{run_id}/cancel", post(api_cancel_run))
        .route("/api/tasks/{task_id}/graph", get(api_graph))
        .route("/api/effects/{effect_id}", get(api_get_effect))
        .route("/api/effects/{effect_id}/resolve", post(api_resolve_effect))
        .route("/api/adapters", get(api_adapters))
        .route("/api/config", get(api_config))
        .route("/api/config/generations", get(api_config_generations))
        .route("/api/config/proposals", get(api_proposals_list))
        .route("/api/config/proposals", post(api_proposals_create))
        .route("/api/profiles", get(api_profiles))
        .route("/api/metrics", get(api_metrics))
        .route("/api/approvals", get(api_approvals))
        .route("/api/approvals/respond", post(api_respond_approval))
        .route("/api/events/read", get(api_events_read))
        .route("/api/events/subscribe", get(api_events_subscribe))
        // 1 MiB JSON bodies are ample for specs/run payloads here.
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn_with_state(state.clone(), require_auth))
        .fallback(serve_static)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&listen).await?;
    tracing::info!(%listen, "agentgw listening");
    axum::serve(listener, app).await
}

struct AppState {
    daemon: Daemon,
    index: RwLock<Index>,
    index_path: PathBuf,
    auth_token: Option<String>,
    /// Per-device credential file (Phase 14); revocations apply per
    /// request without a restart.
    devices_file: Option<PathBuf>,
    /// Daemon runtime dir (socket parent) — kernel.db + events.db are
    /// opened read-only here for the metrics/environment surface.
    runtime_dir: PathBuf,
    /// Whether the listen address is loopback-only — credential-free
    /// requests are only trusted as "local" on loopback binds.
    loopback: bool,
    /// Stable device identity attached to approval responses from
    /// credential-free ("local") and master-token ("admin") callers —
    /// persisted under the runtime dir so restarts keep the same id.
    local_device_id: String,
}

/// Reads or mints the gateway's local device id at
/// `<runtime-dir>/agentgw-device-id` (0600).
fn local_device_id(runtime_dir: &Path) -> String {
    let path = runtime_dir.join("agentgw-device-id");
    if let Ok(id) = std::fs::read_to_string(&path)
        && domain::ids::DeviceId::from_str(id.trim()).is_ok()
    {
        return id.trim().to_owned();
    }
    let id = new_uuid7();
    if let Err(e) = std::fs::write(&path, &id) {
        tracing::warn!("agentgw-device-id persist failed: {e}");
    } else if let Err(e) = std::fs::set_permissions(
        &path,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o600),
    ) {
        tracing::warn!("agentgw-device-id chmod failed: {e}");
    }
    id
}

/// Inserted into request extensions by `require_auth`: `"admin"` for the
/// master token, a device id for a device token, `"local"` when the
/// gateway runs unauthenticated (loopback default).
#[derive(Clone, Debug)]
pub struct AuthedCaller(pub String);

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Index {
    sessions: Vec<String>,
    tasks: Vec<TaskRef>,
    runs: Vec<RunRef>,
    specs: Vec<SpecRef>,
}

#[derive(Serialize, Deserialize, Clone)]
struct SpecRef {
    agent_spec_id: String,
    version: String,
    digest: String,
    /// `display_name` parsed out of the spec body for GUI pickers.
    #[serde(default)]
    display_name: String,
    /// `runtime_profile_name` parsed out of the spec body — lets the
    /// gateway fill `requested_profile` when a caller omits it.
    #[serde(default)]
    profile: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct TaskRef {
    task_id: String,
    session_id: String,
}

#[derive(Serialize, Deserialize, Clone)]
struct RunRef {
    run_id: String,
    task_id: String,
    session_id: String,
}

fn load_index(path: &Path) -> RwLock<Index> {
    let index = std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    RwLock::new(index)
}

/// Persist happens while the write lock is held so on-disk order
/// matches the mutation order — a racing writer can't overwrite a
/// newer index with an older snapshot.
fn remember<F: FnOnce(&mut Index)>(state: &AppState, f: F) {
    if let Ok(mut index) = state.index.write() {
        f(&mut index);
        if let Ok(bytes) = serde_json::to_vec_pretty(&*index) {
            let _ = std::fs::write(&state.index_path, bytes);
        }
    }
}

const MAX_INDEX_ENTRIES: usize = 10_000;

fn index_push<T>(v: &mut Vec<T>, item: T) {
    if v.len() < MAX_INDEX_ENTRIES {
        v.push(item);
    }
}

/// Bearer check: master `--auth-token`, then per-device tokens (each
/// request re-reads the file so `revoke` is immediate). Unauthenticated
/// loopback mode tags the caller `"local"`.
async fn require_auth(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let bearer = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v: &str| v.strip_prefix("Bearer ").map(str::to_owned));
    // A registry read error is an outage, not a bad credential — answer
    // 503 so operators can distinguish it from a 401.
    #[allow(clippy::result_large_err)]
    let device_caller = |t: &str| -> Result<Option<AuthedCaller>, Response> {
        match state.devices_file.as_deref() {
            Some(f) => match devices::authenticate(f, t) {
                Ok(Some(id)) => Ok(Some(AuthedCaller(id))),
                Ok(None) => Ok(None),
                Err(e) => Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "error": format!("device registry unavailable: {e}") })),
                )
                    .into_response()),
            },
            None => Ok(None),
        }
    };
    let caller = if let Some(expected) = &state.auth_token {
        match bearer.as_deref() {
            Some(t) if t == expected => AuthedCaller("admin".to_owned()),
            Some(t) => match device_caller(t) {
                Ok(Some(c)) => c,
                Ok(None) => return unauthorized(),
                Err(r) => return r,
            },
            None => return unauthorized(),
        }
    } else if state.devices_file.is_some() {
        // Devices configured without a master token: a valid device token
        // identifies the caller. Credential-free requests are only "local"
        // on a loopback bind — on a remote listener they're unauthorized.
        match bearer.as_deref() {
            Some(t) => match device_caller(t) {
                Ok(Some(c)) => c,
                Ok(None) => return unauthorized(),
                Err(r) => return r,
            },
            None if state.loopback => AuthedCaller("local".to_owned()),
            None => return unauthorized(),
        }
    } else {
        AuthedCaller("local".to_owned())
    };
    let mut req = req;
    req.extensions_mut().insert(caller);
    next.run(req).await
}

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "unauthorized" })),
    )
        .into_response()
}

/// Minimal browser-isolation headers for the privileged dashboard.
async fn security_headers(req: Request<axum::body::Body>, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        "nosniff".parse().expect("static"),
    );
    h.insert(header::X_FRAME_OPTIONS, "DENY".parse().expect("static"));
    h.insert(
        header::REFERRER_POLICY,
        "no-referrer".parse().expect("static"),
    );
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        // style attributes + <style> tags are used by the bundled SPA
        // (React Flow positions nodes via inline styles).
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data:; font-src 'self'; connect-src 'self'"
            .parse()
            .expect("static"),
    );
    res
}

fn gw_err(e: impl std::fmt::Display) -> Response {
    (
        StatusCode::BAD_GATEWAY,
        Json(json!({ "error": e.to_string() })),
    )
        .into_response()
}

fn bad_req(msg: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": msg.into() })),
    )
        .into_response()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn envelope(command_type: &str, payload: Vec<u8>, key: &str) -> contract::CommandRequest {
    let ids = SystemIdProvider;
    contract::CommandRequest {
        command_id: CommandId::new(&ids).to_string(),
        idempotency_key: IdempotencyKey::new(key)
            .map(|k| k.to_string())
            .unwrap_or_else(|_| format!("key-{}", sha256_hex(key.as_bytes()))),
        principal_id: String::new(),
        actor_id: ActorId::new(&ids).to_string(),
        device_id: String::new(),
        request_digest: sha256_hex(&[command_type.as_bytes(), b"\n", &payload].concat()),
        correlation_id: String::new(),
        causation_id: String::new(),
        deadline_unix_ms: 0,
        command_type: command_type.to_owned(),
        payload,
    }
}

async fn submit<P: Message>(
    daemon: &Daemon,
    command_type: &str,
    payload: P,
    key: &str,
) -> Result<Value, Response> {
    let encoded = payload.encode_to_vec();
    let request = envelope(command_type, encoded, key);
    let response = daemon
        .control
        .clone()
        .submit_command(request)
        .await
        .map_err(gw_err)?
        .into_inner();
    Ok(json!({
        "status": "accepted",
        "command_id": response.command_id,
        "outcome_code": response.outcome_code,
    }))
}

fn opt_string(body: &Value, key: &str) -> String {
    body.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned()
}

fn new_uuid7() -> String {
    uuid::Uuid::now_v7().to_string()
}

/// Entity ids are UUIDv7; absent/blank input gets a fresh one so the
/// GUI form can leave id fields empty.
fn id_or_fresh(body: &Value, key: &str) -> String {
    let v = opt_string(body, key);
    if v.is_empty() { new_uuid7() } else { v }
}

fn req_string<'a>(body: &'a Value, key: &str) -> Result<&'a str, (StatusCode, Json<Value>)> {
    body.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("missing required field {key}") })),
            )
        })
}

// ---- static GUI ----

fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript",
        Some("css") => "text/css",
        Some("json") | Some("map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff") | Some("woff2") => "font/woff2",
        Some("txt") => "text/plain; charset=utf-8",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
}

fn embedded(path: &str) -> Response {
    match WebDist::get(path) {
        Some(file) => {
            let mime = mime_of(path);
            let mut res = ([(header::CONTENT_TYPE, mime)], file.data).into_response();
            // Hashed bundle assets are immutable; index.html is not.
            if path.starts_with("assets/") {
                res.headers_mut().insert(
                    header::CACHE_CONTROL,
                    "public, max-age=31536000, immutable"
                        .parse()
                        .expect("static"),
                );
            } else {
                res.headers_mut()
                    .insert(header::CACHE_CONTROL, "no-cache".parse().expect("static"));
            }
            res
        }
        None => (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response(),
    }
}

async fn serve_index() -> Response {
    embedded("index.html")
}

/// SPA fallback: any non-API path serves index.html so client-side
/// routes work on refresh; unknown /api/ paths stay a real 404.
async fn serve_static(uri: axum::http::Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, Json(json!({"error": "not found"}))).into_response();
    }
    match WebDist::get(path) {
        Some(_) if !path.is_empty() => embedded(path),
        _ => embedded("index.html"),
    }
}

// ---- REST ----

async fn api_health(State(state): State<Arc<AppState>>) -> Response {
    match state
        .daemon
        .control
        .clone()
        .health(contract::HealthRequest {})
        .await
    {
        Ok(r) => {
            let h = r.into_inner();
            Json(json!({
                "status": h.status,
                "daemon_instance_id": h.daemon_instance_id,
                "daemon_fencing_epoch": h.daemon_fencing_epoch,
                "active_config_generation_id": h.active_config_generation_id,
                "outbox_unpublished_count": h.outbox_unpublished_count,
            }))
            .into_response()
        }
        Err(e) => gw_err(e),
    }
}

async fn api_index(State(state): State<Arc<AppState>>) -> Response {
    match state.index.read() {
        Ok(index) => {
            Json(serde_json::to_value(&*index).unwrap_or_else(|_| json!({}))).into_response()
        }
        Err(_) => gw_err("index lock poisoned"),
    }
}

async fn api_create_session(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Response {
    let metadata = body
        .get("metadata")
        .and_then(|v| v.as_str())
        .map(str::as_bytes)
        .map(|b| b.to_vec())
        .unwrap_or_default();
    let session_id = id_or_fresh(&body, "session_id");
    let key = format!("session.{}", session_id);
    match submit(
        &state.daemon,
        "agentos.spec.v1.CreateSession",
        contract::CreateSession {
            session_id: session_id.clone(),
            metadata,
        },
        &key,
    )
    .await
    {
        Ok(v) => {
            remember(&state, |i| {
                if !i.sessions.contains(&session_id) {
                    index_push(&mut i.sessions, session_id.clone());
                }
            });
            let mut v = v;
            v["session_id"] = json!(session_id);
            Json(v).into_response()
        }
        Err(r) => r,
    }
}

async fn api_put_spec(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let id = match req_string(&body, "agent_spec_id") {
        Ok(v) => v.to_owned(),
        Err(r) => return r.into_response(),
    };
    let version = match req_string(&body, "version") {
        Ok(v) => v.to_owned(),
        Err(r) => return r.into_response(),
    };
    let spec_body = if let Some(text) = body.get("body").and_then(|v| v.as_str()) {
        text.as_bytes().to_vec()
    } else if let Some(b64) = body.get("body_base64").and_then(|v| v.as_str()) {
        match b64_decode(b64) {
            Some(v) => v,
            None => return bad_req("body_base64 is not valid base64"),
        }
    } else {
        return bad_req("missing body or body_base64");
    };
    match submit(
        &state.daemon,
        "agentos.spec.v1.PutAgentSpecRevision",
        contract::PutAgentSpecRevision {
            agent_spec_id: id.clone(),
            version: version.clone(),
            body_bytes: spec_body.clone(),
            body_digest: sha256_hex(&spec_body),
        },
        &format!("spec.{id}.{version}.{}", sha256_hex(&spec_body)),
    )
    .await
    {
        Ok(v) => {
            let digest = sha256_hex(&spec_body);
            let mut v = v;
            v["agent_spec_id"] = json!(id);
            v["version"] = json!(version);
            v["digest"] = json!(digest);
            // Spec bodies are JSON for GUI-created agents; pull the
            // display name + runtime profile so pickers and run
            // creation can use them without re-reading the spec.
            let parsed: Value = serde_json::from_slice(&spec_body).unwrap_or_default();
            let display_name = parsed
                .get("display_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            let profile = parsed
                .get("runtime_profile_name")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            remember(&state, |i| {
                if !i
                    .specs
                    .iter()
                    .any(|s| s.agent_spec_id == id && s.version == version)
                {
                    index_push(
                        &mut i.specs,
                        SpecRef {
                            agent_spec_id: id.clone(),
                            version: version.clone(),
                            digest,
                            display_name: display_name.clone(),
                            profile: profile.clone(),
                        },
                    );
                }
            });
            Json(v).into_response()
        }
        Err(r) => r,
    }
}

async fn api_create_run(State(state): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let session_id = match req_string(&body, "session_id") {
        Ok(v) => v.to_owned(),
        Err(r) => return r.into_response(),
    };
    let task_id = id_or_fresh(&body, "task_id");
    let run_id = id_or_fresh(&body, "run_id");
    let task_kind = opt_string(&body, "task_kind");
    let parent_run_id = opt_string(&body, "parent_run_id");
    let payload = body
        .get("task_payload")
        .and_then(|v| v.as_str())
        .map(|s| s.as_bytes().to_vec())
        .unwrap_or_default();
    let agent_spec_ref = match (
        body.get("agent_spec_id").and_then(|v| v.as_str()),
        body.get("spec_version").and_then(|v| v.as_str()),
    ) {
        (Some(id), Some(version)) => Some(contract::VersionedRef {
            id: id.to_owned(),
            version: version.to_owned(),
            digest: opt_string(&body, "spec_digest"),
        }),
        _ => None,
    };
    let requested_capabilities = body
        .get("requested_capabilities")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let key = format!("run.{run_id}.{task_id}");
    match submit(
        &state.daemon,
        "agentos.spec.v1.CreateTaskRun",
        contract::CreateTaskRun {
            task_id: task_id.clone(),
            run_id: run_id.clone(),
            session_id: session_id.clone(),
            task_kind: if task_kind.is_empty() {
                "agentos.task.v1.Run".to_owned()
            } else {
                task_kind
            },
            task_payload: payload,
            agent_spec_ref,
            parent_run_id,
            observed_parent_cancellation_epoch: 0,
            requested_profile: {
                // Fall back to the spec's declared runtime profile so
                // callers that only know the spec (the chat UI) don't
                // wedge the run on `profile '' not defined`.
                let explicit = opt_string(&body, "requested_profile");
                if explicit.is_empty() {
                    state
                        .index
                        .read()
                        .ok()
                        .and_then(|i| {
                            let spec_id = body.get("agent_spec_id").and_then(|v| v.as_str());
                            let spec_ver = body.get("spec_version").and_then(|v| v.as_str());
                            i.specs
                                .iter()
                                .find(|s| {
                                    Some(s.agent_spec_id.as_str()) == spec_id
                                        && Some(s.version.as_str()) == spec_ver
                                })
                                .map(|s| s.profile.clone())
                        })
                        .unwrap_or_default()
                } else {
                    explicit
                }
            },
            workspace_uri: opt_string(&body, "workspace_uri"),
            requested_capabilities,
            requested_budget: Vec::new(),
        },
        &key,
    )
    .await
    {
        Ok(v) => {
            remember(&state, |i| {
                if !i.tasks.iter().any(|t| t.task_id == task_id) {
                    index_push(
                        &mut i.tasks,
                        TaskRef {
                            task_id: task_id.clone(),
                            session_id: session_id.clone(),
                        },
                    );
                }
                if !i.runs.iter().any(|r| r.run_id == run_id) {
                    index_push(
                        &mut i.runs,
                        RunRef {
                            run_id: run_id.clone(),
                            task_id: task_id.clone(),
                            session_id: session_id.clone(),
                        },
                    );
                }
            });
            let mut v = v;
            v["run_id"] = json!(run_id);
            v["task_id"] = json!(task_id);
            Json(v).into_response()
        }
        Err(r) => r,
    }
}

/// `GET /api/runs` — hydrated run rows for the index's run ids. The
/// contract has no list RPC, so this fans out `GetRun` per known id.
async fn api_runs_list(State(state): State<Arc<AppState>>) -> Response {
    let run_ids: Vec<String> = match state.index.read() {
        Ok(i) => i.runs.iter().map(|r| r.run_id.clone()).collect(),
        Err(_) => return gw_err("index lock poisoned"),
    };
    let mut runs = Vec::with_capacity(run_ids.len());
    for run_id in run_ids {
        // A run the daemon forgot (e.g. wiped runtime under a live
        // index) is skipped rather than 502ing the whole list.
        if let Ok(r) = state
            .daemon
            .control
            .clone()
            .get_run(contract::GetRunRequest { run_id })
            .await
            && let Some(run) = r.into_inner().run
        {
            runs.push(run_json(&run));
        }
    }
    Json(json!({"runs": runs})).into_response()
}

fn run_json(run: &contract::AgentRun) -> Value {
    json!({
        "run_id": run.run_id,
        "task_id": run.task_id,
        "session_id": run.session_id,
        "parent_run_id": run.parent_run_id,
        "state": run.state,
        "state_name": run_state_name(run.state),
        "run_revision": run.run_revision,
        "loop_epoch": run.loop_epoch,
        "step_sequence": run.step_sequence,
        "resolved_environment_id": run.resolved_environment_id,
        "output_ref": run.output_ref,
    })
}

fn run_state_name(state: i32) -> &'static str {
    match state {
        0 => "unspecified",
        1 => "created",
        2 => "ready",
        3 => "running",
        4 => "waiting_tool",
        5 => "waiting_child",
        6 => "waiting_human",
        7 => "suspended",
        8 => "cancelling",
        9 => "completed",
        10 => "failed",
        11 => "cancelled",
        _ => "unknown",
    }
}

async fn api_get_run(
    State(state): State<Arc<AppState>>,
    AxPath(run_id): AxPath<String>,
) -> Response {
    let request = contract::GetRunRequest { run_id };
    match state.daemon.control.clone().get_run(request).await {
        Ok(r) => {
            let inner = r.into_inner();
            let run = inner.run.unwrap_or_default();
            Json(run_json(&run)).into_response()
        }
        Err(e) => gw_err(e),
    }
}

/// `GET /api/runs/{id}/decisions` — decodes the run stream's
/// `LoopDecisionAccepted` payloads into readable JSON so the GUI can show
/// what the loop decided each step (including `invoke_effect` requests).
async fn api_run_decisions(
    State(state): State<Arc<AppState>>,
    AxPath(run_id): AxPath<String>,
) -> Response {
    let mut decisions = Vec::new();
    let mut from = 0u64;
    loop {
        let request = contract::ReadEventStreamRequest {
            stream_key: format!("run/{run_id}"),
            from_sequence: from,
            limit: 500,
        };
        let page = match state.daemon.events.clone().read_stream(request).await {
            Ok(r) => r.into_inner(),
            Err(e) => return gw_err(e),
        };
        let last = page.events.last().map(|e| e.sequence);
        for e in &page.events {
            if e.event_type != "LoopDecisionAccepted" {
                continue;
            }
            let Ok(decision) = contract::LoopDecision::decode(e.payload.as_slice()) else {
                continue;
            };
            let (kind, detail) = match decision.decision {
                Some(contract::loop_decision::Decision::Complete(c)) => {
                    ("complete", json!({"output_ref": c.output_ref}))
                }
                Some(contract::loop_decision::Decision::Fail(f)) => {
                    ("fail", json!({"reason_code": f.reason_code}))
                }
                Some(contract::loop_decision::Decision::Wait(w)) => {
                    ("wait", json!({"reason": w.reason}))
                }
                Some(contract::loop_decision::Decision::SpawnAgent(_)) => {
                    ("spawn_agent", json!({}))
                }
                Some(contract::loop_decision::Decision::InvokeEffect(i)) => (
                    "invoke_effect",
                    json!({
                        "operation": i.operation,
                        "payload": String::from_utf8_lossy(&i.payload)
                            .chars().take(2_000).collect::<String>(),
                    }),
                ),
                Some(contract::loop_decision::Decision::RequestApproval(_)) => {
                    ("request_approval", json!({}))
                }
                None => ("none", json!({})),
            };
            decisions.push(json!({
                "sequence": e.sequence,
                "step_sequence": decision.step_sequence,
                "kind": kind,
                "detail": detail,
            }));
        }
        match last {
            Some(seq) if page.events.len() == 500 => from = seq,
            _ => break,
        }
    }
    Json(json!({"decisions": decisions})).into_response()
}

/// Opens a daemon DB read-only — the gateway is a reader, never a writer.
async fn open_db_ro(path: &Path) -> Result<sqlx::SqliteConnection, sqlx::Error> {
    use sqlx::Connection as _;
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false);
    sqlx::SqliteConnection::connect_with(&options).await
}

/// SELECT <col>, COUNT(*) grouped rows as `[{state|kind: v, count: n}]`.
async fn count_by(conn: &mut sqlx::SqliteConnection, sql: &str, key: &str) -> Vec<Value> {
    use sqlx::Row as _;
    match sqlx::query(sql).fetch_all(&mut *conn).await {
        Ok(rows) => rows
            .iter()
            .map(|r| {
                let count = r.get::<i64, _>(1);
                let state = r
                    .try_get::<i64, _>(0)
                    .map(Value::from)
                    .or_else(|_| r.try_get::<String, _>(0).map(Value::from))
                    .unwrap_or(Value::Null);
                json!({key: state, "count": count})
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

async fn count_total(conn: &mut sqlx::SqliteConnection, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(&mut *conn)
        .await
        .unwrap_or_default()
}

/// `GET /api/metrics` — Phase-15 observability surface: durable-store
/// counters read from kernel.db/events.db (read-only connections) plus the
/// gateway's index sizes. All queries are best-effort — a missing table or
/// locked db yields zeros rather than an error page.
async fn api_metrics(State(state): State<Arc<AppState>>) -> Response {
    let kernel_path = state.runtime_dir.join("kernel.db");
    let events_path = state.runtime_dir.join("events.db");
    let mut metrics = json!({"index": {
        "sessions": state.index.read().map(|i| i.sessions.len()).unwrap_or(0),
        "tasks": state.index.read().map(|i| i.tasks.len()).unwrap_or(0),
        "runs": state.index.read().map(|i| i.runs.len()).unwrap_or(0),
        "specs": state.index.read().map(|i| i.specs.len()).unwrap_or(0),
    }});
    match open_db_ro(&kernel_path).await {
        Ok(mut conn) => {
            metrics["runs_by_state"] = json!(
                count_by(
                    &mut conn,
                    "SELECT state, COUNT(*) FROM runs GROUP BY state",
                    "state"
                )
                .await
            );
            metrics["effects_by_state"] = json!(
                count_by(
                    &mut conn,
                    "SELECT state, COUNT(*) FROM effects GROUP BY state",
                    "state"
                )
                .await
            );
            metrics["approvals_by_state"] = json!(
                count_by(
                    &mut conn,
                    "SELECT state, COUNT(*) FROM approval_requests GROUP BY state",
                    "state"
                )
                .await
            );
            metrics["timers_by_state"] = json!(
                count_by(
                    &mut conn,
                    "SELECT state, COUNT(*) FROM timers GROUP BY state",
                    "state"
                )
                .await
            );
            metrics["decisions_total"] =
                json!(count_total(&mut conn, "SELECT COUNT(*) FROM decisions").await);
            metrics["loop_turns_total"] =
                json!(count_total(&mut conn, "SELECT COUNT(*) FROM loop_turns").await);
            metrics["adapters_registered"] =
                json!(count_total(&mut conn, "SELECT COUNT(*) FROM adapter_registrations").await);
            metrics["adapter_instances_total"] =
                json!(count_total(&mut conn, "SELECT COUNT(*) FROM adapter_instances").await);
            metrics["conformance_reports_total"] =
                json!(count_total(&mut conn, "SELECT COUNT(*) FROM conformance_reports").await);
            metrics["reservations_total"] =
                json!(count_total(&mut conn, "SELECT COUNT(*) FROM resource_reservations").await);
            // Pending = staged but not yet journal-published.
            metrics["outbox_pending"] = json!(
                count_total(
                    &mut conn,
                    "SELECT COUNT(*) FROM outbox_events WHERE journal_published_at_ms IS NULL"
                )
                .await
            );
            let active: Option<String> = sqlx::query_scalar::<_, String>(
                "SELECT generation_id FROM active_config_generation LIMIT 1",
            )
            .fetch_optional(&mut conn)
            .await
            .unwrap_or(None);
            metrics["active_generation"] = json!(active);
            let gen_states = count_by(
                &mut conn,
                "SELECT validation_state, COUNT(*) FROM config_generations GROUP BY validation_state",
                "state",
            )
            .await;
            metrics["generations_by_state"] = json!(gen_states);
            let gen_tests = count_by(
                &mut conn,
                "SELECT test_state, COUNT(*) FROM config_generations GROUP BY test_state",
                "state",
            )
            .await;
            metrics["generations_by_test_state"] = json!(gen_tests);
        }
        Err(e) => metrics["kernel_db_error"] = json!(e.to_string()),
    }
    match open_db_ro(&events_path).await {
        Ok(mut conn) => {
            metrics["journal_events_total"] =
                json!(count_total(&mut conn, "SELECT COUNT(*) FROM events").await);
            metrics["journal_max_sequence"] = json!(
                count_total(&mut conn, "SELECT COALESCE(MAX(sequence), 0) FROM events").await
            );
        }
        Err(e) => metrics["events_db_error"] = json!(e.to_string()),
    }
    Json(metrics).into_response()
}

/// `GET /api/runs/{id}/environment` — the frozen Phase-11 resolved
/// environment row + adapter bindings for a run, read straight from
/// kernel.db so historical runs audit without mutable current config.
async fn api_run_environment(
    State(state): State<Arc<AppState>>,
    AxPath(run_id): AxPath<String>,
) -> Response {
    use sqlx::Row as _;
    let mut conn = match open_db_ro(&state.runtime_dir.join("kernel.db")).await {
        Ok(c) => c,
        Err(e) => {
            return gw_err(errors::KernelError::new(
                errors::codes::ErrorCode::Unavailable,
                errors::codes::RetryClass::Safe,
                format!("kernel.db unreadable: {e}"),
            ));
        }
    };
    let row = match sqlx::query(
        "SELECT environment_id, run_id, agent_spec_id, agent_spec_version,
                agent_spec_digest, agent_loop_id, agent_loop_version,
                agent_loop_digest, config_generation_id, workspace_uri,
                workspace_base_revision, workspace_mode, model_provider,
                model_id, model_parameters, kernel_version, protocol_versions,
                capability_grant_ids, approval_request_ids, created_at_ms
         FROM resolved_run_environments WHERE run_id = ?",
    )
    .bind(&run_id)
    .fetch_optional(&mut conn)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            return gw_err(errors::KernelError::new(
                errors::codes::ErrorCode::Internal,
                errors::codes::RetryClass::Never,
                format!("environment query failed: {e}"),
            ));
        }
    };
    let Some(row) = row else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": "run has no resolved environment"})),
        )
            .into_response();
    };
    let env_id: String = row.get("environment_id");
    let blob_json = |name: &str| -> Value {
        let raw: Option<Vec<u8>> = row.get(name);
        raw.and_then(|b| {
            serde_json::from_slice(&b)
                .ok()
                .or_else(|| Some(Value::String(hex_encode(&b))))
        })
        .unwrap_or(Value::Null)
    };
    let bindings = sqlx::query(
        "SELECT port_id, adapter_id, adapter_version, adapter_digest, capabilities
         FROM resolved_bindings WHERE environment_id = ? ORDER BY port_id",
    )
    .bind(&env_id)
    .fetch_all(&mut conn)
    .await
    .unwrap_or_default()
    .iter()
    .map(|b| {
        let caps: Vec<u8> = b.get("capabilities");
        json!({
            "port_id": b.get::<String, _>("port_id"),
            "adapter_id": b.get::<String, _>("adapter_id"),
            "adapter_version": b.get::<String, _>("adapter_version"),
            "adapter_digest": b.get::<String, _>("adapter_digest"),
            "capabilities": serde_json::from_slice::<Value>(&caps)
                .unwrap_or(Value::Null),
        })
    })
    .collect::<Vec<_>>();
    Json(json!({
        "environment": {
            "environment_id": env_id,
            "run_id": row.get::<String, _>("run_id"),
            "agent_spec_id": row.get::<String, _>("agent_spec_id"),
            "agent_spec_version": row.get::<String, _>("agent_spec_version"),
            "agent_spec_digest": row.get::<String, _>("agent_spec_digest"),
            "agent_loop_id": row.get::<String, _>("agent_loop_id"),
            "agent_loop_version": row.get::<String, _>("agent_loop_version"),
            "agent_loop_digest": row.get::<String, _>("agent_loop_digest"),
            "config_generation_id": row.get::<String, _>("config_generation_id"),
            "workspace_uri": row.get::<Option<String>, _>("workspace_uri"),
            "workspace_base_revision": row.get::<Option<String>, _>("workspace_base_revision"),
            "workspace_mode": row.get::<i64, _>("workspace_mode"),
            "model_provider": row.get::<Option<String>, _>("model_provider"),
            "model_id": row.get::<Option<String>, _>("model_id"),
            "model_parameters": blob_json("model_parameters"),
            "kernel_version": row.get::<String, _>("kernel_version"),
            "protocol_versions": blob_json("protocol_versions"),
            "capability_grant_ids": blob_json("capability_grant_ids"),
            "approval_request_ids": blob_json("approval_request_ids"),
            "created_at_ms": row.get::<i64, _>("created_at_ms"),
        },
        "bindings": bindings,
    }))
    .into_response()
}

fn hex_encode(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `GET /api/config/generations` — the config-generation pipeline state
/// (proposed → validated → tested → active), read-only from kernel.db.
async fn api_config_generations(State(state): State<Arc<AppState>>) -> Response {
    use sqlx::Row as _;
    let mut conn = match open_db_ro(&state.runtime_dir.join("kernel.db")).await {
        Ok(c) => c,
        Err(e) => {
            return gw_err(errors::KernelError::new(
                errors::codes::ErrorCode::Unavailable,
                errors::codes::RetryClass::Safe,
                format!("kernel.db unreadable: {e}"),
            ));
        }
    };
    let active: Option<String> = sqlx::query_scalar::<_, String>(
        "SELECT generation_id FROM active_config_generation LIMIT 1",
    )
    .fetch_optional(&mut conn)
    .await
    .unwrap_or(None);
    let rows = sqlx::query(
        "SELECT generation_id, digest, validation_state, test_state,
                created_by_actor_id, created_at_ms
         FROM config_generations ORDER BY created_at_ms DESC",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap_or_default();
    let generations = rows
        .iter()
        .map(|r| {
            let id: String = r.get("generation_id");
            json!({
                "generation_id": id,
                "digest": r.get::<String, _>("digest"),
                "validation_state": r.get::<String, _>("validation_state"),
                "test_state": r.get::<String, _>("test_state"),
                "created_by": r.get::<String, _>("created_by_actor_id"),
                "created_at_ms": r.get::<i64, _>("created_at_ms"),
                "active": active.as_deref() == Some(id.as_str()),
            })
        })
        .collect::<Vec<_>>();
    Json(json!({"active": active, "generations": generations})).into_response()
}

async fn api_graph(
    State(state): State<Arc<AppState>>,
    AxPath(task_id): AxPath<String>,
) -> Response {
    let request = contract::GetRunGraphRequest { task_id };
    match state.daemon.control.clone().get_run_graph(request).await {
        Ok(r) => {
            let graph = r.into_inner().graph.unwrap_or_default();
            Json(json!({
                "task_id": graph.task_id,
                "graph_revision": graph.graph_revision,
                "runs": graph.runs.iter().map(run_json).collect::<Vec<_>>(),
                "dependencies": graph.dependencies.iter().map(|d| json!({
                    "dependency_id": d.dependency_id,
                    "task_id": d.task_id,
                    "source_run_id": d.source_run_id,
                    "target_run_id": d.target_run_id,
                    "condition": d.condition,
                })).collect::<Vec<_>>(),
            }))
            .into_response()
        }
        Err(e) => gw_err(e),
    }
}

async fn api_cancel_run(
    State(state): State<Arc<AppState>>,
    AxPath(run_id): AxPath<String>,
    Json(body): Json<Value>,
) -> Response {
    let key = format!(
        "cancel.{run_id}.{}",
        sha256_hex(body.to_string().as_bytes())
    );
    match submit(
        &state.daemon,
        "agentos.spec.v1.CancelRun",
        contract::CancelRun {
            run_id: run_id.clone(),
            expected_run_revision: body
                .get("expected_run_revision")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            reason: opt_string(&body, "reason"),
        },
        &key,
    )
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(r) => r,
    }
}

async fn api_get_effect(
    State(state): State<Arc<AppState>>,
    AxPath(effect_id): AxPath<String>,
) -> Response {
    let request = contract::GetEffectRequest { effect_id };
    match state.daemon.control.clone().get_effect(request).await {
        Ok(r) => {
            let e = r.into_inner().effect.unwrap_or_default();
            Json(json!({
                "effect_id": e.effect_id,
                "run_id": e.run_id,
                "step_sequence": e.step_sequence,
                "operation": e.operation,
                "state": e.state,
                "adapter_id": e.adapter_id,
                "result_ref": e.result_ref,
                "error_code": e.error_code,
                "executor_fencing_token": e.executor_fencing_token,
            }))
            .into_response()
        }
        Err(e) => gw_err(e),
    }
}

async fn api_resolve_effect(
    State(state): State<Arc<AppState>>,
    AxPath(effect_id): AxPath<String>,
    Json(body): Json<Value>,
) -> Response {
    let action = match req_string(&body, "action") {
        Ok(v) => v.to_owned(),
        Err(r) => return r.into_response(),
    };
    let key = format!("resolve.{effect_id}.{action}");
    match submit(
        &state.daemon,
        "agentos.spec.v1.ResolveUnknownEffect",
        contract::ResolveUnknownEffect {
            effect_id,
            expected_effect_state: body
                .get("expected_effect_state")
                .and_then(|v| v.as_u64())
                .map(|v| v as i32)
                .unwrap_or(8),
            action,
            result_ref: opt_string(&body, "result_ref"),
            reason: opt_string(&body, "reason"),
            approval_request_id: opt_string(&body, "approval_request_id"),
        },
        &key,
    )
    .await
    {
        Ok(v) => Json(v).into_response(),
        Err(r) => r,
    }
}

async fn api_adapters(
    State(state): State<Arc<AppState>>,
    Query(params): Query<BTreeMap<String, String>>,
) -> Response {
    let request = contract::ListAdaptersRequest {
        port_id: params.get("port_id").cloned().unwrap_or_default(),
    };
    match state.daemon.control.clone().list_adapters(request).await {
        Ok(r) => {
            let adapters = r
                .into_inner()
                .adapters
                .iter()
                .map(|a| {
                    json!({
                        "adapter": a.adapter.as_ref().map(|v| json!({
                            "id": v.id, "version": v.version,
                        })),
                        "manifest_digest": a.manifest_digest,
                        "runtime_type": a.runtime_type,
                        "trust_state": a.trust_state,
                        "conformance_state": a.conformance_state,
                        "ports": a.implemented_ports,
                    })
                })
                .collect::<Vec<_>>();
            Json(json!({ "adapters": adapters })).into_response()
        }
        Err(e) => gw_err(e),
    }
}

async fn api_config(State(state): State<Arc<AppState>>) -> Response {
    match state
        .daemon
        .control
        .clone()
        .get_active_config(contract::GetActiveConfigRequest {})
        .await
    {
        Ok(r) => {
            let inner = r.into_inner();
            let g = inner.generation.unwrap_or_default();
            Json(json!({
                "generation_id": g.generation_id,
                "digest": g.digest,
                "validation_state": g.validation_state,
                "test_state": g.test_state,
                "active_revision": inner.active_revision,
                "document": String::from_utf8_lossy(&g.document),
            }))
            .into_response()
        }
        Err(e) => gw_err(e),
    }
}

/// `GET /api/profiles` — the active document's profiles flattened to
/// `{name, bindings}` rows for GUI pickers (agent creation, pipeline
/// designer, run form).
async fn api_profiles(State(state): State<Arc<AppState>>) -> Response {
    let inner = match state
        .daemon
        .control
        .clone()
        .get_active_config(contract::GetActiveConfigRequest {})
        .await
    {
        Ok(r) => r.into_inner(),
        Err(e) => return gw_err(e),
    };
    let document =
        String::from_utf8_lossy(&inner.generation.unwrap_or_default().document).to_string();
    let parsed = match config_engine::parse_document(&document) {
        Ok(d) => d,
        Err(e) => return gw_err(e),
    };
    let profiles = parsed
        .profiles
        .keys()
        .map(|name| {
            // Resolve the extends chain so GUI pickers see the profile's
            // effective bindings, not just the overrides it declares.
            let resolved = config_engine::profile::resolve_profile(&parsed, name);
            let (bindings, extends) = match resolved {
                Ok(r) => (
                    r.bindings,
                    parsed.profiles.get(name).and_then(|p| p.extends.clone()),
                ),
                Err(_) => (BTreeMap::new(), None),
            };
            json!({"name": name, "bindings": bindings, "extends": extends})
        })
        .collect::<Vec<_>>();
    Json(json!({"profiles": profiles})).into_response()
}

/// `POST /api/config/proposals` — validate a candidate config document
/// and stage it under `<runtime-dir>/proposed/<uuid>.yaml`. The kernel
/// only activates config at boot (`--config`), so proposals are a
/// staging + validation surface, not a live apply.
async fn api_proposals_create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Response {
    let document = match req_string(&body, "document") {
        Ok(v) => v.to_owned(),
        Err(r) => return r.into_response(),
    };
    let dir = state.runtime_dir.join("proposed");
    let id = new_uuid7();
    let path = dir.join(format!("{id}.yaml"));
    let path_for_write = path.clone();
    let document_for_write = document.clone();
    // Filesystem I/O off the async executor; failures surface as 500s.
    let write = tokio::task::spawn_blocking(move || -> std::io::Result<i64> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&path_for_write, &document_for_write)?;
        prune_proposals(&dir)?;
        Ok(std::fs::metadata(&path_for_write)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or_default())
    })
    .await;
    let (valid, error) = match config_engine::parse_document(&document) {
        Ok(_) => (true, String::new()),
        Err(e) => (false, e.to_string()),
    };
    let created_at_ms = match write {
        Ok(Ok(t)) => t,
        Ok(Err(e)) => return gw_err(format!("write proposal: {e}")),
        Err(e) => return gw_err(format!("proposal worker: {e}")),
    };
    Json(json!({
        "proposal_id": id,
        "path": path.to_string_lossy(),
        "created_at_ms": created_at_ms,
        "valid": valid,
        "error": error,
    }))
    .into_response()
}

/// Proposals retained on disk — the oldest staged documents are deleted
/// past this bound so repeated proposals cannot fill the runtime volume.
const MAX_PROPOSALS: usize = 64;

fn prune_proposals(dir: &Path) -> std::io::Result<()> {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)?
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("yaml"))
        .filter_map(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .ok()
                .map(|t| (t, e.path()))
        })
        .collect();
    files.sort_by_key(|(t, _)| *t);
    let excess = files.len().saturating_sub(MAX_PROPOSALS);
    for (_, path) in files.into_iter().take(excess) {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}

/// `GET /api/config/proposals` — list staged proposals, re-validating
/// each so a doc that became invalid after an engine upgrade shows it.
async fn api_proposals_list(State(state): State<Arc<AppState>>) -> Response {
    let dir = state.runtime_dir.join("proposed");
    let scanned = tokio::task::spawn_blocking(move || -> std::io::Result<Vec<Value>> {
        let mut proposals = Vec::new();
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(proposals),
            Err(e) => return Err(e),
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                continue;
            }
            let document = match std::fs::read_to_string(&path) {
                Ok(d) => d,
                Err(_) => continue,
            };
            let (valid, error) = match config_engine::parse_document(&document) {
                Ok(_) => (true, String::new()),
                Err(e) => (false, e.to_string()),
            };
            proposals.push(json!({
                "proposal_id": path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default(),
                "path": path.to_string_lossy(),
                "created_at_ms": entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or_default(),
                "valid": valid,
                "error": error,
            }));
        }
        Ok(proposals)
    })
    .await;
    match scanned {
        Ok(Ok(mut proposals)) => {
            proposals.sort_by_key(|p| p["created_at_ms"].as_i64().unwrap_or_default());
            Json(json!({"proposals": proposals})).into_response()
        }
        Ok(Err(e)) => gw_err(format!("proposals dir: {e}")),
        Err(e) => gw_err(format!("proposal worker: {e}")),
    }
}

async fn api_approvals(
    State(state): State<Arc<AppState>>,
    Query(params): Query<BTreeMap<String, String>>,
) -> Response {
    let request = contract::ListApprovalsRequest {
        run_id: params.get("run_id").cloned().unwrap_or_default(),
    };
    match state.daemon.control.clone().list_approvals(request).await {
        Ok(r) => {
            let approvals = r
                .into_inner()
                .approvals
                .iter()
                .map(|a| {
                    json!({
                        "request_id": a.request_id,
                        "request_digest": a.request_digest,
                        "principal_id": a.principal_id,
                        "actor_id": a.actor_id,
                        "run_id": a.run_id,
                        "operation": a.operation,
                        "target_resource": a.target_resource,
                        "state": a.state,
                        "expires_at_ms": a.expires_at_ms,
                        "created_at_ms": a.created_at_ms,
                    })
                })
                .collect::<Vec<_>>();
            Json(json!({ "approvals": approvals })).into_response()
        }
        Err(e) => gw_err(e),
    }
}

async fn api_respond_approval(
    State(state): State<Arc<AppState>>,
    caller: Option<axum::Extension<AuthedCaller>>,
    Json(body): Json<Value>,
) -> Response {
    let request_id = match req_string(&body, "request_id") {
        Ok(v) => v.to_owned(),
        Err(r) => return r.into_response(),
    };
    let request_digest = match req_string(&body, "request_digest") {
        Ok(v) => v.to_owned(),
        Err(r) => return r.into_response(),
    };
    let decision = match req_string(&body, "decision") {
        Ok(v) => v.to_owned(),
        Err(r) => return r.into_response(),
    };
    // The responding device is never caller-asserted: a device-token
    // caller's response is bound to that authenticated device, and
    // admin/local callers are recorded under the gateway's own stable
    // device identity. A body-supplied `device_id` is ignored.
    let authenticated = caller.map(|axum::Extension(c)| c.0);
    let device_id = match authenticated.as_deref() {
        Some("admin") | Some("local") | None => state.local_device_id.clone(),
        Some(id) => id.to_owned(),
    };
    let request = contract::ApprovalResponseRequest {
        request_id,
        request_digest,
        decision,
        device_id,
        responder_principal_id: String::new(),
    };
    match state.daemon.control.clone().respond_approval(request).await {
        Ok(r) => Json(json!({ "status": r.into_inner().status })).into_response(),
        Err(e) => gw_err(e),
    }
}

fn event_json(e: &contract::EventEnvelope) -> Value {
    json!({
        "event_id": e.event_id,
        "event_type": e.event_type,
        "stream_key": e.stream_key,
        "sequence": e.sequence,
        "occurred_unix_ms": e.occurred_unix_ms,
        "sensitivity": e.sensitivity,
        "payload_b64": if e.payload.is_empty() {
            String::new()
        } else {
            b64_encode(&e.payload)
        },
    })
}

async fn api_events_read(
    State(state): State<Arc<AppState>>,
    Query(params): Query<BTreeMap<String, String>>,
) -> Response {
    let stream_key = params.get("stream_key").cloned().unwrap_or_default();
    if stream_key.is_empty() {
        return bad_req("missing stream_key");
    }
    let request = contract::ReadEventStreamRequest {
        stream_key,
        from_sequence: params
            .get("from_sequence")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        limit: params
            .get("limit")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(200)
            .min(2_000),
    };
    match state.daemon.events.clone().read_stream(request).await {
        Ok(r) => {
            let inner = r.into_inner();
            Json(json!({
                "events": inner.events.iter().map(event_json).collect::<Vec<_>>(),
                "retention_gap": inner.retention_gap,
            }))
            .into_response()
        }
        Err(e) => gw_err(e),
    }
}

async fn api_events_subscribe(
    State(state): State<Arc<AppState>>,
    Query(params): Query<BTreeMap<String, String>>,
) -> Response {
    let stream_key = params.get("stream_key").cloned().unwrap_or_default();
    if stream_key.is_empty() {
        return bad_req("missing stream_key");
    }
    let request = contract::SubscribeEventsRequest {
        stream_key,
        after_sequence: params
            .get("after_sequence")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
    };
    let mut events = state.daemon.events.clone();
    match events.subscribe(request).await {
        Ok(r) => {
            let stream = r.into_inner().filter_map(|item| match item {
                Ok(frame) => {
                    let data = match frame.frame {
                        Some(contract::event_stream_frame::Frame::Event(e)) => {
                            serde_json::to_string(&event_json(&e))
                                .unwrap_or_else(|_| "{}".to_owned())
                        }
                        Some(contract::event_stream_frame::Frame::Lag(l)) => {
                            serde_json::to_string(&json!({
                                "lag": true,
                                "stream_key": l.stream_key,
                                "resume_sequence": l.resume_sequence,
                            }))
                            .unwrap_or_else(|_| "{}".to_owned())
                        }
                        None => return None,
                    };
                    Some(Ok::<_, Infallible>(Event::default().data(data)))
                }
                Err(e) => Some(Ok(Event::default()
                    .event("error")
                    .data(json!({ "error": e.to_string() }).to_string()))),
            });
            Sse::new(stream)
                .keep_alive(KeepAlive::default())
                .into_response()
        }
        Err(e) => gw_err(e),
    }
}

fn b64_encode(input: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len() * 4 / 3 + 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    const T: [i8; 256] = {
        let mut t = [-1i8; 256];
        let mut i = 0;
        let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        while i < 64 {
            t[alphabet[i] as usize] = i as i8;
            i += 1;
        }
        t
    };
    // Strict validation: '=' only allowed as 1-2 trailing pad chars.
    let padding = s.bytes().rev().take_while(|b| *b == b'=').count();
    if padding > 2 {
        return None;
    }
    let body_len = s.len() - padding;
    if s.as_bytes()[..body_len].contains(&b'=') {
        return None;
    }
    if !(body_len + padding).is_multiple_of(4) {
        return None;
    }
    let bytes = &s.as_bytes()[..body_len];
    if bytes.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut acc = 0u32;
        let mut n = 0u32;
        for &b in chunk {
            let v = T[b as usize];
            if v < 0 {
                return None;
            }
            acc = (acc << 6) | v as u32;
            n += 1;
        }
        if n < 2 {
            return None;
        }
        acc <<= 6 * (4 - n);
        out.push((acc >> 16) as u8);
        if n > 2 {
            out.push((acc >> 8) as u8);
        }
        if n > 3 {
            out.push(acc as u8);
        }
    }
    Some(out)
}
