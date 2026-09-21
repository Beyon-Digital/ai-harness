//! Wasm fixture effect adapter (`wasm.echo`) — the sample "custom wasm
//! plugin": a WASI module speaking the framed adapter protocol over
//! stdin/stdout (the host maps both onto the adapter socketpair).
//!
//! `execute` decodes an `EffectExecutionRequest` and uppercases the
//! payload text — a deterministic, sandbox-safe plugin operation.
//! Operation results replay idempotently via `operation_id`.
use std::collections::BTreeMap;
use std::io::{Read, Write, stdin, stdout};
use std::sync::Mutex;

use domain::generated::contract::{
    AdapterFrame, AdapterHello, EffectExecutionRequest, EffectExecutionResponse,
    adapter_frame::Body,
};
use prost::Message;

/// u32-BE length + prost `AdapterFrame` — same wire format as
/// `adapter-protocol::framing` (that crate's session helpers are
/// unix-only, so the guest carries its own codec).
fn write_frame(stream: &mut impl Write, frame: &AdapterFrame) -> std::io::Result<()> {
    let body = frame.encode_to_vec();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .and_then(|()| stream.write_all(&body))
        .and_then(|()| stream.flush())
}

fn read_frame(stream: &mut impl Read) -> std::io::Result<Option<AdapterFrame>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let mut body = vec![0u8; u32::from_be_bytes(len_buf) as usize];
    stream.read_exact(&mut body)?;
    AdapterFrame::decode(body.as_slice())
        .map(Some)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

const PORT_ID: &str = "effect.execute";
const PROTOCOL_VERSION: u32 = 1;

static OPS: Mutex<Option<BTreeMap<String, Vec<u8>>>> = Mutex::new(None);

fn main() {
    let stdin = stdin();
    let stdout = stdout();
    let mut input = stdin.lock();
    let mut output = stdout.lock();

    let Some(bootstrap) = read_frame(&mut input).unwrap_or(None) else {
        return;
    };
    let Some(Body::Bootstrap(b)) = bootstrap.body else {
        return;
    };
    let hello = AdapterFrame {
        body: Some(Body::Hello(AdapterHello {
            adapter_instance_id: b.adapter_instance_id,
            adapter_id: std::env::var("FIXTURE_ADAPTER_ID").unwrap_or_default(),
            adapter_version: std::env::var("FIXTURE_ADAPTER_VERSION").unwrap_or_default(),
            bundle_digest: b.expected_bundle_digest,
            protocol_version: PROTOCOL_VERSION,
            implemented_ports: vec![PORT_ID.to_owned()],
            capability_document: Vec::new(),
        })),
    };
    if write_frame(&mut output, &hello).is_err() {
        return;
    }

    while let Ok(Some(frame)) = read_frame(&mut input) {
        let Some(body) = frame.body else { continue };
        match body {
            Body::Ping(p) => {
                let pong = AdapterFrame {
                    body: Some(Body::Pong(domain::generated::contract::AdapterPong {
                        nonce: p.nonce,
                    })),
                };
                if write_frame(&mut output, &pong).is_err() {
                    return;
                }
            }
            Body::Shutdown(_) => return,
            Body::Request(req) => {
                let (payload, error_code) = handle(&req);
                let response = AdapterFrame {
                    body: Some(Body::Response(
                        domain::generated::contract::PortCallResponse {
                            call_id: req.call_id.clone(),
                            payload,
                            error_code,
                        },
                    )),
                };
                if write_frame(&mut output, &response).is_err() {
                    return;
                }
            }
            _ => {}
        }
    }
}

/// `execute`/`status` over an in-guest op map — WASI has no filesystem
/// granted, so idempotent replay uses guest memory (the module is
/// per-invocation; durable dedup stays the kernel's job).
fn handle(req: &domain::generated::contract::PortCallRequest) -> (Vec<u8>, String) {
    if req.operation != "execute" {
        return (Vec::new(), "unknown_operation".to_owned());
    }
    let Ok(exec) = EffectExecutionRequest::decode(req.payload.as_slice()) else {
        return (Vec::new(), "invalid_argument".to_owned());
    };
    // Replay: an operation_id we already served returns its recorded row.
    {
        let guard = OPS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ops) = guard.as_ref()
            && let Some(resp) = ops.get(&exec.operation_id)
        {
            return (resp.clone(), String::new());
        }
    }
    let text = String::from_utf8_lossy(&exec.payload).to_uppercase();
    let resp = EffectExecutionResponse {
        effect_id: exec.effect_id.clone(),
        status: "succeeded".to_owned(),
        result_ref: format!("data:text/plain;base64,{}", b64(text.as_bytes())),
        provider_operation_ref: exec.operation_id.clone(),
        error_code: String::new(),
    }
    .encode_to_vec();
    {
        let mut guard = OPS.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .get_or_insert_with(BTreeMap::new)
            .insert(exec.operation_id.clone(), resp.clone());
    }
    (resp, String::new())
}

fn b64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
        }
        if c.len() < 3 {
            out.replace_range(out.len() - (3 - c.len()).., &"=".repeat(3 - c.len()));
        }
    }
    out
}
