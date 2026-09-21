//! OpenRouter-backed effect adapter (`model.chat`).
//!
//! Speaks the framed adapter protocol over its fd-0 socketpair: reads
//! `AdapterBootstrap`, replies `AdapterHello`, then serves `execute` and
//! `status` PortCallRequests. `execute` treats
//! `EffectExecutionRequest.payload` as a chat-completion request JSON
//! (`{"model"?, "messages": [...], "temperature"?, "response_format"?}`),
//! calls OpenRouter, and returns the assistant content as a `data:` URI
//! in `result_ref`. Results are recorded in a durable JSON store keyed
//! by `operation_id`, so a duplicate `execute` replays the recorded
//! answer instead of calling the model twice, and `status` reconciles
//! after a crash.
//!
//! Env:
//! - `OPENROUTER_API_KEY` (required for `execute`) — propagated by the
//!   daemon env allowlist.
//! - `OPENROUTER_MODEL` — default model when the payload omits `model`
//!   (default `openrouter/free`).
//! - `OPENROUTER_BASE_URL` — default `https://openrouter.ai/api/v1`; must
//!   be `https://` on `openrouter.ai` (or a subdomain) so the API key is
//!   only sent to OpenRouter. `OPENROUTER_ALLOW_ANY_BASE_URL=1` opts out.
//! - `OPENROUTER_SITE`, `OPENROUTER_APP_NAME` — optional referer headers.
//! - `OPENROUTER_TIMEOUT_MS` — HTTP timeout (default 55000).
//! - `FIXTURE_STORE` — durable store path (default `openrouter-effect-store.json`).

use std::collections::BTreeMap;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

use adapter_protocol::framing::{read_frame, write_frame};
use domain::generated::contract::{
    AdapterFrame, AdapterHello, AdapterPong, EffectExecutionRequest, EffectExecutionResponse,
    EffectStatusRequest, EffectStatusResponse, adapter_frame::Body,
};
use prost::Message;
use serde_json::Value;

const PORT_ID: &str = "effect.execute";
const PROTOCOL_VERSION: u32 = 1;
const MAX_RESULT_BYTES: usize = 64 * 1024;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("openrouter-effect: {e}");
            ExitCode::FAILURE
        }
    }
}

struct LlmConfig {
    api_key: String,
    default_model: String,
    base_url: String,
    site: Option<String>,
    app_name: Option<String>,
    timeout: Duration,
}

