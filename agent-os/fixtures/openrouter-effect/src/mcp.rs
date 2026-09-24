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
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        return Err(tool_error(&result));
    }
    Ok(encode_result(&result, payload))
}

fn tool_error(result: &Value) -> String {
    let message = result
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    if message.is_empty() {
        "mcp_tool_error".to_owned()
    } else {
        format!("mcp_tool_error: {message}")
    }
}

/// Preserve MCP content for the UI and include the completed request so
/// the model can continue a multi-tool task without repeating the last call.
fn encode_result(result: &Value, request: &Value) -> String {
    let mut result = result.clone();
    if !result.is_object() {
        result = json!({"content": [{"type": "text", "text": result.to_string()}]});
    }
    if let Some(object) = result.as_object_mut() {
        let mut encoded_request = json!({
            "op": request.get("op").cloned().unwrap_or(Value::Null),
            "server": request.get("server").cloned().unwrap_or(Value::Null),
            "tool": request.get("tool").cloned().unwrap_or(Value::Null),
            "arguments": request.get("arguments").cloned().unwrap_or(json!({})),
        });
        if let Some(remaining_calls) = request.get("remaining_calls") {
            encoded_request["remaining_calls"] = remaining_calls.clone();
        }
        if let Some(final_output) = request.get("final_output") {
            encoded_request["final_output"] = final_output.clone();
        }
        object.insert("request".to_owned(), encoded_request);
    }
    serde_json::to_string(&result).unwrap_or_default()
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
        mcp_http_url_allowed(url)?;
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

/// HTTP MCP destinations are SSRF-sensitive: configured headers (e.g.
/// Authorization) get forwarded to the URL. Allow `https:` to any host
/// and `http:` only to loopback; `MCP_ALLOW_ANY_URL=1` opts out for
/// trusted internal networks.
fn mcp_http_url_allowed(url: &str) -> Result<(), String> {
    if std::env::var("MCP_ALLOW_ANY_URL").ok().as_deref() == Some("1") {
        return Ok(());
    }
    let rest = url
        .strip_prefix("https://")
        .map(|r| ("https", r))
        .or_else(|| url.strip_prefix("http://").map(|r| ("http", r)))
        .ok_or("mcp server url must be http(s)")?;
    if rest.0 == "https" {
        return Ok(());
    }
    let host = rest
        .1
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default()
        .split(':')
        .next()
        .unwrap_or_default()
        .trim_matches(['[', ']']);
    let loopback = host == "localhost"
        || host == "::1"
        || host
            .parse::<std::net::Ipv4Addr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false);
    if loopback {
        Ok(())
    } else {
        Err(format!(
            "mcp server url {url} refused: plain http is loopback-only \
             (set MCP_ALLOW_ANY_URL=1 to override)"
        ))
    }
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
    // Streamable-HTTP servers may mint a session id on initialize;
    // propagate it on every later request in this call.
    let session = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let body = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("mcp_http_read: {e}"))?;
    if let Some(v) = extract_response(&body, &Value::from(1)) {
        v.map_err(|e| format!("mcp_initialize: {e}"))?;
    }
    let post = |payload: &Value| -> Result<String, String> {
        let mut r = agent
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");
        for (k, v) in headers {
            r = r.header(k, v);
        }
        if let Some(s) = &session {
            r = r.header("Mcp-Session-Id", s);
        }
        r.send_json(payload)
            .map_err(|e| format!("mcp_http: {e}"))?
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("mcp_http_read: {e}"))
    };
    let _ = post(&json!({
        "jsonrpc": "2.0", "method": "notifications/initialized", "params": {},
    }));
    let call = json!({
        "jsonrpc": "2.0", "id": 2, "method": method, "params": params,
    });
    let body = post(&call)?;
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
    fn encode_preserves_text_content_and_request() {
        let v = json!({"content": [{"type": "text", "text": "a"}, {"type": "text", "text": "b"}]});
        let request = json!({
            "op": "mcp.call_tool",
            "server": "computer",
            "tool": "wait",
            "arguments": {"milliseconds": 50}
        });
        let encoded: Value =
            serde_json::from_str(&encode_result(&v, &request)).expect("encoded result");
        assert_eq!(encoded["content"], v["content"]);
        assert_eq!(encoded["request"], request);
    }

    #[test]
    fn encode_preserves_non_content_results() {
        let v = json!({"tools": [{"name": "echo"}]});
        let encoded: Value =
            serde_json::from_str(&encode_result(&v, &json!({}))).expect("encoded result");
        assert_eq!(encoded["tools"], v["tools"]);
        assert!(encoded["request"].is_object());
    }

    #[test]
    fn encode_preserves_mixed_content() {
        let value = json!({
            "content": [
                {"type": "text", "text": "captured"},
                {"type": "image", "mimeType": "image/jpeg", "data": "abc"}
            ]
        });
        let encoded: Value =
            serde_json::from_str(&encode_result(&value, &json!({}))).expect("encoded result");
        assert_eq!(encoded["content"], value["content"]);
    }

    #[test]
    fn tool_reported_errors_become_effect_errors() {
        let error = tool_error(&json!({
            "isError": true,
            "content": [{"type": "text", "text": "window unavailable"}]
        }));
        assert_eq!(error, "mcp_tool_error: window unavailable");
        assert_eq!(tool_error(&json!({"isError": true})), "mcp_tool_error");
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

    /// Minimal streamable-HTTP MCP server: initialize mints a
    /// `Mcp-Session-Id`, the notification gets a 202, and `tools/call`
    /// answers SSE-framed — asserting the header is propagated.
    #[test]
    fn http_call_against_mock_server() {
        use std::io::{BufRead, BufReader, Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let seen2 = seen.clone();
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming().take(3) {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let mut headers = Vec::new();
                let mut len = 0usize;
                while reader.read_line(&mut line).unwrap() > 0 {
                    let t = line.trim_end().to_owned();
                    line.clear();
                    if t.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = t.split_once(':')
                        && k.trim().eq_ignore_ascii_case("content-length")
                    {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    headers.push(t.clone());
                }
                seen2.lock().unwrap().extend(headers);
                let mut buf = vec![0u8; len];
                reader.read_exact(&mut buf).unwrap();
                let req: Value = serde_json::from_slice(&buf).unwrap();
                let (status, extra, payload) = match req.get("method").and_then(Value::as_str) {
                    Some("initialize") => (
                        "200 OK",
                        "Mcp-Session-Id: sess-1\r\nContent-Type: application/json\r\n",
                        json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"mock","version":"0"}}}).to_string(),
                    ),
                    Some("notifications/initialized") => {
                        ("202 Accepted", "", String::new())
                    }
                    Some("tools/call") => (
                        "200 OK",
                        "Content-Type: text/event-stream\r\n",
                        format!(
                            "data: {}\n\n",
                            json!({"jsonrpc":"2.0","id":2,"result":{"content":[{"type":"text","text":"http-mcp-ok"}]}})
                        ),
                    ),
                    _ => ("400 Bad Request", "", String::new()),
                };
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\n\r\n{payload}",
                    payload.len()
                )
                .unwrap();
            }
        });
        let out = http_call(
            &format!("http://127.0.0.1:{port}/mcp"),
            &[],
            "tools/call",
            json!({"name": "echo", "arguments": {}}),
            Duration::from_secs(10),
        )
        .expect("http_call");
        handle.join().unwrap();
        let encoded: Value = serde_json::from_str(&encode_result(
            &out,
            &json!({
                "op": "mcp.call_tool",
                "server": "http",
                "tool": "echo",
                "arguments": {}
            }),
        ))
        .expect("encoded result");
        assert_eq!(encoded["content"][0]["text"], "http-mcp-ok");
        assert!(
            seen.lock()
                .unwrap()
                .iter()
                .any(|h| h.eq_ignore_ascii_case("mcp-session-id: sess-1")),
            "session header must propagate to later requests"
        );
    }

    #[test]
    fn http_url_policy() {
        assert!(mcp_http_url_allowed("https://mcp.example.com/x").is_ok());
        assert!(mcp_http_url_allowed("http://127.0.0.1:8080/mcp").is_ok());
        assert!(mcp_http_url_allowed("http://localhost/mcp").is_ok());
        assert!(mcp_http_url_allowed("http://169.254.169.254/meta").is_err());
        assert!(mcp_http_url_allowed("file:///etc/passwd").is_err());
    }
}
