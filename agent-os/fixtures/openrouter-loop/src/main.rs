//! OpenRouter-driven AgentLoop adapter.
//!
//! Speaks the framed adapter protocol on fd 0 (socketpair). This loop
//! performs no network I/O itself: the model call is requested as a
//! durable `invoke_effect` decision (`model.chat`) so the kernel routes
//! it through the Effect Coordinator — fenced, idempotent, reconciled —
//! to the bound `effect.execute` adapter (`fixtures/openrouter-effect`).
//!
//! Turn flow per run:
//!   1. No settled model effect yet -> `InvokeEffect{operation:
//!      "model.chat", payload: <chat-completions request JSON>}`; the
//!      run parks in `WaitingTool` while the effect executes.
//!   2. Kernel feeds settled effect outcomes back in `LoopInput.events`
//!      (JSON array) -> the committed `result_ref` data URI carries the
//!      assistant's reply, which is parsed as the run's next decision:
//!
//!   {"complete": {"output": "<final answer text>"}}
//!   {"fail": {"reason_code": "<snake_case code>", "reason": "<why>"}}
//!   {"wait": {"reason": "<what it is waiting for>"}}
//!   {"request_approval": {"operation": "<op>", "reason": "<why>"}}
//!
//! Env:
//! - `OPENROUTER_MODEL` — chat model id embedded in the effect payload
//!   (default `openrouter/free`); the bound effect adapter applies its
//!   own default when the field is absent.
//!
//! A `complete` decision's output text is returned as a `data:` URI in
//! `output_ref` (capped, so the run record stays small).

use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use adapter_protocol::framing::{read_frame, write_frame};
use domain::generated::contract::{
    AdapterFrame, AdapterHello, AdapterPong, Complete, Fail, InvokeEffect, LoopDecision, LoopInput,
    PortCallResponse, RequestApproval, Wait, adapter_frame::Body, loop_decision,
};
use prost::Message;
use serde_json::{Value, json};

const PORT_ID: &str = "agent_loop";
const PROTOCOL_VERSION: u32 = 1;
const MAX_OUTPUT_BYTES: usize = 24 * 1024;
const MODEL_CHAT_OP: &str = "model.chat";
const MCP_CALL_OP: &str = "mcp.call_tool";
/// Bounded tool-call loop: a run may chain at most this many `mcp.*`
/// effects before the loop forces a completion off the last result.
const MAX_MCP_HOPS: u32 = 4;

/// The daemon's fixed bootstrap principal/actor identities — required
/// fields on the `CreateApprovalRequest` draft.
const DAEMON_PRINCIPAL: &str = "00000000-0000-7000-8000-0000000000dd";
const DAEMON_ACTOR: &str = "00000000-0000-7000-8000-0000000000ae";

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("openrouter-loop: {e}");
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

    let model = env("OPENROUTER_MODEL").unwrap_or_else(|_| "openrouter/free".to_owned());

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
        let response = decide(&request, &model);
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

