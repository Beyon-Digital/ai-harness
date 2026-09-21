//! ACP (Agent Client Protocol) loop adapter — drives an ACP-compatible
//! agent subprocess over NDJSON JSON-RPC instead of calling a model
//! directly. The task becomes a `session/prompt`; streamed
//! `agent_message_chunk`/`agent_message` updates are aggregated into the
//! run's `complete` output.
//!
//! Flow per `decide` call: spawn `ACP_COMMAND` → `initialize` →
//! `session/new` → `session/prompt` → collect until the prompt response
//! → `Complete`. Server→client `session/request_permission` requests are
//! answered `cancelled` unless `ACP_ALLOW_TOOLS=1`, in which case the
//! first `allow*` option is selected.
//!
//! Env:
//! - `ACP_COMMAND` (required) — e.g. `gemini` with `ACP_ARGS=--acp`.
//! - `ACP_ARGS` — JSON array or whitespace-split argument list.
//! - `ACP_CWD` — working dir passed to `session/new` (default `.`).
//! - `ACP_TIMEOUT_MS` — per-RPC deadline (default 120000).
//! - `ACP_ALLOW_TOOLS` — `1` auto-selects the first allow option.

use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

use adapter_protocol::framing::{read_frame, write_frame};
use domain::generated::contract::{
    AdapterFrame, AdapterHello, AdapterPong, Complete, Fail, LoopDecision, LoopInput,
    PortCallResponse, adapter_frame::Body, loop_decision,
};
use ndjson_rpc::{Client, Incoming, Reply, Spawn};
use prost::Message;
use serde_json::{Value, json};

const PORT_ID: &str = "agent_loop";
const PROTOCOL_VERSION: u32 = 1;
const MAX_OUTPUT_BYTES: usize = 24 * 1024;
const ACP_VERSION: u32 = 1;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("acp-loop: {e}");
            ExitCode::FAILURE
        }
    }
}

struct AcpConfig {
    command: String,
    args: Vec<String>,
    cwd: String,
    timeout: Duration,
    allow_tools: bool,
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
                adapter_id: env("AGENTOS_ADAPTER_ID").unwrap_or_default(),
                adapter_version: env("AGENTOS_ADAPTER_VERSION").unwrap_or_default(),
                bundle_digest: bootstrap.expected_bundle_digest,
                protocol_version: PROTOCOL_VERSION,
                implemented_ports: vec![PORT_ID.to_owned()],
                capability_document: Vec::new(),
            })),
        },
    )
    .map_err(err)?;

    let config = AcpConfig {
        command: env("ACP_COMMAND").unwrap_or_default(),
        args: parse_args(&env("ACP_ARGS").unwrap_or_default()),
        cwd: env("ACP_CWD").unwrap_or_else(|_| ".".to_owned()),
        timeout: env("ACP_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or(Duration::from_millis(120_000)),
        allow_tools: env("ACP_ALLOW_TOOLS").as_deref() == Ok("1"),
    };

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
        let response = decide(&request, &config);
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

fn decide(
    request: &domain::generated::contract::PortCallRequest,
    config: &AcpConfig,
) -> PortCallResponse {
    let reply = |decision: Option<loop_decision::Decision>, error_code: String| {
        let Ok(input) = LoopInput::decode(request.payload.as_slice()) else {
            return PortCallResponse {
                call_id: request.call_id.clone(),
                payload: Vec::new(),
                error_code: "invalid_argument".to_owned(),
            };
        };
        let out = LoopDecision {
            run_id: input.run_id,
            run_revision: input.run_revision,
            loop_epoch: input.loop_epoch,
            step_sequence: input.step_sequence,
            input_event_cursor: input.input_event_cursor,
            turn_id: input.turn_id.clone(),
            decision_id: input.turn_id,
            decision,
        };
        PortCallResponse {
            call_id: request.call_id.clone(),
            payload: out.encode_to_vec(),
            error_code,
        }
    };

    let Ok(input) = LoopInput::decode(request.payload.as_slice()) else {
        return PortCallResponse {
            call_id: request.call_id.clone(),
            payload: Vec::new(),
            error_code: "invalid_argument".to_owned(),
        };
    };

    if config.command.is_empty() {
        return reply(
            Some(loop_decision::Decision::Fail(Fail {
                reason_code: "missing_acp_command".to_owned(),
            })),
            String::new(),
        );
    }

    // The task payload may be plain text or the GUI's JSON envelope
    // `{"task": ...}` — extract the human text either way.
    let raw = String::from_utf8_lossy(&input.state);
    let task = serde_json::from_str::<Value>(raw.trim())
        .ok()
        .and_then(|v| v.get("task").and_then(Value::as_str).map(str::to_owned))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| raw.to_string());

    match acp_turn(config, &task) {
        Ok(text) => reply(
            Some(loop_decision::Decision::Complete(Complete {
                output_ref: data_uri(&text),
            })),
            String::new(),
        ),
        Err(reason) => reply(
            Some(loop_decision::Decision::Fail(Fail {
                reason_code: reason,
            })),
            String::new(),
        ),
    }
}

