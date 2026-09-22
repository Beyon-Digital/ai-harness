//! Minimal MCP server over stdio (NDJSON JSON-RPC) — used by tests and
//! as the reference MCP connection for `mcp.*` effects.
//!
//! Implements `initialize`, `notifications/initialized`, `ping`,
//! `tools/list`, `tools/call`, and `resources/read`. The single `echo`
//! tool returns `{text}` verbatim; `uppercase` uppercases it — enough to
//! prove tool dispatch and argument passing.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(msg) = serde_json::from_str::<Value>(trimmed) else {
            continue;
        };
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            continue;
        };
        // Notifications carry no id — nothing to answer.
        let Some(id) = id else { continue };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}, "resources": {}},
                "serverInfo": {"name": "echo-mcp", "version": "0.1.0"},
            }),
            "ping" => json!({}),
            "tools/list" => json!({
                "tools": [
                    {
                        "name": "echo",
                        "description": "Echo the input text verbatim.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"text": {"type": "string"}},
                            "required": ["text"],
                        },
                    },
                    {
                        "name": "uppercase",
                        "description": "Uppercase the input text.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {"text": {"type": "string"}},
                            "required": ["text"],
                        },
                    },
                ],
            }),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let text = params
                    .get("arguments")
                    .and_then(|a| a.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let echoed = match name {
                    "echo" => text.to_owned(),
                    "uppercase" => text.to_uppercase(),
                    _ => {
                        respond(&mut out, id, None, Some((-32602, "unknown tool")));
                        continue;
                    }
                };
                json!({
                    "content": [{"type": "text", "text": echoed}],
                    "isError": false,
                })
            }
            "resources/read" => json!({
                "contents": [{
                    "uri": params.get("uri").cloned().unwrap_or(Value::Null),
                    "mimeType": "text/plain",
                    "text": "hello from echo-mcp",
                }],
            }),
            _ => {
                respond(&mut out, id, None, Some((-32601, "method not found")));
                continue;
            }
        };
        respond(&mut out, id, Some(result), None);
    }
}

fn respond(out: &mut impl Write, id: Value, result: Option<Value>, error: Option<(i64, &str)>) {
    let msg = match (result, error) {
        (Some(r), _) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        (_, Some((code, message))) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": code, "message": message},
        }),
        _ => return,
    };
    let mut body = serde_json::to_vec(&msg).unwrap_or_default();
    body.push(b'\n');
    let _ = out.write_all(&body);
    let _ = out.flush();
}