fn decide(request: &domain::generated::contract::PortCallRequest, model: &str) -> PortCallResponse {
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

    let raw = String::from_utf8_lossy(&input.state);
    let envelope = serde_json::from_str::<Value>(raw.trim()).ok();
    let get = |key: &str| {
        envelope
            .as_ref()
            .and_then(|v| v.get(key))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let task = get("task").unwrap_or_else(|| raw.to_string());
    // MCP tool use is opt-in per run: `{"tools": true}` in the task
    // envelope. The model may then answer with `{"mcp_call": {...}}`.
    let tools_enabled = envelope
        .as_ref()
        .and_then(|v| v.get("tools").or_else(|| v.get("mcp")))
        .map(|v| v.as_bool().unwrap_or(false) || v.as_str() == Some("true"))
        .unwrap_or(false);
    let mcp_hops = settled_effects(&input.events)
        .iter()
        .filter(|e| {
            e.get("operation")
                .and_then(Value::as_str)
                .map(|o| o.starts_with("mcp."))
                .unwrap_or(false)
        })
        .count() as u32;

    // Branch on the NEWEST settled effect only — settled effects persist
    // in the event feed, so checking "any mcp effect" would re-dispatch
    // the same tool result forever.
    let latest = settled_effects(&input.events).last().cloned();
    if let Some(effect) = latest
        && effect
            .get("operation")
            .and_then(Value::as_str)
            .map(|o| o.starts_with("mcp."))
            .unwrap_or(false)
    {
        let state = effect
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if mcp_hops >= MAX_MCP_HOPS {
            return reply(
                Some(loop_decision::Decision::Fail(Fail {
                    reason_code: "mcp_hop_limit".to_owned(),
                })),
                String::new(),
            );
        }
        let note = match state {
            "committed" => {
                let result = effect
                    .get("result_ref")
                    .and_then(Value::as_str)
                    .and_then(decode_data_uri)
                    .unwrap_or_default();
                format!("The MCP tool call succeeded with result:\n{result}")
            }
            "failed" => {
                let err = effect
                    .get("error_code")
                    .and_then(Value::as_str)
                    .unwrap_or("tool call failed");
                format!("The MCP tool call failed: {err}")
            }
            _ => {
                return reply(
                    Some(loop_decision::Decision::Fail(Fail {
                        reason_code: format!("mcp_effect_{state}"),
                    })),
                    String::new(),
                );
            }
        };
        let model = get("model").unwrap_or_else(|| model.to_owned());
        let mut request = chat_request(&model, &task, tools_enabled);
        if let Some(m) = request["messages"].as_array_mut() {
            m.push(json!({"role": "user", "content": note}));
        }
        if let Some(url) = get("base_url") {
            request["base_url"] = Value::String(url);
        }
        return reply(
            Some(loop_decision::Decision::InvokeEffect(InvokeEffect {
                operation: MODEL_CHAT_OP.to_owned(),
                payload: serde_json::to_vec(&request).unwrap_or_default(),
                effect_claim: Vec::new(),
            })),
            String::new(),
        );
    }

    // A settled model effect means the coordinator already executed the
    // chat call; its result drives this run's next decision.
    if let Some(effect) = latest_model_effect(&input.events) {
        let state = effect
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match state {
            "committed" => {
                let result_ref = effect
                    .get("result_ref")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let Some(content) = decode_data_uri(result_ref) else {
                    return reply(
                        Some(loop_decision::Decision::Fail(Fail {
                            reason_code: "model_result_undecodable".to_owned(),
                        })),
                        String::new(),
                    );
                };
                // The model asked to call an MCP tool — route it through
                // the Effect Coordinator like the model call itself.
                if tools_enabled
                    && mcp_hops < MAX_MCP_HOPS
                    && let Some(call) = parse_mcp_call(&content)
                {
                    return reply(
                        Some(loop_decision::Decision::InvokeEffect(InvokeEffect {
                            operation: MCP_CALL_OP.to_owned(),
                            payload: serde_json::to_vec(&call).unwrap_or_default(),
                            effect_claim: Vec::new(),
                        })),
                        String::new(),
                    );
                }
                return reply(
                    Some(parse_model_decision(&content, &input.run_id)),
                    String::new(),
                );
            }
            "failed" => {
                return reply(
                    Some(loop_decision::Decision::Fail(Fail {
                        reason_code: effect
                            .get("error_code")
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .unwrap_or("model_effect_failed")
                            .to_owned(),
                    })),
                    String::new(),
                );
            }
            _ => {
                return reply(
                    Some(loop_decision::Decision::Fail(Fail {
                        reason_code: format!("model_effect_{state}"),
                    })),
                    String::new(),
                );
            }
        }
    }

    // No settled effect yet — request the first model call through the
    // Effect Coordinator. The run task may be a plain string or a JSON
    // envelope `{"task": ..., "model"?: ..., "base_url"?: ..., "tools"?:
    // true}` the GUI emits when a non-default OpenAI-compatible provider
    // or MCP tool use is selected; the base URL is enforced by the
    // effect adapter's endpoint guard.
    let effective_model = get("model").unwrap_or_else(|| model.to_owned());
    let mut request = chat_request(&effective_model, &task, tools_enabled);
    if let Some(url) = get("base_url") {
        request["base_url"] = Value::String(url);
    }
    reply(
        Some(loop_decision::Decision::InvokeEffect(InvokeEffect {
            operation: MODEL_CHAT_OP.to_owned(),
            payload: serde_json::to_vec(&request).unwrap_or_default(),
            effect_claim: Vec::new(),
        })),
        String::new(),
    )
}

/// The newest settled `model.chat` outcome in the events batch, if any.
fn latest_model_effect(events: &[u8]) -> Option<Value> {
    latest_effect_matching(events, |o| o == MODEL_CHAT_OP)
}

fn settled_effects(events: &[u8]) -> Vec<Value> {
    serde_json::from_slice::<Value>(events)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
}

fn latest_effect_matching(events: &[u8], pred: impl Fn(&str) -> bool) -> Option<Value> {
    settled_effects(events)
        .iter()
        .rev()
        .find(|e| {
            e.get("operation")
                .and_then(Value::as_str)
                .map(&pred)
                .unwrap_or(false)
        })
        .cloned()
}

/// `{"mcp_call": {"server": ..., "tool": ..., "arguments": {...}}}`
/// out of a model reply, normalised into the executor's payload shape.
fn parse_mcp_call(content: &str) -> Option<Value> {
    let trimmed = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    let call = parsed.get("mcp_call")?;
    let server = call.get("server").and_then(Value::as_str)?;
    let tool = call.get("tool").and_then(Value::as_str)?;
    Some(json!({
        "op": MCP_CALL_OP,
        "server": server,
        "tool": tool,
        "arguments": call.get("arguments").cloned().unwrap_or(json!({})),
    }))
}

fn chat_request(model: &str, task: &str, tools_enabled: bool) -> Value {
    let system = "You are the decision loop of an agent operating system. \
        You receive the task (what the user asked the agent to do). \
        Reply with EXACTLY ONE JSON object and nothing else — no prose, \
        no markdown fences. Choose one:\n\
        {\"complete\": {\"output\": \"<the final answer/result text>\"}}\n\
        {\"fail\": {\"reason_code\": \"<snake_case>\"}}\n\
        {\"wait\": {\"reason\": \"<what you are waiting for>\"}}\n\
        {\"request_approval\": {\"operation\": \"<op>\", \"reason\": \"<why a human must approve>\"}}\n\n\
        Prefer \"complete\" with the best answer you can produce in one \
        shot. Use \"request_approval\" only for genuinely risky/irreversible \
        intent. Use \"wait\" only if the task explicitly says to pause.";
    let tools_clause = if tools_enabled {
        "\n\nYou may also call an MCP tool by replying \
        {\"mcp_call\": {\"server\": \"<server name>\", \"tool\": \"<tool>\", \
        \"arguments\": {...}}} — the kernel executes it and hands you the \
        result on the next turn."
    } else {
        ""
    };
    let system = format!("{system}{tools_clause}");
    json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": format!("task: {task}")},
        ],
        "temperature": 0.2,
        "response_format": {"type": "json_object"},
    })
}

