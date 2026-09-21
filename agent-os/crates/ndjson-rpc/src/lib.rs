//! Minimal blocking JSON-RPC 2.0 client over newline-delimited JSON on a
//! child process's stdin/stdout — the transport shape shared by MCP
//! (stdio transport) and ACP.
//!
//! The peer may interleave notifications and server→client requests
//! between our request and its response. `request` drains those into a
//! callback so callers can stream progress (ACP `session/update`) or
//! answer requests (ACP `session/request_permission`).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// An incoming peer-originated message seen while waiting for a response.
pub enum Incoming<'a> {
    /// `{"method": ..., "params": ...}` — no reply expected. The
    /// callback's return value is discarded.
    Notification { method: &'a str, params: &'a Value },
    /// `{"id": ..., "method": ..., "params": ...}` — the callback's
    /// [`Reply`] becomes the response sent back to the peer.
    Request(&'a IncomingRequest),
}

/// A server→client request; `respond` sends back `result`.
pub struct IncomingRequest {
    /// JSON-RPC id (echoed verbatim).
    pub id: Value,
    /// Method name.
    pub method: String,
    /// Params object.
    pub params: Value,
}

/// How to handle an [`Incoming::Request`].
pub enum Reply {
    /// Send a success response with this result object.
    Result(Value),
    /// Send a JSON-RPC error response.
    Error { code: i64, message: String },
    /// Do not answer (peer's timeout will resolve it).
    Ignore,
}

impl IncomingRequest {
    fn respond_with(&self, reply: Reply) -> Option<Value> {
        let id = self.id.clone();
        match reply {
            Reply::Result(result) => Some(json!({
                "jsonrpc": "2.0", "id": id, "result": result,
            })),
            Reply::Error { code, message } => Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": code, "message": message},
            })),
            Reply::Ignore => None,
        }
    }
}

/// Error surfaced by [`Client`].
#[derive(Debug)]
pub enum RpcError {
    /// Transport-level failure (spawn/write/read).
    Io(std::io::Error),
    /// Peer closed its side (EOF) mid-request.
    Closed,
    /// Peer returned a JSON-RPC error.
    Remote { code: i64, message: String },
    /// Deadline elapsed waiting for the response.
    Timeout,
    /// Response/notification wasn't parseable JSON-RPC.
    Malformed(String),
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RpcError::Io(e) => write!(f, "io: {e}"),
            RpcError::Closed => write!(f, "peer closed"),
            RpcError::Remote { code, message } => write!(f, "rpc {code}: {message}"),
            RpcError::Timeout => write!(f, "timeout"),
            RpcError::Malformed(m) => write!(f, "malformed: {m}"),
        }
    }
}

impl std::error::Error for RpcError {}

impl From<std::io::Error> for RpcError {
    fn from(e: std::io::Error) -> Self {
        RpcError::Io(e)
    }
}

/// Options for [`Client::spawn`].
pub struct Spawn {
    /// Executable.
    pub command: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Extra env vars (the child inherits the parent env plus these).
    pub env: Vec<(String, String)>,
    /// Working directory.
    pub cwd: Option<String>,
}

/// Blocking NDJSON JSON-RPC client holding the spawned child.
pub struct Client {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl Client {
    /// Spawn `command` with stdio pipes and return a ready client.
    pub fn spawn(spec: Spawn) -> Result<Client, RpcError> {
        let mut cmd = Command::new(&spec.command);
        cmd.args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Agent stderr is ours to log — inherit so a noisy server
            // doesn't deadlock on a full pipe nobody drains.
            .stderr(Stdio::inherit());
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().ok_or(RpcError::Closed)?;
        let stdout = child.stdout.take().ok_or(RpcError::Closed)?;
        Ok(Client {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
    }

    /// Send a notification (no id, no response).
    pub fn notify(&mut self, method: &str, params: Value) -> Result<(), RpcError> {
        self.send(&json!({"jsonrpc": "2.0", "method": method, "params": params}))
    }

    /// Send a request and return its `result`.
    ///
    /// `on_incoming` sees every notification and server→client request
    /// the peer sends while we wait; returning a [`Reply`] for a request
    /// sends the response.
    pub fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
        on_incoming: &mut dyn FnMut(Incoming) -> Reply,
    ) -> Result<Value, RpcError> {
        let id = Value::from(self.next_id);
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        }))?;
        let until = Instant::now() + deadline;
        let mut line = String::new();
        loop {
            if Instant::now() > until {
                return Err(RpcError::Timeout);
            }
            line.clear();
            let n = self.stdout.read_line(&mut line)?;
            if n == 0 {
                return Err(RpcError::Closed);
            }
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let msg: Value =
                serde_json::from_str(trimmed).map_err(|e| RpcError::Malformed(e.to_string()))?;
            let has_id = msg.get("id").map(|v| !v.is_null()).unwrap_or(false);
            let has_method = msg.get("method").is_some();
            if has_method && has_id {
                // Server→client request.
                let req = IncomingRequest {
                    id: msg["id"].clone(),
                    method: msg["method"].as_str().unwrap_or_default().to_owned(),
                    params: msg.get("params").cloned().unwrap_or(Value::Null),
                };
                let reply = on_incoming(Incoming::Request(&req));
                if let Some(resp) = req.respond_with(reply) {
                    self.send(&resp)?;
                }
            } else if has_method {
                let m = msg["method"].as_str().unwrap_or_default().to_owned();
                let params = msg.get("params").cloned().unwrap_or(Value::Null);
                on_incoming(Incoming::Notification {
                    method: &m,
                    params: &params,
                });
            } else if has_id && msg["id"] == id {
                if let Some(e) = msg.get("error") {
                    return Err(RpcError::Remote {
                        code: e["code"].as_i64().unwrap_or(-32000),
                        message: e["message"].as_str().unwrap_or("remote error").to_owned(),
                    });
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            // Stale response for another id — ignore.
        }
    }

    /// Convenience: request and ignore peer traffic.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.request(method, params, Duration::from_secs(60), &mut |_| {
            Reply::Ignore
        })
    }

    fn send(&mut self, msg: &Value) -> Result<(), RpcError> {
        let body = serde_json::to_vec(msg).map_err(|e| RpcError::Malformed(e.to_string()))?;
        self.stdin.write_all(&body)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
