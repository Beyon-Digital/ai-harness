//! MCP (Model Context Protocol) client for `mcp.*` effect operations.
//!
//! The `effect.execute` port carries no operation name, so the payload
//! itself discriminates: `{"op": "mcp.list_tools" | "mcp.call_tool" |
//! "mcp.read_resource", "server": "<name>", ...}`. Anything else falls
//! through to `model.chat`.
//!
//! `MCP_SERVERS` is a JSON map of server name → spec:
//!   `{"echo": {"command": "echo-mcp", "args": [], "env": {"K": "V"}},
//!     "remote": {"url": "https://mcp.example.com/mcp", "headers":
//!       {"Authorization": "Bearer ..."}}}`
//! Stdio servers are spawned per call (stateless, durable-idempotent at
//! the effect layer). HTTP servers use the streamable-HTTP transport:
//! a POST whose SSE `data:` frames are scanned for our response.

use std::time::Duration;

use ndjson_rpc::{Client, Incoming, Reply, Spawn};
use serde_json::{Value, json};

/// True when the payload asks for an `mcp.*` operation.
pub fn is_mcp_payload(payload: &Value) -> bool {
    payload
        .get("op")
        .and_then(Value::as_str)
        .map(|op| op.starts_with("mcp."))
        .unwrap_or(false)
}

/// Execute one `mcp.*` call, returning the text to store as result.
pub fn call(payload: &Value, timeout: Duration) -> Result<String, String> {
    let op = payload
        .get("op")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let server = payload
        .get("server")
        .and_then(Value::as_str)
        .ok_or("missing `server`")?;
    let spec = server_spec(server)?;
    let (method, params) = match op {
        "mcp.list_tools" => ("tools/list", json!({})),
        "mcp.call_tool" => (
            "tools/call",
            json!({
                "name": payload.get("tool").and_then(Value::as_str).ok_or("missing `tool`")?,
                "arguments": payload.get("arguments").cloned().unwrap_or(json!({})),
            }),
        ),
        "mcp.read_resource" => (
            "resources/read",
            json!({
                "uri": payload.get("uri").and_then(Value::as_str).ok_or("missing `uri`")?,
            }),
        ),
        other => return Err(format!("unknown mcp op {other}")),
    };
    let result = match &spec {
        ServerSpec::Stdio {
            command,
            args,
            env,
            cwd,
        } => stdio_call(command, args, env, cwd.as_deref(), method, params, timeout)?,
        ServerSpec::Http { url, headers } => http_call(url, headers, method, params, timeout)?,
    };
    Ok(flatten_result(&result))
}

/// `tools/call` returns `{content: [{type:"text",text}...]}` — join the
/// text parts; anything else is returned as compact JSON.
fn flatten_result(result: &Value) -> String {
    if let Some(content) = result.get("content").and_then(Value::as_array) {
        let texts: Vec<&str> = content
            .iter()
            .filter_map(|c| c.get("text").and_then(Value::as_str))
            .collect();
        if !texts.is_empty() {
            return texts.join("\n");
        }
    }
    serde_json::to_string(result).unwrap_or_default()
}

enum ServerSpec {
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
        cwd: Option<String>,
    },
    Http {
        url: String,
        headers: Vec<(String, String)>,
    },
}

fn server_spec(name: &str) -> Result<ServerSpec, String> {
    let raw =
        std::env::var("MCP_SERVERS").map_err(|_| "MCP_SERVERS not configured on the daemon")?;
    let servers: Value =
        serde_json::from_str(&raw).map_err(|e| format!("MCP_SERVERS is not JSON: {e}"))?;
    let spec = servers
        .get(name)
        .ok_or_else(|| format!("mcp server `{name}` not in MCP_SERVERS"))?;
    if let Some(url) = spec.get("url").and_then(Value::as_str) {
        let headers = spec
            .get("headers")
            .and_then(Value::as_object)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        return Ok(ServerSpec::Http {
            url: url.to_owned(),
            headers,
        });
    }
    let command = spec
        .get("command")
        .and_then(Value::as_str)
        .ok_or("server spec needs `command` or `url`")?
        .to_owned();
    let args = spec
        .get("args")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let env = spec
        .get("env")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let cwd = spec.get("cwd").and_then(Value::as_str).map(str::to_owned);
    Ok(ServerSpec::Stdio {
        command,
        args,
        env,
        cwd,
    })
}

