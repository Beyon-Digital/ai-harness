//! Fixture external effect adapter (`fixture.increment_counter`).
//!
//! Speaks the framed adapter protocol over its fd-0 socketpair: reads
//! `AdapterBootstrap`, replies `AdapterHello`, then serves `execute` and
//! `status` PortCallRequests against a small JSON durable store keyed by
//! `operation_id` — a duplicate request replays the recorded result
//! without re-applying the side effect.
//!
//! Fault flags (env, for conformance tests):
//! - `FIXTURE_CRASH_BEFORE_RESPONSE=1` — apply the side effect, exit(2)
//!   before writing the response (the exact ambiguity reconciliation
//!   exists for);
//! - `FIXTURE_DELAY_MS=<ms>` — sleep before each response;
//! - `FIXTURE_MIN_FENCE=<n>` — reject `fencing_token < n` with
//!   `fencing_rejected`.
use std::collections::BTreeMap;
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use adapter_protocol::framing::{read_frame, write_frame};
use domain::generated::contract::{
    AdapterFrame, AdapterHello, EffectExecutionRequest, EffectExecutionResponse,
    EffectStatusRequest, EffectStatusResponse, adapter_frame::Body,
};
use prost::Message;

const PORT_ID: &str = "effect.execute";
const PROTOCOL_VERSION: u32 = 1;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("effect-adapter: {e}");
            ExitCode::FAILURE
        }
    }
}

fn ipc() -> std::io::Result<UnixStream> {
    // The supervisor maps the private socketpair end onto fd 0.
    // SAFETY: fd 0 is our live socketpair end, owned uniquely by this
    // process since spawn; we take sole ownership once.
    let fd = unsafe { OwnedFd::from_raw_fd(0) };
    Ok(UnixStream::from(fd))
}

fn run() -> std::io::Result<()> {
    let mut stream = ipc()?;
    stream.set_read_timeout(None)?;
    // 1. bootstrap
    let Some(bootstrap_frame) = read_frame(&mut stream).map_err(err)? else {
        return Err(err_msg("closed before bootstrap"));
    };
    let Some(Body::Bootstrap(bootstrap)) = bootstrap_frame.body else {
        return Err(err_msg("expected AdapterBootstrap"));
    };
    // 2. hello — echo the identity we were given (self-assertion verified
    // against the registry by the kernel; the fixture simply echoes).
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

    let store_path = env("FIXTURE_STORE").unwrap_or_else(|_| "fixture-store.json".to_owned());
    let crash = env("FIXTURE_CRASH_BEFORE_RESPONSE").as_deref() == Ok("1");
    let delay_ms = env("FIXTURE_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let min_fence = env("FIXTURE_MIN_FENCE")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);

    // 3. request loop
    while let Some(frame) = read_frame(&mut stream).map_err(err)? {
        let Some(body) = frame.body else { continue };
        let request = match body {
            Body::Request(req) => req,
            Body::Ping(p) => {
                write_frame(
                    &mut stream,
                    &AdapterFrame {
                        body: Some(Body::Pong(domain::generated::contract::AdapterPong {
                            nonce: p.nonce,
                        })),
                    },
                )
                .map_err(err)?;
                continue;
            }
            Body::Shutdown(_) => return Ok(()),
            _ => continue,
        };
        if delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }
        let response = handle(&request, &store_path, min_fence);
        if crash {
            // Side effect applied inside handle; die before responding.
            std::process::exit(2);
        }
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
    min_fence: u64,
) -> domain::generated::contract::PortCallResponse {
    let (payload, error_code) = match request.operation.as_str() {
        "execute" => {
            let req = EffectExecutionRequest::decode(request.payload.as_slice());
            match req {
                Ok(req) => execute(req, store_path, min_fence),
                Err(_) => (Vec::new(), "invalid_argument".to_owned()),
            }
        }
        "status" => {
            let req = EffectStatusRequest::decode(request.payload.as_slice());
            match req {
                Ok(req) => status(req, store_path),
                Err(_) => (Vec::new(), "invalid_argument".to_owned()),
            }
        }
        _ => (Vec::new(), "unknown_operation".to_owned()),
    };
    domain::generated::contract::PortCallResponse {
        call_id: request.call_id.clone(),
        payload,
        error_code,
    }
}

fn load_store(path: &str) -> BTreeMap<String, (u64, String)> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .map(|v| {
            v.as_object()
                .map(|o| {
                    o.iter()
                        .filter_map(|(k, v)| {
                            Some((
                                k.clone(),
                                (v["count"].as_u64()?, v["result_ref"].as_str()?.to_owned()),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

fn save_store(path: &str, store: &BTreeMap<String, (u64, String)>) {
    let mut obj = serde_json::Map::new();
    for (k, (count, result_ref)) in store {
        obj.insert(
            k.clone(),
            serde_json::json!({"count": count, "result_ref": result_ref}),
        );
    }
    let _ = std::fs::write(path, serde_json::to_string(&obj).unwrap_or_default());
}

/// `fixture.increment_counter`: idempotent on `operation_id` — a repeat
/// replays the recorded (count, result_ref) instead of incrementing.
fn execute(req: EffectExecutionRequest, store_path: &str, min_fence: u64) -> (Vec<u8>, String) {
    if req.fencing_token < min_fence {
        return (
            EffectExecutionResponse {
                effect_id: req.effect_id,
                status: "failed".to_owned(),
                result_ref: String::new(),
                provider_operation_ref: req.operation_id,
                error_code: "fencing_rejected".to_owned(),
            }
            .encode_to_vec(),
            String::new(),
        );
    }
    let mut store = load_store(store_path);
    let entry = store.get(&req.operation_id).cloned();
    let (_count, result_ref) = match entry {
        Some((count, r)) => (count, r),
        None => {
            let count = store.len() as u64 + 1;
            let r = format!("fixture://counter/{count}");
            store.insert(req.operation_id.clone(), (count, r.clone()));
            save_store(store_path, &store);
            (count, r)
        }
    };
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

fn status(req: EffectStatusRequest, store_path: &str) -> (Vec<u8>, String) {
    let store = load_store(store_path);
    let (status, result_ref) = match store.get(&req.operation_id) {
        Some((_, r)) => ("succeeded", r.clone()),
        None => ("not_found", String::new()),
    };
    (
        EffectStatusResponse {
            status: status.to_owned(),
            result_ref,
            error_code: String::new(),
        }
        .encode_to_vec(),
        String::new(),
    )
}

fn env(key: &str) -> Result<String, std::env::VarError> {
    std::env::var(key)
}

fn err(e: errors::KernelError) -> std::io::Error {
    std::io::Error::other(e.to_string())
}

fn err_msg(m: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::BrokenPipe, m.to_owned())
}
