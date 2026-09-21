//! Fixture AgentLoop process (`LOOP-001`).
//!
//! Speaks the framed adapter protocol on fd 0, then serves
//! `agent_loop.next` PortCallRequests: the payload is a `LoopInput`, the
//! decision is read from a deterministic script (`FIXTURE_LOOP_SCRIPT`,
//! a JSON array indexed by `step_sequence`), and the response echoes the
//! exact run revision/epoch/step/cursor/turn with a generated
//! `decision_id`.
//!
//! Fault flags (env):
//! - `FIXTURE_LOOP_DELAY_MS=<ms>` — sleep before each decision;
//! - `FIXTURE_LOOP_STALE=1` — respond with `loop_epoch - 1`;
//! - `FIXTURE_LOOP_CRASH=1` — die after receiving the request, before
//!   responding (restart/crash tests).
//!
//! Script vocabulary (JSON objects, exactly one key each):
//! `{"complete":{"output_ref":"..."}}`,
//! `{"fail":{"reason_code":"..."}}`,
//! `{"wait":{"reason":"...","timer_id":"..."}}`,
//! `{"spawn_agent":{"child_request":"<base64>"}}`,
//! `{"invoke_effect":{"operation":"...","payload":"<base64>",
//!   "effect_claim":"<base64>"}}`,
//! `{"request_approval":{"approval_draft":"<base64>"}}`.

use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use adapter_protocol::framing::{read_frame, write_frame};
use domain::generated::contract::{
    AdapterFrame, AdapterHello, AdapterPong, Complete, Fail, InvokeEffect, LoopDecision, LoopInput,
    PortCallResponse, RequestApproval, SpawnAgent, Wait, adapter_frame::Body, loop_decision,
};
use prost::Message;

const PORT_ID: &str = "agent_loop";
const PROTOCOL_VERSION: u32 = 1;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("agent-loop fixture: {e}");
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
                adapter_id: env("FIXTURE_LOOP_ADAPTER_ID").unwrap_or_default(),
                adapter_version: env("FIXTURE_LOOP_ADAPTER_VERSION").unwrap_or_default(),
                bundle_digest: bootstrap.expected_bundle_digest,
                protocol_version: PROTOCOL_VERSION,
                implemented_ports: vec![PORT_ID.to_owned()],
                capability_document: Vec::new(),
            })),
        },
    )
    .map_err(err)?;

    let script: Vec<serde_json::Value> =
        serde_json::from_str(&env("FIXTURE_LOOP_SCRIPT").unwrap_or_else(|_| "[]".to_owned()))
            .unwrap_or_default();
    let delay_ms = env("FIXTURE_LOOP_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let stale = env("FIXTURE_LOOP_STALE").as_deref() == Ok("1");
    let crash = env("FIXTURE_LOOP_CRASH").as_deref() == Ok("1");

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
        if crash {
            std::process::exit(2);
        }
        let response = decide(&request, &script, stale);
        if delay_ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
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

fn decide(
    request: &domain::generated::contract::PortCallRequest,
    script: &[serde_json::Value],
    stale: bool,
) -> PortCallResponse {
    let Ok(input) = LoopInput::decode(request.payload.as_slice()) else {
        return PortCallResponse {
            call_id: request.call_id.clone(),
            payload: Vec::new(),
            error_code: "invalid_argument".to_owned(),
        };
    };
    let idx = input.step_sequence.saturating_sub(1) as usize;
    let step = script
        .get(idx)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({"fail": {"reason_code": "script_exhausted"}}));
    let decision = parse_decision(&step);
    let loop_epoch = if stale {
        input.loop_epoch.saturating_sub(1)
    } else {
        input.loop_epoch
    };
    let out = LoopDecision {
        run_id: input.run_id,
        run_revision: input.run_revision,
        loop_epoch,
        step_sequence: input.step_sequence,
        input_event_cursor: input.input_event_cursor,
        turn_id: input.turn_id,
        decision_id: format!("dec-{:04}", input.step_sequence),
        decision: Some(decision),
    };
    PortCallResponse {
        call_id: request.call_id.clone(),
        payload: out.encode_to_vec(),
        error_code: String::new(),
    }
}

fn b64(v: &serde_json::Value, key: &str) -> Vec<u8> {
    v.get(key)
        .and_then(|s| s.as_str())
        .map(|s| base64_decode(s.as_bytes()))
        .unwrap_or_default()
}

fn base64_decode(input: &[u8]) -> Vec<u8> {
    // Minimal base64 decoder for fixture scripts (alphabet A-Z a-z 0-9 +/).
    fn val(b: u8) -> Option<u8> {
        match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut acc = 0u32;
    let mut bits = 0;
    for &b in input {
        if b == b'=' {
            break;
        }
        let Some(v) = val(b) else { continue };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn parse_decision(step: &serde_json::Value) -> loop_decision::Decision {
    if let Some(c) = step.get("complete") {
        return loop_decision::Decision::Complete(Complete {
            output_ref: c
                .get("output_ref")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_owned(),
        });
    }
    if let Some(f) = step.get("fail") {
        return loop_decision::Decision::Fail(Fail {
            reason_code: f
                .get("reason_code")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_owned(),
        });
    }
    if let Some(w) = step.get("wait") {
        return loop_decision::Decision::Wait(Wait {
            reason: w
                .get("reason")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_owned(),
            timer_id: w
                .get("timer_id")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_owned(),
        });
    }
    if let Some(s) = step.get("spawn_agent") {
        return loop_decision::Decision::SpawnAgent(SpawnAgent {
            child_request: b64(s, "child_request"),
        });
    }
    if let Some(e) = step.get("invoke_effect") {
        return loop_decision::Decision::InvokeEffect(InvokeEffect {
            operation: e
                .get("operation")
                .and_then(|s| s.as_str())
                .unwrap_or_default()
                .to_owned(),
            payload: b64(e, "payload"),
            effect_claim: b64(e, "effect_claim"),
        });
    }
    if let Some(a) = step.get("request_approval") {
        return loop_decision::Decision::RequestApproval(RequestApproval {
            approval_draft: b64(a, "approval_draft"),
        });
    }
    loop_decision::Decision::Fail(Fail {
        reason_code: "unrecognized_script_step".to_owned(),
    })
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
