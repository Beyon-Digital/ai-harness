//! Local memory-store effect adapter (`memory.*` operations).
//!
//! Speaks the framed adapter protocol over its fd-0 socketpair and serves
//! `execute`/`status` on `effect.execute`. Each effect payload is a JSON
//! memory operation:
//!
//!   memory.put     {"namespace": N, "memory_id"?: I, "record": R,
//!                   "sensitivity"?: S}
//!   memory.get     {"namespace": N, "memory_id": I}
//!   memory.delete  {"namespace": N, "memory_id": I}
//!   memory.list    {"namespace": N}
//!   memory.search  {"namespace": N, "query": {"text": "substr"}}
//!
//! Phase-10 provenance + sensitivity: every stored record is wrapped in
//! `{record, sensitivity, provenance:{effect_id, operation_id,
//! fencing_token, written_at_ms}}`. `sensitivity` is one of
//! public|internal|confidential|secret (default `internal`); a put may
//! raise but never lower a record's class (`sensitivity_downgrade`).
//! Namespaces are authority-scoped identifiers — `ns` must match
//! `[a-z0-9][a-z0-9._-]{0,63}` so a caller can't escape into a foreign
//! namespace via odd characters.
//!
//! Records live in a durable JSON store (`{namespace: {id: envelope}}` at
//! `FIXTURE_STORE`, default `local-memory-store.json`) so memory survives
//! daemon restarts. Every write is fenced/idempotent on `operation_id`
//! and committed effects return `memory://<namespace>/<id>` result refs;
//! reads return a `data:application/json;base64,` payload carrying the
//! record plus its provenance/sensitivity envelope.
//!
//! Env: `FIXTURE_STORE` — store path (set by the daemon).

use std::collections::BTreeMap;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use adapter_protocol::framing::{read_frame, write_frame};
use domain::generated::contract::{
    AdapterFrame, AdapterHello, AdapterPong, EffectExecutionRequest, EffectExecutionResponse,
    EffectStatusRequest, EffectStatusResponse, adapter_frame::Body,
};
use prost::Message;
use serde_json::Value;

const PORT_ID: &str = "effect.execute";
const PROTOCOL_VERSION: u32 = 1;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("local-memory: {e}");
            ExitCode::FAILURE
        }
    }
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

    let store_path = env("FIXTURE_STORE").unwrap_or_else(|_| "local-memory-store.json".to_owned());

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
        let response = handle(&request, &store_path);
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