/// One full ACP turn against a freshly spawned agent.
fn acp_turn(config: &AcpConfig, task: &str) -> Result<String, String> {
    let mut client = Client::spawn(Spawn {
        command: config.command.clone(),
        args: config.args.clone(),
        env: Vec::new(),
        cwd: Some(config.cwd.clone()),
    })
    .map_err(|e| format!("acp_spawn_{}", shorten(&e.to_string())))?;

    client
        .request(
            "initialize",
            json!({
                "protocolVersion": ACP_VERSION,
                "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false},
                "clientInfo": {"name": "agentos-acp-loop", "version": "0.1.0"},
            }),
            config.timeout,
            &mut |_| Reply::Ignore,
        )
        .map_err(|e| format!("acp_initialize_{}", shorten(&e.to_string())))?;

    let session = client
        .request(
            "session/new",
            json!({"cwd": config.cwd, "mcpServers": []}),
            config.timeout,
            &mut |_| Reply::Ignore,
        )
        .map_err(|e| format!("acp_session_{}", shorten(&e.to_string())))?;
    let session_id = session
        .get("sessionId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();

    let mut text = String::new();
    let allow_tools = config.allow_tools;
    client
        .request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": task}],
            }),
            config.timeout,
            &mut |incoming| match incoming {
                Incoming::Notification { method, params } => {
                    if method == "session/update" {
                        let update = &params["update"];
                        let kind = update["sessionUpdate"].as_str().unwrap_or_default();
                        if (kind == "agent_message_chunk" || kind == "agent_message")
                            && let Some(t) = update
                                .get("content")
                                .and_then(|c| c.get("text"))
                                .and_then(Value::as_str)
                        {
                            text.push_str(t);
                        }
                    }
                    Reply::Ignore
                }
                Incoming::Request(req) => {
                    if req.method == "session/request_permission" {
                        return permission_reply(&req.params, allow_tools);
                    }
                    Reply::Error {
                        code: -32601,
                        message: "unsupported client capability".to_owned(),
                    }
                }
            },
        )
        .map_err(|e| format!("acp_prompt_{}", shorten(&e.to_string())))?;

    Ok(text)
}

fn permission_reply(params: &Value, allow_tools: bool) -> Reply {
    if !allow_tools {
        return Reply::Result(json!({"outcome": {"outcome": "cancelled"}}));
    }
    // Pick the first option whose kind permits the action.
    let option = params
        .get("options")
        .and_then(Value::as_array)
        .and_then(|opts| {
            opts.iter()
                .find(|o| {
                    o.get("kind")
                        .and_then(Value::as_str)
                        .map(|k| k.starts_with("allow"))
                        .unwrap_or(false)
                })
                .or_else(|| opts.first())
        })
        .and_then(|o| o.get("optionId"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Reply::Result(json!({
        "outcome": {"outcome": "selected", "optionId": option},
    }))
}

fn parse_args(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(v) = serde_json::from_str::<Value>(trimmed)
        && let Some(arr) = v.as_array()
    {
        return arr
            .iter()
            .filter_map(|a| a.as_str().map(str::to_owned))
            .collect();
    }
    trimmed.split_whitespace().map(str::to_owned).collect()
}

fn shorten(s: &str) -> String {
    s.chars().take(80).collect()
}

fn data_uri(text: &str) -> String {
    let mut end = MAX_OUTPUT_BYTES.min(text.len());
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