fn parse_model_decision(content: &str, run_id: &str) -> loop_decision::Decision {
    let trimmed = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            // Model answered with prose — treat the whole reply as the output.
            return loop_decision::Decision::Complete(Complete {
                output_ref: data_uri(trimmed),
            });
        }
    };
    if let Some(v) = parsed.get("complete") {
        let output = v.get("output").and_then(|s| s.as_str()).unwrap_or_default();
        return loop_decision::Decision::Complete(Complete {
            output_ref: data_uri(output),
        });
    }
    if let Some(v) = parsed.get("fail") {
        return loop_decision::Decision::Fail(Fail {
            reason_code: v
                .get("reason_code")
                .and_then(|s| s.as_str())
                .unwrap_or("model_failed")
                .to_owned(),
        });
    }
    if let Some(v) = parsed.get("wait") {
        return loop_decision::Decision::Wait(Wait {
            reason: v
                .get("reason")
                .and_then(|s| s.as_str())
                .unwrap_or("model requested wait")
                .to_owned(),
            timer_id: String::new(),
        });
    }
    if let Some(v) = parsed.get("request_approval") {
        let draft = domain::generated::contract::CreateApprovalRequest {
            request_id: String::new(),
            request_digest: String::new(),
            principal_id: DAEMON_PRINCIPAL.to_owned(),
            actor_id: DAEMON_ACTOR.to_owned(),
            run_id: run_id.to_owned(),
            operation: v
                .get("operation")
                .and_then(|s| s.as_str())
                .unwrap_or("llm.requested_approval")
                .to_owned(),
            target_resource: v
                .get("reason")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_owned(),
            capability_ids: Vec::new(),
            extension_bundle_digest: String::new(),
            config_generation_digest: String::new(),
            expires_at_ms: 0,
            nonce: String::new(),
        };
        return loop_decision::Decision::RequestApproval(RequestApproval {
            approval_draft: draft.encode_to_vec(),
        });
    }
    loop_decision::Decision::Fail(Fail {
        reason_code: "unrecognized_model_decision".to_owned(),
    })
}

/// Decodes a `data:<mime>;base64,<body>` URI into text.
fn decode_data_uri(uri: &str) -> Option<String> {
    let (_, body) = uri.split_once(";base64,")?;
    let bytes = base64_decode(body)?;
    String::from_utf8(bytes).ok()
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let table = |c: u8| T.iter().position(|&t| t == c).map(|p| p as u8);
    let bytes: Vec<u8> = input.bytes().filter(|b| *b != b'=').collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            return None;
        }
        let vals: Option<Vec<u8>> = chunk.iter().map(|&b| table(b)).collect();
        let v = vals?;
        let n = (u32::from(v[0]) << 18)
            | (u32::from(*v.get(1)?) << 12)
            | v.get(2).map_or(0, |c| u32::from(*c) << 6)
            | v.get(3).map_or(0, |c| u32::from(*c));
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
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