fn handle(
    request: &domain::generated::contract::PortCallRequest,
    store_path: &str,
) -> domain::generated::contract::PortCallResponse {
    let (payload, error_code) = match request.operation.as_str() {
        "execute" => match EffectExecutionRequest::decode(request.payload.as_slice()) {
            Ok(req) => execute(req, store_path),
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

/// Store shape: `{ops: {op_id: record}, memory: {ns: {id: record}}}`.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Store {
    #[serde(default)]
    ops: BTreeMap<String, OpRecord>,
    #[serde(default)]
    memory: BTreeMap<String, BTreeMap<String, Value>>,
}

#[derive(Default, serde::Serialize, serde::Deserialize, Clone)]
struct OpRecord {
    status: String,
    result_ref: String,
    error_code: String,
}

/// Missing store ⇒ fresh empty store; every other failure is reported
/// so the kernel leaves the effect Dispatched for reconciliation instead
/// of committing a result computed from a silently-empty memory.
fn load_store(path: &str) -> std::io::Result<Store> {
    match std::fs::read_to_string(path) {
        Ok(body) => serde_json::from_str(&body)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store::default()),
        Err(e) => Err(e),
    }
}

fn save_store(path: &str, store: &Store) {
    let Ok(body) = serde_json::to_string(store) else {
        return;
    };
    // Write-then-rename so a crash mid-write cannot leave a torn store.
    let tmp = format!("{path}.tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

const SENSITIVITY: [&str; 4] = ["public", "internal", "confidential", "secret"];

fn sensitivity_rank(s: &str) -> Option<u8> {
    SENSITIVITY.iter().position(|&x| x == s).map(|i| i as u8)
}

/// Namespace authority rule: `[a-z0-9][a-z0-9._-]{0,63}` — keeps each
/// namespace a clean isolation domain.
fn valid_namespace(ns: &str) -> bool {
    !ns.is_empty()
        && ns.len() <= 64
        && ns.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || (i > 0 && matches!(b, b'.' | b'_' | b'-'))
        })
}

fn execute(req: EffectExecutionRequest, store_path: &str) -> (Vec<u8>, String) {
    let finish = |status: &str, result_ref: String, code: &str| -> (Vec<u8>, String) {
        (
            EffectExecutionResponse {
                effect_id: req.effect_id.clone(),
                status: status.to_owned(),
                result_ref,
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
    if let Some(rec) = store.ops.get(&req.operation_id) {
        return finish(&rec.status, rec.result_ref.clone(), &rec.error_code);
    }
    let Ok(payload) = serde_json::from_slice::<Value>(&req.payload) else {
        return finish("failed", String::new(), "invalid_argument");
    };
    // The effect `operation` column is kernel-side bookkeeping; the memory
    // op name travels in the payload so one adapter can serve all of them.
    let op = payload
        .get("op")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let namespace = payload
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !op.starts_with("memory.") || !valid_namespace(namespace) {
        return finish("failed", String::new(), "unsupported_operation");
    }
    let ns = store.memory.entry(namespace.to_owned()).or_default();

    let out = match op {
        "memory.put" => {
            let id = payload
                .get("memory_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| req.operation_id.clone());
            let Some(record) = payload.get("record").cloned() else {
                return finish("failed", String::new(), "invalid_argument");
            };
            let sensitivity = payload
                .get("sensitivity")
                .and_then(Value::as_str)
                .unwrap_or("internal");
            let Some(rank) = sensitivity_rank(sensitivity) else {
                return finish("failed", String::new(), "invalid_sensitivity");
            };
            // Provenance + sensitivity are an authority: a put may raise
            // a record's class but never lower it.
            if let Some(existing) = ns.get(&id) {
                let existing_rank = existing
                    .get("sensitivity")
                    .and_then(Value::as_str)
                    .and_then(sensitivity_rank)
                    .unwrap_or(1);
                if rank < existing_rank {
                    return finish("failed", String::new(), "sensitivity_downgrade");
                }
            }
            let written_at_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            ns.insert(
                id.clone(),
                serde_json::json!({
                    "record": record,
                    "sensitivity": sensitivity,
                    "provenance": {
                        "effect_id": req.effect_id,
                        "operation_id": req.operation_id,
                        "fencing_token": req.fencing_token,
                        "written_at_ms": written_at_ms,
                    },
                }),
            );
            ("succeeded", format!("memory://{namespace}/{id}"), "")
        }
        "memory.get" => {
            let Some(id) = payload.get("memory_id").and_then(Value::as_str) else {
                return finish("failed", String::new(), "invalid_argument");
            };
            match ns.get(id) {
                Some(record) => (
                    "succeeded",
                    format!("data:application/json;base64,{}", json_b64(record)),
                    "",
                ),
                None => return finish("failed", String::new(), "not_found"),
            }
        }
        "memory.delete" => {
            let Some(id) = payload.get("memory_id").and_then(Value::as_str) else {
                return finish("failed", String::new(), "invalid_argument");
            };
            ns.remove(id);
            ("succeeded", format!("memory://{namespace}/{id}"), "")
        }
        "memory.list" => {
            let ids: Vec<serde_json::Value> = ns
                .iter()
                .map(|(id, env)| {
                    serde_json::json!({
                        "id": id,
                        "sensitivity": env.get("sensitivity")
                            .and_then(Value::as_str)
                            .unwrap_or("internal"),
                    })
                })
                .collect();
            (
                "succeeded",
                format!(
                    "data:application/json;base64,{}",
                    b64(&serde_json::to_vec(&ids).unwrap_or_default())
                ),
                "",
            )
        }
        "memory.search" => {
            let needle = payload["query"]["text"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase();
            let hits: Vec<serde_json::Value> = ns
                .iter()
                .filter(|(id, rec)| {
                    id.to_lowercase().contains(&needle)
                        || rec.to_string().to_lowercase().contains(&needle)
                })
                .map(|(id, env)| {
                    serde_json::json!({
                        "id": id,
                        "sensitivity": env.get("sensitivity")
                            .and_then(Value::as_str)
                            .unwrap_or("internal"),
                    })
                })
                .collect();
            (
                "succeeded",
                format!(
                    "data:application/json;base64,{}",
                    b64(&serde_json::to_vec(&hits).unwrap_or_default())
                ),
                "",
            )
        }
        _ => return finish("failed", String::new(), "unsupported_operation"),
    };
    store.ops.insert(
        req.operation_id.clone(),
        OpRecord {
            status: out.0.to_owned(),
            result_ref: out.1.clone(),
            error_code: String::new(),
        },
    );
    save_store(store_path, &store);
    finish(out.0, out.1, "")
}

fn status(req: EffectStatusRequest, store_path: &str) -> (Vec<u8>, String) {
    let store = match load_store(store_path) {
        Ok(store) => store,
        Err(e) => return (Vec::new(), format!("unavailable: {e}")),
    };
    let rec = store.ops.get(&req.operation_id);
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

fn json_b64(v: &Value) -> String {
    b64(&serde_json::to_vec(v).unwrap_or_default())
}

fn b64(input: &[u8]) -> String {
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