fn run() -> std::io::Result<()> {
    // SAFETY: fd 0 is the private socketpair end mapped by the supervisor.
    let fd = unsafe { OwnedFd::from_raw_fd(0) };
    let mut stream = UnixStream::from(fd);
    stream.set_read_timeout(None)?;

    let Some(bootstrap_frame) = read_frame(&mut stream).map_err(err)? else {
        return Err(err_msg("closed before bootstrap"));
    };
    let Some(Body::Bootstrap(bootstrap)) = bootstrap_frame.body else {
        return Err(err_msg("expected AdapterBootstrap"));
    };
    write_frame(
        &mut stream,
        &AdapterFrame {
            body: Some(Body::Hello(AdapterHello {
                adapter_instance_id: bootstrap.adapter_instance_id,
                adapter_id: env("FIXTURE_ADAPTER_ID").unwrap_or_default(),
                adapter_version: env("FIXTURE_ADAPTER_VERSION").unwrap_or_default(),
                bundle_digest: bootstrap.expected_bundle_digest,
                protocol_version: PROTOCOL_VERSION,
                implemented_ports: vec![PORT_ID.to_owned()],
                capability_document: Vec::new(),
            })),
        },
    )
    .map_err(err)?;

    let base_url = env("OPENROUTER_BASE_URL")
        .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_owned())
        .trim_end_matches('/')
        .to_owned();
    if env("OPENROUTER_ALLOW_ANY_BASE_URL").as_deref() != Ok("1") {
        let host = base_url
            .strip_prefix("https://")
            .and_then(|rest| rest.split('/').next())
            .and_then(|h| h.split(':').next())
            .unwrap_or_default();
        if !(host == "openrouter.ai" || host.ends_with(".openrouter.ai")) {
            return Err(err_msg(
                "OPENROUTER_BASE_URL must be https on openrouter.ai \
                 (or set OPENROUTER_ALLOW_ANY_BASE_URL=1)",
            ));
        }
    }
    let config = LlmConfig {
        api_key: env("OPENROUTER_API_KEY").unwrap_or_default(),
        default_model: env("OPENROUTER_MODEL").unwrap_or_else(|_| "openrouter/free".to_owned()),
        base_url,
        site: env("OPENROUTER_SITE").ok(),
        app_name: env("OPENROUTER_APP_NAME").ok(),
        timeout: env("OPENROUTER_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_millis(55_000)),
    };
    let store_path =
        env("FIXTURE_STORE").unwrap_or_else(|_| "openrouter-effect-store.json".to_owned());

    while let Some(frame) = read_frame(&mut stream).map_err(err)? {
        let Some(body) = frame.body else { continue };
        let request = match body {
            Body::Request(req) => req,
            Body::Ping(p) => {
                write_frame(
                    &mut stream,
                    &AdapterFrame {
                        body: Some(Body::Pong(AdapterPong { nonce: p.nonce })),
                    },
                )
                .map_err(err)?;
                continue;
            }
            Body::Shutdown(_) => return Ok(()),
            _ => continue,
        };
        let response = handle(&request, &config, &store_path);
        write_frame(
            &mut stream,
            &AdapterFrame {
                body: Some(Body::Response(response)),
            },
        )
        .map_err(err)?;
    }
    Ok(())
}

#[derive(Default, serde::Serialize, serde::Deserialize, Clone)]
struct OpRecord {
    status: String,
    result_ref: String,
    error_code: String,
}

fn handle(
    request: &domain::generated::contract::PortCallRequest,
    config: &LlmConfig,
    store_path: &str,
) -> domain::generated::contract::PortCallResponse {
    let (payload, error_code) = match request.operation.as_str() {
        "execute" => match EffectExecutionRequest::decode(request.payload.as_slice()) {
            Ok(req) => execute(req, config, store_path),
            Err(_) => (Vec::new(), "invalid_argument".to_owned()),
        },
        "status" => match EffectStatusRequest::decode(request.payload.as_slice()) {
            Ok(req) => status(req, store_path),
            Err(_) => (Vec::new(), "invalid_argument".to_owned()),
        },
        _ => (Vec::new(), "unknown_operation".to_owned()),
    };
    domain::generated::contract::PortCallResponse {
        call_id: request.call_id.clone(),
        payload,
        error_code,
    }
}

/// Missing store ⇒ fresh empty store; every other failure is reported
/// so the kernel leaves the effect Dispatched for reconciliation rather
/// than replaying/cancelling from a silently-empty op table.
fn load_store(path: &str) -> std::io::Result<BTreeMap<String, OpRecord>> {
    match std::fs::read_to_string(path) {
        Ok(body) => serde_json::from_str(&body)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e),
    }
}

