//! OpenRouter-backed AgentLoop adapter.
//!
//! Speaks the framed adapter protocol on fd 0 (socketpair), then serves
//! `agent_loop.next` PortCallRequests by asking an OpenRouter chat model
//! for the next `LoopDecision`. The model's job each turn: given the
//! task payload (carried in `LoopInput.state`, utf-8), decide one action
//! and answer with a single JSON object:
//!
//!   {"complete": {"output": "<final answer text>"}}
//!   {"fail": {"reason_code": "<snake_case code>", "reason": "<why>"}}
//!   {"wait": {"reason": "<what it is waiting for>"}}
//!   {"request_approval": {"operation": "<op>", "reason": "<why>"}}
//!
//! `invoke_effect` and `spawn_agent` are intentionally not offered: this
//! adapter is a minimal real-inference loop, not a tool-using runtime.
//!
//! Env:
//! - `OPENROUTER_API_KEY` (required) — passed through from the daemon env.
//! - `OPENROUTER_MODEL` — chat model id (default `openrouter/free`).
//! - `OPENROUTER_BASE_URL` — default `https://openrouter.ai/api/v1`.
//! - `OPENROUTER_SITE`, `OPENROUTER_APP_NAME` — optional referer headers.
//! - `OPENROUTER_TIMEOUT_MS` — HTTP timeout (default 55000).
//!
//! A `complete` decision's output text is returned as a `data:` URI in
//! `output_ref` (capped, so the run record stays small).

use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::time::Duration;

use adapter_protocol::framing::{read_frame, write_frame};
use domain::generated::contract::{
    AdapterFrame, AdapterHello, AdapterPong, Complete, Fail, LoopDecision, LoopInput,
    PortCallResponse, RequestApproval, Wait, adapter_frame::Body, loop_decision,
};
use prost::Message;
use serde_json::{Value, json};

const PORT_ID: &str = "agent_loop";
const PROTOCOL_VERSION: u32 = 1;
const MAX_OUTPUT_BYTES: usize = 24 * 1024;

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

struct LlmConfig {
    api_key: String,
    model: String,
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

    let config = LlmConfig {
        api_key: env("OPENROUTER_API_KEY").unwrap_or_default(),
        model: env("OPENROUTER_MODEL").unwrap_or_else(|_| "openrouter/free".to_owned()),
        base_url: env("OPENROUTER_BASE_URL")
            .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_owned())
            .trim_end_matches('/')
            .to_owned(),
        site: env("OPENROUTER_SITE").ok(),
        app_name: env("OPENROUTER_APP_NAME").ok(),
        timeout: env("OPENROUTER_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_millis)
            .unwrap_or_else(|| Duration::from_millis(55_000)),
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
    config: &LlmConfig,
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

    if config.api_key.is_empty() {
        return reply(
            Some(loop_decision::Decision::Fail(Fail {
                reason_code: "missing_openrouter_api_key".to_owned(),
            })),
            String::new(),
        );
    }

    let Ok(input) = LoopInput::decode(request.payload.as_slice()) else {
        return PortCallResponse {
            call_id: request.call_id.clone(),
            payload: Vec::new(),
            error_code: "invalid_argument".to_owned(),
        };
    };
    let task = String::from_utf8_lossy(&input.state);
    let events = String::from_utf8_lossy(&input.events);

    let content = match call_model(config, &task, &events, input.step_sequence) {
        Ok(c) => c,
        Err(reason) => {
            eprintln!("openrouter-loop: model call failed: {reason}");
            return reply(
                Some(loop_decision::Decision::Fail(Fail {
                    reason_code: "llm_call_failed".to_owned(),
                })),
                String::new(),
            );
        }
    };

    let decision = parse_model_decision(&content, &input.run_id);
    reply(Some(decision), String::new())
}

fn call_model(config: &LlmConfig, task: &str, events: &str, step: u64) -> Result<String, String> {
    let system = "You are the decision loop of an agent operating system. \
        On each turn you receive the task (what the user asked the agent to \
        do) and the current step number. Reply with EXACTLY ONE JSON object \
        and nothing else — no prose, no markdown fences. Choose one:\n\
        {\"complete\": {\"output\": \"<the final answer/result text>\"}}\n\
        {\"fail\": {\"reason_code\": \"<snake_case>\"}}\n\
        {\"wait\": {\"reason\": \"<what you are waiting for>\"}}\n\
        {\"request_approval\": {\"operation\": \"<op>\", \"reason\": \"<why a human must approve>\"}}\n\n\
        Prefer \"complete\" with the best answer you can produce in one \
        shot. Use \"request_approval\" only for genuinely risky/irreversible \
        intent. Use \"wait\" only if the task explicitly says to pause.";
    let user = if events.is_empty() {
        format!("step: {step}\ntask: {task}")
    } else {
        format!("step: {step}\ntask: {task}\nnew_events: {events}")
    };
    let body = json!({
        "model": config.model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "temperature": 0.2,
        "response_format": {"type": "json_object"},
    });
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
    let mut response = req.send_json(&body).map_err(|e| e.to_string())?;
    let payload: Value = response.body_mut().read_json().map_err(|e| e.to_string())?;
    let Some(choice) = payload["choices"][0]["message"]["content"].as_str() else {
        return Err(format!("unexpected response shape: {payload}"));
    };
    Ok(choice.to_owned())
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

fn data_uri(text: &str) -> String {
    let capped: String = text.chars().take(MAX_OUTPUT_BYTES).collect();
    format!(
        "data:text/plain;base64,{}",
        base64_encode(capped.as_bytes())
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