fn stdio_call(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: Option<&str>,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let mut client = Client::spawn(Spawn {
        command: command.to_owned(),
        args: args.to_vec(),
        env: env.to_vec(),
        cwd: cwd.map(str::to_owned),
    })
    .map_err(|e| format!("mcp_spawn: {e}"))?;
    let mut ignore = |_i: Incoming| Reply::Ignore;
    client
        .request(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "agentos-mcp", "version": "0.1.0"},
            }),
            timeout,
            &mut ignore,
        )
        .map_err(|e| format!("mcp_initialize: {e}"))?;
    let _ = client.notify("notifications/initialized", json!({}));
    client
        .request(method, params, timeout, &mut ignore)
        .map_err(|e| format!("{method}: {e}"))
}

/// Streamable-HTTP MCP: POST the JSON-RPC message; the response is
/// either plain JSON or an SSE stream whose `data:` lines carry it.
fn http_call(
    url: &str,
    headers: &[(String, String)],
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .into();
    let mut req = agent
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    for (k, v) in headers {
        req = req.header(k, v);
    }
    // `initialize` first, then the op — streamable-HTTP servers expect it.
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "agentos-mcp", "version": "0.1.0"},
        },
    });
    let mut resp = req.send_json(&init).map_err(|e| format!("mcp_http: {e}"))?;
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("mcp_http_read: {e}"))?;
    if let Some(v) = extract_response(&body, &Value::from(1)) {
        v.map_err(|e| format!("mcp_initialize: {e}"))?;
    }
    let call = json!({
        "jsonrpc": "2.0", "id": 2, "method": method, "params": params,
    });
    let mut resp = agent
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");
    for (k, v) in headers {
        resp = resp.header(k, v);
    }
    let mut resp = resp
        .send_json(&call)
        .map_err(|e| format!("mcp_http: {e}"))?;
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("mcp_http_read: {e}"))?;
    match extract_response(&body, &Value::from(2)) {
        Some(Ok(v)) => Ok(v),
        Some(Err(e)) => Err(format!("{method}: {e}")),
        None => Err("mcp_http: no response frame".to_owned()),
    }
}

/// Find the JSON-RPC message for `id` in a body that is either plain
/// JSON or SSE `data:` lines.
fn extract_response(body: &str, id: &Value) -> Option<Result<Value, String>> {
    let try_parse = |s: &str| -> Option<Result<Value, String>> {
        let v: Value = serde_json::from_str(s).ok()?;
        if v.get("id") == Some(id) {
            if let Some(e) = v.get("error") {
                return Some(Err(e["message"]
                    .as_str()
                    .unwrap_or("remote error")
                    .to_owned()));
            }
            return Some(Ok(v.get("result").cloned().unwrap_or(Value::Null)));
        }
        None
    };
    for line in body.lines() {
        let data = line.strip_prefix("data:").map(str::trim).unwrap_or(line);
        if let Some(r) = try_parse(data) {
            return Some(r);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flatten_joins_text_content() {
        let v = json!({"content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]});
        assert_eq!(flatten_result(&v), "a\nb");
    }

    #[test]
    fn flatten_falls_back_to_json() {
        let v = json!({"tools": [{"name": "echo"}]});
        assert_eq!(flatten_result(&v), r#"{"tools":[{"name":"echo"}]}"#);
    }

    #[test]
    fn extract_plain_json_response() {
        let body = r#"{"jsonrpc":"2.0","id":2,"result":{"ok":true}}"#;
        assert_eq!(
            extract_response(body, &Value::from(2)).unwrap().unwrap(),
            json!({"ok": true})
        );
    }

    #[test]
    fn extract_sse_framed_response() {
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{\"v\":1}}\n\n";
        assert_eq!(
            extract_response(body, &Value::from(2)).unwrap().unwrap(),
            json!({"v": 1})
        );
    }

    #[test]
    fn extract_remote_error() {
        let body = r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"no such method"}}"#;
        assert_eq!(
            extract_response(body, &Value::from(2)).unwrap(),
            Err("no such method".to_owned())
        );
    }

    #[test]
    fn mcp_payload_detection() {
        assert!(is_mcp_payload(&json!({"op": "mcp.call_tool"})));
        assert!(!is_mcp_payload(&json!({"model": "x"})));
        assert!(!is_mcp_payload(&json!({"op": "model.chat"})));
    }
}