fn save_store(path: &str, store: &BTreeMap<String, OpRecord>) {
    let Ok(body) = serde_json::to_string(store) else {
        return;
    };
    // Write-then-rename so a crash mid-write cannot leave a torn store.
    let tmp = format!("{path}.tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

/// `model.chat`: idempotent on `operation_id` — a repeat replays the
/// recorded result instead of paying for a second completion.
fn execute(req: EffectExecutionRequest, config: &LlmConfig, store_path: &str) -> (Vec<u8>, String) {
    let fail = |code: &str| -> (Vec<u8>, String) {
        (
            EffectExecutionResponse {
                effect_id: req.effect_id.clone(),
                status: "failed".to_owned(),
                result_ref: String::new(),
                provider_operation_ref: req.operation_id.clone(),
                error_code: code.to_owned(),
            }
            .encode_to_vec(),
            String::new(),
        )
    };

    let mut store = match load_store(store_path) {
        Ok(store) => store,
        Err(e) => return (Vec::new(), format!("unavailable: {e}")),
    };
    if let Some(rec) = store.get(&req.operation_id) {
        return (
            EffectExecutionResponse {
                effect_id: req.effect_id,
                status: rec.status.clone(),
                result_ref: rec.result_ref.clone(),
                provider_operation_ref: req.operation_id,
                error_code: rec.error_code.clone(),
            }
            .encode_to_vec(),
            String::new(),
        );
    }
    if config.api_key.is_empty() {
        return fail("missing_openrouter_api_key");
    }
    let Ok(payload) = serde_json::from_slice::<Value>(&req.payload) else {
        return fail("invalid_argument");
    };
    let mut body = payload.clone();
    if body.get("model").is_none() {
        body["model"] = Value::String(config.default_model.clone());
    }
    if body.get("messages").and_then(|m| m.as_array()).is_none() {
        return fail("invalid_argument");
    }

    match call_chat(config, &body) {
        Ok(content) => {
            let result_ref = data_uri(&content);
            store.insert(
                req.operation_id.clone(),
                OpRecord {
                    status: "succeeded".to_owned(),
                    result_ref: result_ref.clone(),
                    error_code: String::new(),
                },
            );
            save_store(store_path, &store);
            (
                EffectExecutionResponse {
                    effect_id: req.effect_id,
                    status: "succeeded".to_owned(),
                    result_ref,
                    provider_operation_ref: req.operation_id,
                    error_code: String::new(),
                }
                .encode_to_vec(),
                String::new(),
            )
        }
        // Transient/provider failures are not recorded: the kernel marks
        // the effect failed and a retried execute re-attempts cleanly.
        Err(code) => fail(&code),
    }
}

fn status(req: EffectStatusRequest, store_path: &str) -> (Vec<u8>, String) {
    let store = match load_store(store_path) {
        Ok(store) => store,
        Err(e) => return (Vec::new(), format!("unavailable: {e}")),
    };
    let rec = store.get(&req.operation_id);
    (
        EffectStatusResponse {
            status: rec
                .map(|r| r.status.clone())
                .unwrap_or_else(|| "not_found".to_owned()),
            result_ref: rec.map(|r| r.result_ref.clone()).unwrap_or_default(),
            error_code: rec.map(|r| r.error_code.clone()).unwrap_or_default(),
        }
        .encode_to_vec(),
        String::new(),
    )
}

fn call_chat(config: &LlmConfig, body: &Value) -> Result<String, String> {
    let url = format!("{}/chat/completions", config.base_url);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(config.timeout))
        .build()
        .into();
    let mut req = agent
        .post(&url)
        .header("Authorization", &format!("Bearer {}", config.api_key))
        .header("Content-Type", "application/json");
    if let Some(site) = &config.site {
        req = req.header("HTTP-Referer", site);
    }
    if let Some(name) = &config.app_name {
        req = req.header("X-Title", name);
    }
    let mut response = req
        .send_json(body)
        .map_err(|_| "provider_unreachable".to_owned())?;
    let payload: Value = response
        .body_mut()
        .read_json()
        .map_err(|_| "provider_bad_response".to_owned())?;
    payload["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "provider_bad_response".to_owned())
}

fn data_uri(text: &str) -> String {
    let mut end = MAX_RESULT_BYTES.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "data:text/plain;base64,{}",
        base64_encode(&text.as_bytes()[..end])
    )
}

fn base64_encode(input: &[u8]) -> String {
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

fn env(key: &str) -> Result<String, std::env::VarError> {
    std::env::var(key)
}

fn err(e: impl std::fmt::Display) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

fn err_msg(msg: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, msg.to_owned())
}
