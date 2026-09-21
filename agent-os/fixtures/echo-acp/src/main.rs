//! Minimal ACP-compatible agent over stdio (NDJSON JSON-RPC) — the
//! reference peer for the `acp-loop` adapter's tests and demos.
//!
//! Implements `initialize`, `session/new`, and `session/prompt`: each
//! prompt echoes its text back in two `session/update`
//! `agent_message_chunk` notifications, then resolves the request.
//! `session/request_permission` is emitted when the prompt contains
//! "perm-required" so tests can exercise the permission path.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut session_id = String::new();
    let mut next_sid = 0u64;
    let mut line = String::new();
    loop {
        line.clear();
        match input.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        // Responses to our own outbound requests (permission replies)
        // carry no method — consumed inline elsewhere.
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            continue;
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match method {
            "initialize" => respond(
                &mut out,
                msg["id"].clone(),
                json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {"loadSession": false},
                    "authMethods": [],
                    "agentInfo": {"name": "echo-acp", "version": "0.1.0"},
                }),
            ),
            "session/new" => {
                next_sid += 1;
                session_id = format!("sess-{next_sid}");
                respond(
                    &mut out,
                    msg["id"].clone(),
                    json!({"sessionId": session_id}),
                );
            }
            "session/prompt" => {
                let sid = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or(&session_id)
                    .to_owned();
                let text = params
                    .get("prompt")
                    .and_then(Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|p| {
                                (p.get("type").and_then(Value::as_str) == Some("text"))
                                    .then(|| p.get("text").and_then(Value::as_str))
                                    .flatten()
                            })
                            .collect::<Vec<_>>()
                            .join("")
                    })
                    .unwrap_or_default();
                let mut chunks = vec![format!("acp-echo:{text}")];
                if text.contains("perm-required") {
                    chunks.push("permission-was-requested".to_owned());
                    send(
                        &mut out,
                        &json!({
                            "jsonrpc": "2.0",
                            "id": 900,
                            "method": "session/request_permission",
                            "params": {
                                "sessionId": sid,
                                "options": [
                                    {"optionId": "allow-1", "name": "Allow once", "kind": "allow_once"},
                                    {"optionId": "deny-1", "name": "Deny", "kind": "reject_once"},
                                ],
                            },
                        }),
                    );
                    // Read the client's response line.
                    let mut answer = String::new();
                    let _ = input.read_line(&mut answer);
                }
                for chunk in chunks {
                    send(
                        &mut out,
                        &json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": {
                                "sessionId": sid,
                                "update": {
                                    "sessionUpdate": "agent_message_chunk",
                                    "content": {"type": "text", "text": chunk},
                                },
                            },
                        }),
                    );
                }
                respond(
                    &mut out,
                    msg["id"].clone(),
                    json!({"stopReason": "end_turn"}),
                );
            }
            _ => {
                if let Some(id) = msg.get("id").cloned() {
                    respond_err(&mut out, id, -32601, "method not found");
                }
            }
        }
    }
    let _ = out.flush();
}

fn send(out: &mut impl Write, msg: &Value) {
    let mut body = serde_json::to_vec(msg).unwrap_or_default();
    body.push(b'\n');
    let _ = out.write_all(&body);
    let _ = out.flush();
}

fn respond(out: &mut impl Write, id: Value, result: Value) {
    send(out, &json!({"jsonrpc": "2.0", "id": id, "result": result}));
}

fn respond_err(out: &mut impl Write, id: Value, code: i64, message: &str) {
    send(
        out,
        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
    );
}
