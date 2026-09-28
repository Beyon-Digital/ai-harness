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
//! Creative mode (`{"creative": true}` in the task envelope) adds two
//! more replies — `{"harness_call": {"op": "harness.*", ...}}` for the
//! self-extension tool surface, and `{"spawn_agent": {"task": ...}}`
//! for child runs (spawn context comes from the envelope's
//! `spawn_defaults`; child outcomes arrive as `kernel.child_settled`
//! entries when the run leaves `WaitingChild`).
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
use std::path::PathBuf;
use std::process::ExitCode;

use adapter_protocol::framing::{read_frame, write_frame};
use domain::generated::contract::{
    AdapterFrame, AdapterHello, AdapterPong, Complete, CreateTaskRun, Fail, InvokeEffect,
    LoopDecision, LoopInput, PortCallResponse, RequestApproval, SpawnAgent, VersionedRef, Wait,
    adapter_frame::Body, loop_decision,
};
use prost::Message;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const PORT_ID: &str = "agent_loop";
const PROTOCOL_VERSION: u32 = 1;
const MAX_OUTPUT_BYTES: usize = 24 * 1024;
const MODEL_CHAT_OP: &str = "model.chat";
const MCP_CALL_OP: &str = "mcp.call_tool";
/// Bounded tool-call loop: a run may chain at most this many `mcp.*`
/// effects before the loop forces a completion off the last result.
const MAX_MCP_HOPS: u32 = 4;
/// Creative mode runs longer tool loops — scaffold/validate/write is
/// multi-step — but still bounded: at most this many `harness.*` ops.
const MAX_HARNESS_HOPS: u32 = 16;
/// Default child-run cap per parent; the envelope's `max_children`
/// overrides it.
const DEFAULT_MAX_CHILDREN: u64 = 8;

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
    let tools_required = tools_enabled
        && envelope
            .as_ref()
            .and_then(|v| v.get("tool_choice"))
            .and_then(Value::as_str)
            == Some("required");
    // Creative mode (`{"creative": true}` in the envelope) unlocks the
    // `harness.*` tool surface and `spawn_agent` replies.
    let creative = envelope
        .as_ref()
        .and_then(|v| v.get("creative"))
        .map(|v| v.as_bool().unwrap_or(false) || v.as_str() == Some("true"))
        .unwrap_or(false);
    // Run-wide settled `mcp.*` effects — accumulates across steps and
    // survives daemon restarts, so the hop cap cannot be evaded by
    // alternating model/tool turns.
    let mcp_hops = op_count(&input.events, "mcp.");
    let harness_hops = op_count(&input.events, "harness.");
    let model_hops = op_count(&input.events, MODEL_CHAT_OP);

    // Branch on the NEWEST settled effect only — the feed contains just
    // the current step's outcomes, so checking "any mcp effect" would
    // re-dispatch the same tool result forever.
    let latest = settled_effects(&input.events).last().cloned();
    if let Some(ref effect) = latest
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
        // The hop cap bounds NEW tool dispatches, not result delivery: a
        // settled result still feeds one final model call with tools off
        // so the run can complete with an answer.
        let final_only = mcp_hops >= MAX_MCP_HOPS;
        let note = match state {
            "committed" => {
                let result = effect
                    .get("result_ref")
                    .and_then(Value::as_str)
                    .and_then(decode_data_uri)
                    .unwrap_or_default();
                if let Some(next) = next_planned_call(&result) {
                    if final_only {
                        return reply(
                            Some(loop_decision::Decision::Fail(Fail {
                                reason_code: "mcp_hop_limit".to_owned(),
                            })),
                            String::new(),
                        );
                    }
                    return reply(
                        Some(loop_decision::Decision::InvokeEffect(InvokeEffect {
                            operation: MCP_CALL_OP.to_owned(),
                            payload: serde_json::to_vec(&next).unwrap_or_default(),
                            effect_claim: Vec::new(),
                        })),
                        String::new(),
                    );
                }
                if let Some(output) = planned_final_output(&result) {
                    return reply(
                        Some(loop_decision::Decision::Complete(Complete {
                            output_ref: data_uri(&output),
                        })),
                        String::new(),
                    );
                }
                format!(
                    "The MCP tool call succeeded with result:\n{}",
                    summarize_mcp_result(&result)
                )
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
        let mut request =
            chat_request(&model, &task, tools_enabled && !final_only, false, creative);
        if let Some(m) = request["messages"].as_array_mut() {
            let content = mcp_follow_up_content(&note, &result_for_note(&latest), mcp_hops);
            m.push(json!({
                "role": "user",
                "content": content
            }));
            if final_only {
                m.push(json!({
                    "role": "user",
                    "content": "The MCP tool-call budget is exhausted. Do not request another tool. Return `complete` with the available result, or `fail`."
                }));
            }
        }
        if let Some(url) = get("base_url") {
            request["base_url"] = Value::String(url);
        }
        if let Some(ke) = get("api_key_env") {
            request["api_key_env"] = Value::String(ke);
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

    // Settled `harness.*` ops feed back exactly like `mcp.*` — the
    // model sees the op's JSON result and decides the next step.
    if let Some(ref effect) = latest
        && effect
            .get("operation")
            .and_then(Value::as_str)
            .map(|o| o.starts_with("harness."))
            .unwrap_or(false)
    {
        let op = effect
            .get("operation")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let state = effect
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let final_only = harness_hops >= MAX_HARNESS_HOPS;
        let note = match state {
            "committed" => {
                let result = effect
                    .get("result_ref")
                    .and_then(Value::as_str)
                    .and_then(decode_data_uri)
                    .unwrap_or_default();
                format!("The harness op `{op}` succeeded with result:\n{result}")
            }
            "failed" => {
                let err = effect
                    .get("error_code")
                    .and_then(Value::as_str)
                    .unwrap_or("harness op failed");
                format!("The harness op `{op}` failed: {err}")
            }
            _ => {
                return reply(
                    Some(loop_decision::Decision::Fail(Fail {
                        reason_code: format!("harness_effect_{state}"),
                    })),
                    String::new(),
                );
            }
        };
        let model = get("model").unwrap_or_else(|| model.to_owned());
        let mut request = chat_request(&model, &task, tools_enabled, false, creative);
        if let Some(m) = request["messages"].as_array_mut() {
            m.push(json!({
                "role": "user",
                "content": format!(
                    "Harness op #{harness_hops} has finished. {note}\n\
                     Continue with the next action from the task, or return \
                     `complete` when it is done."
                )
            }));
            if final_only {
                m.push(json!({
                    "role": "user",
                    "content": "The harness-op budget is exhausted. Do not request another harness op. Return `complete` with what you have, or `fail`."
                }));
            }
        }
        if let Some(url) = get("base_url") {
            request["base_url"] = Value::String(url);
        }
        if let Some(ke) = get("api_key_env") {
            request["api_key_env"] = Value::String(ke);
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

    // `WaitingChild` resume: the kernel fed `kernel.child_settled`
    // entries for every spawned child — summarize them back to the
    // model so it can synthesize or continue.
    if latest
        .as_ref()
        .and_then(|e| e.get("operation"))
        .and_then(Value::as_str)
        == Some("kernel.child_settled")
    {
        let children: Vec<String> = settled_effects(&input.events)
            .iter()
            .filter(|e| e.get("operation").and_then(Value::as_str) == Some("kernel.child_settled"))
            .map(|e| {
                let run = e.get("run_id").and_then(Value::as_str).unwrap_or("?");
                let state = e.get("state").and_then(Value::as_str).unwrap_or("?");
                let task = e
                    .get("task_excerpt")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let result = e
                    .get("result_ref")
                    .and_then(Value::as_str)
                    .and_then(decode_data_uri)
                    .unwrap_or_default();
                let error = e.get("error_code").and_then(Value::as_str).unwrap_or("");
                format!("- child {run} ({state}) task `{task}`: {result}{error}")
            })
            .collect();
        let model = get("model").unwrap_or_else(|| model.to_owned());
        let mut request = chat_request(&model, &task, tools_enabled, false, creative);
        if let Some(m) = request["messages"].as_array_mut() {
            m.push(json!({
                "role": "user",
                "content": format!(
                    "Every spawned child agent has finished:\n{}\n\
                     Synthesize their results and continue the task — spawn \
                     more children only if genuinely needed, or return `complete`.",
                    children.join("\n")
                )
            }));
        }
        if let Some(url) = get("base_url") {
            request["base_url"] = Value::String(url);
        }
        if let Some(ke) = get("api_key_env") {
            request["api_key_env"] = Value::String(ke);
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
                let mcp_plan = parse_mcp_plan(&content, &task);
                let mcp_call = parse_mcp_call(&content);
                if tools_required && mcp_hops == 0 {
                    if let Some(call) = mcp_plan {
                        return reply(
                            Some(loop_decision::Decision::InvokeEffect(InvokeEffect {
                                operation: MCP_CALL_OP.to_owned(),
                                payload: serde_json::to_vec(&call).unwrap_or_default(),
                                effect_claim: Vec::new(),
                            })),
                            String::new(),
                        );
                    }
                    if model_hops < 2 {
                        let model = get("model").unwrap_or_else(|| model.to_owned());
                        let mut request = chat_request(&model, &task, true, true, creative);
                        if let Some(messages) = request["messages"].as_array_mut() {
                            messages.push(json!({
                                "role": "assistant",
                                "content": content,
                            }));
                            messages.push(json!({
                                "role": "user",
                                "content": "That response did not provide the required tool plan. Reply now with exactly one `mcp_plan` JSON object containing every computer action in order.",
                            }));
                        }
                        if let Some(url) = get("base_url") {
                            request["base_url"] = Value::String(url);
                        }
                        if let Some(ke) = get("api_key_env") {
                            request["api_key_env"] = Value::String(ke);
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
                    return reply(
                        Some(loop_decision::Decision::Fail(Fail {
                            reason_code: "tool_plan_required".to_owned(),
                        })),
                        String::new(),
                    );
                }
                if tools_enabled
                    && mcp_hops < MAX_MCP_HOPS
                    && let Some(call) = mcp_call
                {
                    if latest_mcp_request_hash(&input.events).as_deref()
                        == Some(request_hash(&call).as_str())
                    {
                        let first_duplicate = model_hops <= mcp_hops.saturating_add(1);
                        let retry_duplicate = model_hops <= mcp_hops.saturating_add(2);
                        if first_duplicate || retry_duplicate {
                            let model = get("model").unwrap_or_else(|| model.to_owned());
                            let keep_tools = first_duplicate && mcp_hops == 1;
                            let mut request =
                                chat_request(&model, &task, keep_tools, false, creative);
                            if let Some(messages) = request["messages"].as_array_mut() {
                                messages.push(json!({
                                    "role": "assistant",
                                    "content": content,
                                }));
                                if keep_tools {
                                    messages.push(json!({
                                        "role": "user",
                                        "content": format!(
                                            "That repeats the most recently completed MCP call. \
                                             {mcp_hops} tool call has already finished. Choose \
                                             the next different action from the task, or return \
                                             `complete` if no action remains."
                                        ),
                                    }));
                                } else {
                                    messages.push(json!({
                                        "role": "user",
                                        "content": format!(
                                            "{mcp_hops} computer tool calls have already finished, \
                                             and the latest request repeats the last completed call. \
                                             Tool use is now closed. Return `complete` with the final \
                                             result using the available evidence. If the task names \
                                             exact response text, use it verbatim."
                                        ),
                                    }));
                                }
                            }
                            if let Some(url) = get("base_url") {
                                request["base_url"] = Value::String(url);
                            }
                            if let Some(ke) = get("api_key_env") {
                                request["api_key_env"] = Value::String(ke);
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
                        return reply(
                            Some(loop_decision::Decision::Fail(Fail {
                                reason_code: "repeated_mcp_call".to_owned(),
                            })),
                            String::new(),
                        );
                    }
                    return reply(
                        Some(loop_decision::Decision::InvokeEffect(InvokeEffect {
                            operation: MCP_CALL_OP.to_owned(),
                            payload: serde_json::to_vec(&call).unwrap_or_default(),
                            effect_claim: Vec::new(),
                        })),
                        String::new(),
                    );
                }
                if tools_enabled && mcp_hops >= MAX_MCP_HOPS && parse_mcp_call(&content).is_some() {
                    return reply(
                        Some(loop_decision::Decision::Fail(Fail {
                            reason_code: "mcp_hop_limit".to_owned(),
                        })),
                        String::new(),
                    );
                }
                // Creative mode: the model asked for a harness op — same
                // durable dispatch as model.chat/mcp.*. The op string is
                // carried in the request payload, not the effect
                // operation (the profile binds one adapter for all
                // effect.execute calls).
                if creative && let Some(call) = parse_harness_call(&content) {
                    if harness_hops >= MAX_HARNESS_HOPS {
                        return reply(
                            Some(loop_decision::Decision::Fail(Fail {
                                reason_code: "harness_hop_limit".to_owned(),
                            })),
                            String::new(),
                        );
                    }
                    if latest_tool_request_hash(&input.events).as_deref()
                        == Some(request_hash(&call).as_str())
                    {
                        return reply(
                            Some(loop_decision::Decision::Fail(Fail {
                                reason_code: "repeated_harness_call".to_owned(),
                            })),
                            String::new(),
                        );
                    }
                    let operation = call
                        .get("op")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    return reply(
                        Some(loop_decision::Decision::InvokeEffect(InvokeEffect {
                            operation,
                            payload: serde_json::to_vec(&call).unwrap_or_default(),
                            effect_claim: Vec::new(),
                        })),
                        String::new(),
                    );
                }
                // Creative mode: spawn a child agent run.
                if creative && let Some(spawn) = parse_spawn_agent(&content) {
                    return match build_child_request(&spawn, &input, envelope.as_ref()) {
                        Ok(child_request) => reply(
                            Some(loop_decision::Decision::SpawnAgent(SpawnAgent {
                                child_request,
                            })),
                            String::new(),
                        ),
                        Err(code) => reply(
                            Some(loop_decision::Decision::Fail(Fail { reason_code: code })),
                            String::new(),
                        ),
                    };
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
    let mut request = chat_request(
        &effective_model,
        &task,
        tools_enabled,
        tools_required,
        creative,
    );
    if let Some(url) = get("base_url") {
        request["base_url"] = Value::String(url);
    }
    if let Some(ke) = get("api_key_env") {
        request["api_key_env"] = Value::String(ke);
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

/// Marker operation that carries run-wide `op_counts` as the first
/// element of the `events` array — keeps the version-1 array shape.
const OP_COUNTS_MARKER: &str = "kernel.op_counts";

/// Events payload: a version-1 array of settled-effect objects whose
/// first element may be the `kernel.op_counts` marker carrying run-wide
/// counts. An object form (`{"settled": [..], "op_counts": {..}}`)
/// produced by transitional daemons is read the same way.
fn events_doc(events: &[u8]) -> Value {
    serde_json::from_slice::<Value>(events).unwrap_or(Value::Null)
}

fn is_marker(e: &Value) -> bool {
    e.get("operation").and_then(Value::as_str) == Some(OP_COUNTS_MARKER)
}

fn settled_effects(events: &[u8]) -> Vec<Value> {
    let batch = match events_doc(events) {
        Value::Array(a) => a,
        v => v
            .get("settled")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
    };
    batch.into_iter().filter(|e| !is_marker(e)).collect()
}

/// Run-wide terminal-effect count for ops matching `prefix` — durable
/// across turns and daemon restarts, unlike the per-step `settled`
/// batch.
fn op_count(events: &[u8], prefix: &str) -> u32 {
    let counts = match events_doc(events) {
        // Marker element carries `{.., "op_counts": {op: n}}`.
        Value::Array(a) => a
            .iter()
            .find(|e| e.get("op_counts").is_some())
            .and_then(|e| e.get("op_counts").cloned()),
        v => v.get("op_counts").cloned(),
    };
    counts
        .as_ref()
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(op, _)| op.starts_with(prefix))
                .map(|(_, n)| n.as_u64().unwrap_or(0))
                .sum::<u64>() as u32
        })
        .unwrap_or_else(|| {
            // Feeds without a counts marker — degrade to the current
            // step's matches (old, lossy behavior).
            settled_effects(events)
                .iter()
                .filter(|e| {
                    e.get("operation")
                        .and_then(Value::as_str)
                        .map(|o| o.starts_with(prefix))
                        .unwrap_or(false)
                })
                .count() as u32
        })
}

fn latest_mcp_request_hash(events: &[u8]) -> Option<String> {
    match events_doc(events) {
        Value::Array(events) => events
            .iter()
            .find(|event| event.get("latest_mcp_request_hash").is_some())
            .and_then(|event| event.get("latest_mcp_request_hash"))
            .and_then(Value::as_str)
            .filter(|hash| !hash.is_empty())
            .map(str::to_owned),
        value => value
            .get("latest_mcp_request_hash")
            .and_then(Value::as_str)
            .filter(|hash| !hash.is_empty())
            .map(str::to_owned),
    }
}

fn request_hash(request: &Value) -> String {
    let digest = Sha256::digest(serde_json::to_vec(request).unwrap_or_default());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Marker's `latest_tool_request_hash` — covers `mcp.*` and `harness.*`
/// alike; falls back to the mcp-only field for older kernels.
fn latest_tool_request_hash(events: &[u8]) -> Option<String> {
    match events_doc(events) {
        Value::Array(events) => events
            .iter()
            .find(|event| event.get("latest_tool_request_hash").is_some())
            .and_then(|event| event.get("latest_tool_request_hash"))
            .and_then(Value::as_str)
            .filter(|hash| !hash.is_empty())
            .map(str::to_owned),
        value => value
            .get("latest_tool_request_hash")
            .and_then(Value::as_str)
            .filter(|hash| !hash.is_empty())
            .map(str::to_owned),
    }
    .or_else(|| latest_mcp_request_hash(events))
}

/// Marker's `child_count` — total children the kernel has recorded
/// under this run (caps `spawn_agent`).
fn child_count(events: &[u8]) -> u64 {
    match events_doc(events) {
        Value::Array(events) => events
            .iter()
            .find(|event| event.get("child_count").is_some())
            .and_then(|event| event.get("child_count"))
            .and_then(Value::as_u64),
        value => value.get("child_count").and_then(Value::as_u64),
    }
    .unwrap_or(0)
}

/// Payload hashes of children already fed via `kernel.child_settled` —
/// blocks a spawn that would duplicate a known child byte-for-byte.
fn child_payload_hashes(events: &[u8]) -> Vec<String> {
    settled_effects(events)
        .iter()
        .filter(|e| e.get("operation").and_then(Value::as_str) == Some("kernel.child_settled"))
        .filter_map(|e| {
            e.get("payload_hash")
                .and_then(Value::as_str)
                .filter(|h| !h.is_empty())
                .map(str::to_owned)
        })
        .collect()
}

/// `{"harness_call": {"op": "harness.*", ...args}}` out of a model
/// reply, normalised into the executor's payload shape. The op must be
/// under `harness.` — other prefixes belong to their own reply kinds.
fn parse_harness_call(content: &str) -> Option<Value> {
    let trimmed = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    let call = parsed.get("harness_call")?;
    let op = call.get("op").and_then(Value::as_str)?;
    if !op.starts_with("harness.") {
        return None;
    }
    let mut payload = call.clone();
    payload["op"] = Value::String(op.to_owned());
    Some(payload)
}

/// `{"spawn_agent": {"task": ..., "envelope"?: {...}, "profile"?: ...}}`
/// out of a model reply.
fn parse_spawn_agent(content: &str) -> Option<Value> {
    let trimmed = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    let spawn = parsed.get("spawn_agent")?;
    if !spawn.get("task").map(Value::is_string).unwrap_or(false) {
        return None;
    }
    Some(spawn.clone())
}

/// Builds the `CreateTaskRun` the kernel submits for `spawn_agent`.
/// Session/spec/profile come from the envelope's `spawn_defaults`
/// (injected by the caller that created this run); task_id and
/// parent_run_id stay kernel-owned — the daemon fills the parent's id
/// and inherits the task when the child leaves it blank.
fn build_child_request(
    spawn: &Value,
    input: &LoopInput,
    envelope: Option<&Value>,
) -> Result<Vec<u8>, String> {
    let defaults = envelope
        .and_then(|e| e.get("spawn_defaults"))
        .ok_or("spawn_not_configured")?;
    let field = |key: &str| {
        defaults
            .get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let session_id = field("session_id").ok_or("spawn_not_configured")?;
    let spec_id = field("agent_spec_id").ok_or("spawn_not_configured")?;
    let spec_version = field("spec_version").ok_or("spawn_not_configured")?;
    let spec_digest = field("spec_digest").ok_or("spawn_not_configured")?;
    let profile = spawn
        .get("profile")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| field("profile"))
        .unwrap_or_default();
    let max_children = envelope
        .and_then(|e| e.get("max_children"))
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_MAX_CHILDREN);
    if child_count(&input.events) >= max_children {
        return Err("child_limit".to_owned());
    }
    let task_text = spawn
        .get("task")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("invalid_spawn")?;
    let mut child_env = spawn.get("envelope").cloned().unwrap_or_else(|| json!({}));
    if !child_env.is_object() {
        return Err("invalid_spawn".to_owned());
    }
    child_env["task"] = Value::String(task_text.to_owned());
    // The child inherits the parent's creative context so it can run
    // harness ops and spawn its own children (the global child cap
    // still applies). Explicit envelope values win.
    for key in ["creative", "tools", "model", "base_url", "api_key_env"] {
        if child_env.get(key).is_none()
            && let Some(value) = envelope.and_then(|e| e.get(key))
        {
            child_env[key] = value.clone();
        }
    }
    if child_env.get("spawn_defaults").is_none() {
        child_env["spawn_defaults"] = defaults.clone();
    }
    let payload = serde_json::to_vec(&child_env).unwrap_or_default();
    let digest = Sha256::digest(&payload);
    let hash: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if child_payload_hashes(&input.events)
        .iter()
        .any(|h| h == &hash)
    {
        return Err("duplicate_spawn".to_owned());
    }
    Ok(CreateTaskRun {
        task_id: String::new(),
        run_id: String::new(),
        session_id,
        task_kind: "agentos.task.v1.Run".to_owned(),
        task_payload: payload,
        agent_spec_ref: Some(VersionedRef {
            id: spec_id,
            version: spec_version,
            digest: spec_digest,
        }),
        parent_run_id: input.run_id.clone(),
        observed_parent_cancellation_epoch: 0,
        requested_profile: profile,
        workspace_uri: String::new(),
        requested_capabilities: Vec::new(),
        requested_budget: Vec::new(),
    }
    .encode_to_vec())
}

/// The harness root — same resolution as the effect adapter's
/// (`AGENTOS_HARNESS_ROOT`, else `~/.agentos/harness`).
fn harness_root() -> Option<PathBuf> {
    if let Ok(root) = std::env::var("AGENTOS_HARNESS_ROOT")
        && !root.trim().is_empty()
    {
        return Some(PathBuf::from(root));
    }
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".agentos").join("harness"))
}

/// `(name, description)` pairs from every `<root>/skills/*/SKILL.md`
/// frontmatter — injected into the creative system prompt so the model
/// can see and invoke installed skills.
fn skill_index() -> Vec<(String, String)> {
    let Some(dir) = harness_root().map(|root| root.join("skills")) else {
        return Vec::new();
    };
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut index: Vec<(String, String)> = read_dir
        .flatten()
        .filter_map(|entry| {
            let file = std::fs::read_to_string(entry.path().join("SKILL.md")).ok()?;
            let front = file
                .strip_prefix("---")?
                .split_once("\n---")
                .map(|(head, _)| head)?;
            let field = |key: &str| {
                front.lines().find_map(|line| {
                    line.trim()
                        .strip_prefix(&format!("{key}:"))
                        .map(|v| v.trim().trim_matches('"').to_owned())
                })
            };
            Some((field("name")?, field("description")?))
        })
        .take(24)
        .collect();
    index.sort();
    index
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

fn parse_mcp_plan(content: &str, task: &str) -> Option<Value> {
    let trimmed = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    let plan = parsed.get("mcp_plan")?;
    let calls = plan.get("calls").and_then(Value::as_array)?;
    if calls.is_empty() || calls.len() > MAX_MCP_HOPS as usize {
        return None;
    }
    let mut normalized = calls
        .iter()
        .map(normalize_mcp_call)
        .collect::<Option<Vec<_>>>()?;
    let mut first = normalized.remove(0);
    first["remaining_calls"] = Value::Array(normalized);
    let final_output = exact_requested_output(task).unwrap_or_default();
    if !final_output.is_empty() {
        first["final_output"] = Value::String(final_output);
    }
    Some(first)
}

fn normalize_mcp_call(call: &Value) -> Option<Value> {
    Some(json!({
        "op": MCP_CALL_OP,
        "server": call.get("server").and_then(Value::as_str)?,
        "tool": call.get("tool").and_then(Value::as_str)?,
        "arguments": call.get("arguments").cloned().unwrap_or_else(|| json!({})),
    }))
}

fn next_planned_call(result: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(result).ok()?;
    let request = value.get("request")?;
    let calls = request.get("remaining_calls").and_then(Value::as_array)?;
    let mut remaining = calls.clone();
    if remaining.is_empty() {
        return None;
    }
    let mut next = normalize_mcp_call(&remaining.remove(0))?;
    next["remaining_calls"] = Value::Array(remaining);
    if let Some(output) = request.get("final_output").and_then(Value::as_str)
        && !output.is_empty()
    {
        next["final_output"] = Value::String(output.to_owned());
    }
    Some(next)
}

fn planned_final_output(result: &str) -> Option<String> {
    let value: Value = serde_json::from_str(result).ok()?;
    let request = value.get("request")?;
    let remaining = request.get("remaining_calls").and_then(Value::as_array)?;
    if !remaining.is_empty() {
        return None;
    }
    request
        .get("final_output")
        .and_then(Value::as_str)
        .filter(|output| !output.is_empty())
        .map(str::to_owned)
}

fn exact_requested_output(task: &str) -> Option<String> {
    let lower = task.to_ascii_lowercase();
    let start = lower.rfind("exactly ")? + "exactly ".len();
    let candidate = task[start..]
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .trim_matches(|character| {
            matches!(
                character,
                '"' | '\'' | '`' | '.' | ',' | ';' | ':' | '!' | '?'
            )
        });
    (!candidate.is_empty() && !candidate.contains(char::is_whitespace))
        .then(|| candidate.to_owned())
}

fn summarize_mcp_result(result: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<Value>(result) else {
        return result.to_owned();
    };
    if let Some(content) = value.get_mut("content").and_then(Value::as_array_mut) {
        for item in content {
            if item.get("type").and_then(Value::as_str) == Some("image") {
                item["data"] = Value::String("[image attached]".to_owned());
            }
        }
    }
    serde_json::to_string(&value).unwrap_or_else(|_| result.to_owned())
}

fn result_for_note(effect: &Option<Value>) -> String {
    effect
        .as_ref()
        .and_then(|value| value.get("result_ref"))
        .and_then(Value::as_str)
        .and_then(decode_data_uri)
        .unwrap_or_default()
}

fn mcp_follow_up_content(note: &str, result: &str, mcp_hops: u32) -> Value {
    let instruction = format!(
        "MCP tool call #{mcp_hops} has finished. {note}\n\
         The `request` field identifies the completed call. Do not immediately \
         repeat that same call. Continue with the next pending action from the \
         task, or return `complete` when all requested actions are finished."
    );
    let images = serde_json::from_str::<Value>(result)
        .ok()
        .and_then(|value| value.get("content").and_then(Value::as_array).cloned())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            if item.get("type").and_then(Value::as_str) != Some("image") {
                return None;
            }
            let mime = item.get("mimeType").and_then(Value::as_str)?;
            let data = item.get("data").and_then(Value::as_str)?;
            (data != "[image attached]").then(|| {
                json!({
                    "type": "image_url",
                    "image_url": {"url": format!("data:{mime};base64,{data}")}
                })
            })
        })
        .collect::<Vec<_>>();
    if images.is_empty() {
        return Value::String(instruction);
    }
    let mut content = vec![json!({"type": "text", "text": instruction})];
    content.extend(images);
    Value::Array(content)
}

fn chat_request(
    model: &str,
    task: &str,
    tools_enabled: bool,
    tools_required: bool,
    creative: bool,
) -> Value {
    let system = "You are the decision loop of an agent operating system. \
        You receive the task (what the user asked the agent to do). \
        Reply with EXACTLY ONE JSON object and nothing else — no prose, \
        no markdown fences. Choose one:\n\
        {\"complete\": {\"output\": \"<the final answer/result text>\"}}\n\
        {\"fail\": {\"reason_code\": \"<snake_case>\"}}\n\
        {\"wait\": {\"reason\": \"<what you are waiting for>\"}}\n\
        {\"request_approval\": {\"operation\": \"<op>\", \"reason\": \"<why a human must approve>\"}}\n\n\
        Use \"complete\" with the best answer you can produce when no more \
        actions are needed. Use \"request_approval\" only for genuinely risky/irreversible \
        intent. Use \"wait\" only if the task explicitly says to pause.";
    let tools_clause = if tools_enabled {
        "\n\nYou may also call an MCP tool by replying \
        {\"mcp_call\": {\"server\": \"<server name>\", \"tool\": \"<tool>\", \
        \"arguments\": {...}}} — the kernel executes it and hands you the \
        result on the next turn. The built-in `computer` server provides \
        screenshot {}, click {x,y,button}, type {text}, key {key}, and \
        wait {milliseconds}. If the task asks you to inspect, operate, type \
        into, click, press keys in, or wait on the computer, you MUST call \
        the appropriate tool instead of narrating or claiming the action. \
        Use screenshot before and after visual actions."
    } else {
        ""
    };
    let required_clause = if tools_required {
        "\n\nComputer mode is active. Reply with exactly one plan object:\n\
        {\"mcp_plan\":{\"calls\":[{\"server\":\"computer\",\"tool\":\"<tool>\",\
        \"arguments\":{}}],\"final_output\":\"<exact literal response only when requested>\"}}\n\
        Include every requested computer action in order, with no more than \
        four calls. Use `final_output` only when the task gives exact final \
        response text; visual answers are produced after inspecting the result. Do \
        not return `complete`, `mcp_call`, prose, or markdown."
    } else {
        ""
    };
    let creative_clause = if creative {
        let skills = skill_index()
            .iter()
            .map(|(name, desc)| format!("- {name} — {desc}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "\n\nCreative mode is active — you may extend the harness itself. \
            Two additional replies are available:\n\
            {{\"harness_call\": {{\"op\": \"<harness op>\", ...args}}}} — the kernel \
            executes the op and hands you its JSON result next turn. Ops:\n\
              harness.catalog  {{path?}}            file tree of the harness root\n\
              harness.list     {{path?}}            one directory listing\n\
              harness.read     {{path}}             file contents\n\
              harness.write    {{path, content, create_only?}}  write a file\n\
              harness.scaffold {{kind, name, description?}}     create an extension\n\
              harness.validate {{path}}             check an extension's files\n\
              harness.register {{kind, name}}       install an extension\n\
              harness.registry {{}}                 list registered extensions\n\
            Every path stays under the harness root. `kind` is one of skill, \
            workflow, adapter, loop, mcp_server. After scaffold + validate, \
            register an mcp_server to call it live through `mcp_call`\n\
            (server = its registered name); skills and workflows install into \
            the registry; adapter/loop register as needs_build (they require a \
            compile + profile binding outside the run).\n\
            {{\"spawn_agent\": {{\"task\": \"<subtask>\", \"envelope\": {{...}}?, \
            \"profile\": \"<profile>\"?}}}} — spawn a child agent run that inherits \
            your settings; you resume automatically when all children finish. \
            Prefer a few well-scoped children.\
            {}",
            if skills.is_empty() {
                String::new()
            } else {
                format!(
                    "\n\nInstalled skills (invoke by naming them in a child's `task` \
                     or by following them yourself):\n{skills}"
                )
            }
        )
    } else {
        String::new()
    };
    let system = format!("{system}{tools_clause}{required_clause}{creative_clause}");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_computer_prompt_requires_an_ordered_plan() {
        let request = chat_request("openrouter/free", "inspect the screen", true, true, false);
        let system = request["messages"][0]["content"]
            .as_str()
            .expect("system prompt");
        assert!(system.contains("\"mcp_plan\""));
        assert!(system.contains("every requested computer action in order"));
        assert!(system.contains("instead of narrating or claiming the action"));
    }

    #[test]
    fn planned_computer_calls_continue_without_another_model_turn() {
        let plan = json!({
            "mcp_plan": {
                "calls": [
                    {"server": "computer", "tool": "screenshot", "arguments": {}},
                    {"server": "computer", "tool": "wait", "arguments": {"milliseconds": 500}},
                    {"server": "computer", "tool": "screenshot", "arguments": {}}
                ],
                "final_output": "ignored"
            }
        });
        let first = parse_mcp_plan(
            &plan.to_string(),
            "Take a screenshot, wait 500 ms, take another screenshot, then say exactly COMPUTER_SEQUENCE_OK.",
        )
        .expect("plan");
        assert_eq!(first["tool"], "screenshot");
        assert_eq!(first["remaining_calls"].as_array().unwrap().len(), 2);
        assert_eq!(first["final_output"], "COMPUTER_SEQUENCE_OK");

        let first_result = json!({"request": first, "content": []}).to_string();
        let second = next_planned_call(&first_result).expect("second call");
        assert_eq!(second["tool"], "wait");
        assert_eq!(second["arguments"]["milliseconds"], 500);

        let second_result = json!({"request": second, "content": []}).to_string();
        let third = next_planned_call(&second_result).expect("third call");
        assert_eq!(third["tool"], "screenshot");
        assert!(third["remaining_calls"].as_array().unwrap().is_empty());

        let third_result = json!({"request": third, "content": []}).to_string();
        assert_eq!(
            planned_final_output(&third_result).as_deref(),
            Some("COMPUTER_SEQUENCE_OK")
        );
    }

    #[test]
    fn screenshot_dependent_plan_does_not_use_prewritten_answer() {
        let plan = json!({
            "mcp_plan": {
                "calls": [
                    {"server": "computer", "tool": "screenshot", "arguments": {}}
                ],
                "final_output": "The screen says something unverified."
            }
        });
        let first = parse_mcp_plan(&plan.to_string(), "What does the screen say?").expect("plan");
        assert!(first.get("final_output").is_none());
        let result = json!({"request": first, "content": []}).to_string();
        assert!(planned_final_output(&result).is_none());
    }

    #[test]
    fn mcp_result_summary_keeps_request_metadata() {
        let result = json!({
            "request": {
                "op": "mcp.call_tool",
                "server": "computer",
                "tool": "screenshot",
                "arguments": {}
            },
            "content": [
                {"type": "text", "text": "captured"},
                {"type": "image", "mimeType": "image/jpeg", "data": "abc"}
            ]
        });
        let summary = summarize_mcp_result(&result.to_string());
        assert!(summary.contains("\"tool\":\"screenshot\""));
        assert!(summary.contains("[image attached]"));
        assert!(!summary.contains("\"data\":\"abc\""));
    }

    #[test]
    fn screenshot_result_is_forwarded_as_vision_input() {
        let result = json!({
            "content": [
                {"type": "text", "text": "captured"},
                {"type": "image", "mimeType": "image/jpeg", "data": "abc"}
            ]
        });
        let content = mcp_follow_up_content("captured", &result.to_string(), 1);
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "data:image/jpeg;base64,abc");
    }

    #[test]
    fn inline_run_metadata_preserves_budget_and_duplicate_guard() {
        let events = serde_json::to_vec(&json!([{
            "operation": "mcp.call_tool",
            "state": "committed",
            "op_counts": {"mcp.call_tool": 4},
            "latest_mcp_request_hash": "hash"
        }]))
        .unwrap();
        assert_eq!(op_count(&events, "mcp."), 4);
        assert_eq!(latest_mcp_request_hash(&events).as_deref(), Some("hash"));
    }

    #[test]
    fn harness_call_parses_and_rejects_foreign_ops() {
        let call = parse_harness_call(
            r#"{"harness_call":{"op":"harness.read","path":"skills/x/SKILL.md"}}"#,
        )
        .expect("harness call");
        assert_eq!(call["op"], "harness.read");
        assert_eq!(call["path"], "skills/x/SKILL.md");
        assert!(parse_harness_call(r#"{"harness_call":{"op":"mcp.call_tool"}}"#).is_none());
        assert!(parse_harness_call(r#"{"mcp_call":{"server":"s","tool":"t"}}"#).is_none());
    }

    #[test]
    fn spawn_agent_requires_task() {
        assert!(parse_spawn_agent(r#"{"spawn_agent":{"task":"sub"}}"#).is_some());
        assert!(parse_spawn_agent(r#"{"spawn_agent":{}}"#).is_none());
        assert!(parse_spawn_agent(r#"{"complete":{"output":"x"}}"#).is_none());
    }

    fn spawn_input(events: Value) -> LoopInput {
        LoopInput {
            run_id: "parent-run".to_owned(),
            events: serde_json::to_vec(&events).unwrap(),
            ..Default::default()
        }
    }

    fn spawn_envelope() -> Value {
        json!({
            "task": "parent task",
            "creative": true,
            "spawn_defaults": {
                "session_id": "sess",
                "agent_spec_id": "spec",
                "spec_version": "1",
                "spec_digest": "sha256:abc",
                "profile": "openrouter-wasm"
            }
        })
    }

    #[test]
    fn build_child_request_inherits_parent_envelope() {
        let input = spawn_input(json!([{"child_count": 0}]));
        let spawn = json!({"task": "child task"});
        let bytes =
            build_child_request(&spawn, &input, Some(&spawn_envelope())).expect("child request");
        let req = CreateTaskRun::decode(bytes.as_slice()).expect("proto");
        assert_eq!(req.session_id, "sess");
        assert_eq!(req.parent_run_id, "parent-run");
        assert_eq!(req.task_id, "");
        assert_eq!(req.requested_profile, "openrouter-wasm");
        let spec = req.agent_spec_ref.expect("spec");
        assert_eq!((spec.id.as_str(), spec.version.as_str()), ("spec", "1"));
        let payload: Value = serde_json::from_slice(&req.task_payload).unwrap();
        assert_eq!(payload["task"], "child task");
        assert_eq!(payload["creative"], true);
        assert_eq!(payload["spawn_defaults"]["session_id"], "sess");
    }

    #[test]
    fn build_child_request_guards() {
        let input = spawn_input(json!([{"child_count": 0}]));
        let spawn = json!({"task": "x"});
        assert_eq!(
            build_child_request(&spawn, &input, None).unwrap_err(),
            "spawn_not_configured"
        );
        let capped = spawn_input(json!([{"child_count": 8}]));
        assert_eq!(
            build_child_request(&spawn, &capped, Some(&spawn_envelope())).unwrap_err(),
            "child_limit"
        );
        // duplicate_spawn: a child entry already carries this payload's hash.
        let env = spawn_envelope();
        let ok = build_child_request(&spawn, &input, Some(&env)).expect("first");
        let payload = CreateTaskRun::decode(ok.as_slice()).unwrap().task_payload;
        let hash = format!("{:x}", Sha256::digest(&payload));
        let resumed = spawn_input(json!([
            {
                "operation": "kernel.child_settled",
                "run_id": "child-1",
                "state": "completed",
                "payload_hash": hash,
            },
            {"child_count": 1}
        ]));
        assert_eq!(
            build_child_request(&spawn, &resumed, Some(&env)).unwrap_err(),
            "duplicate_spawn"
        );
    }

    #[test]
    fn marker_reads_child_count_and_tool_hash() {
        let events = serde_json::to_vec(&json!([{
            "operation": "kernel.op_counts",
            "child_count": 3,
            "latest_tool_request_hash": "tool-hash",
            "latest_mcp_request_hash": "mcp-hash"
        }]))
        .unwrap();
        assert_eq!(child_count(&events), 3);
        assert_eq!(
            latest_tool_request_hash(&events).as_deref(),
            Some("tool-hash")
        );
        // Older kernels emit only the mcp field — it stands in for tools.
        let legacy = serde_json::to_vec(&json!([{"latest_mcp_request_hash": "mcp-hash"}])).unwrap();
        assert_eq!(
            latest_tool_request_hash(&legacy).as_deref(),
            Some("mcp-hash")
        );
    }
}
