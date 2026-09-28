//! `harness.*` effect operations — the creative-mode tool surface.
//!
//! Creative mode lets an agent inspect and extend the harness itself:
//! read/write files under a harness root, scaffold plugins (skills,
//! workflows, adapters, loops, MCP tool servers), validate manifests,
//! and register extensions so later runs can use them.
//!
//! Every path is confined to `AGENTOS_HARNESS_ROOT` (default
//! `~/.agentos/harness`): absolute paths, `..` segments, and symlink
//! escapes are rejected. Writes cap at 1 MiB and auto-create parents.
//!
//! Ops (payload `op` field):
//!   harness.list      {path?, depth?}                      — tree listing
//!   harness.read      {path, max_bytes?}                   — file contents
//!   harness.write     {path, content, create_only?}        — write a file
//!   harness.scaffold  {kind, name, description?}           — plugin skeleton
//!   harness.validate  {path}                               — manifest checks
//!   harness.register  {kind, name, command?, args?, env?,
//!                      cwd?, path?, description?}          — install extension
//!   harness.registry  {}                                   — installed entries
//!   harness.catalog   {}                                   — op documentation
//!
//! Registering kind `mcp_server` also appends to
//! `<root>/mcp-servers.json`, which the MCP client merges into its live
//! server map — a new tool becomes callable by later runs without a
//! daemon restart.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const MAX_READ_BYTES: usize = 256 * 1024;
const DEFAULT_READ_BYTES: usize = 64 * 1024;
const MAX_WRITE_BYTES: usize = 1024 * 1024;
const MAX_LIST_ENTRIES: usize = 2_000;
const MAX_LIST_DEPTH: u64 = 6;
const SKIP_DIRS: &[&str] = &[".git", "target", "node_modules", ".hg", ".svn"];

/// True when the payload asks for a `harness.*` operation.
pub fn is_harness_payload(payload: &Value) -> bool {
    payload
        .get("op")
        .and_then(Value::as_str)
        .map(|op| op.starts_with("harness."))
        .unwrap_or(false)
}

/// Execute one `harness.*` op, returning the JSON result text to store.
pub fn call(payload: &Value) -> Result<String, String> {
    let op = payload
        .get("op")
        .and_then(Value::as_str)
        .ok_or("missing `op`")?;
    match op {
        "harness.catalog" => Ok(catalog().to_string()),
        "harness.registry" => registry(),
        "harness.list" => list(payload),
        "harness.read" => read(payload),
        "harness.write" => write(payload),
        "harness.scaffold" => scaffold(payload),
        "harness.validate" => validate(payload),
        "harness.register" => register(payload),
        other => Err(format!("unknown harness op {other}")),
    }
}

/// The harness root — where skills, workflows, extensions and the
/// extension registry live. Created lazily on first write.
pub fn root() -> Result<PathBuf, String> {
    if let Ok(dir) = std::env::var("AGENTOS_HARNESS_ROOT") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return Ok(PathBuf::from(dir));
        }
    }
    let home = std::env::var("HOME")
        .map_err(|_| "AGENTOS_HARNESS_ROOT unset and HOME unknown".to_owned())?;
    Ok(PathBuf::from(home).join(".agentos").join("harness"))
}

/// File the MCP client merges into its `MCP_SERVERS` map — registering
/// an `mcp_server` writes here so the tool is live on the next call.
/// `MCP_SERVERS_FILE` wins when set (mcp.rs resolves the same way, so
/// writer and reader always agree).
pub fn mcp_servers_file() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("MCP_SERVERS_FILE") {
        let path = path.trim();
        if !path.is_empty() {
            return Some(PathBuf::from(path));
        }
    }
    root().ok().map(|r| r.join("mcp-servers.json"))
}

/// Write `bytes` to `tmp` only when it does not already exist — a
/// pre-planted symlink at a fixed tmp name must fail, not redirect the
/// write outside the root.
fn write_tmp(tmp: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(tmp)
        .and_then(|mut file| std::io::Write::write_all(&mut file, bytes))
        .map_err(|e| format!("write {}: {e}", tmp.display()))
}

fn registry_file(root: &Path) -> PathBuf {
    root.join("registry.json")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Resolve `rel` strictly under `root`: no absolute paths, no `..`, and
/// existing ancestors must canonicalize inside the root (symlink guard).
fn resolve(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err("empty path".to_owned());
    }
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err("path must be relative to the harness root".to_owned());
    }
    for component in rel_path.components() {
        match component {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("path escapes the harness root".to_owned());
            }
            _ => {}
        }
    }
    let candidate = root.join(rel_path);
    // Walk to the deepest existing ancestor; if any existing portion of
    // the path canonicalizes outside the root, a symlink is escaping.
    // `symlink_metadata` counts a *dangling* symlink as present —
    // `exists()` would miss it, and a later write would follow it out.
    let canon_root = root
        .canonicalize()
        .map_err(|e| format!("harness root unavailable: {e}"))?;
    let mut probe = candidate.clone();
    loop {
        if probe.symlink_metadata().is_ok() {
            let canon = probe
                .canonicalize()
                .map_err(|_| format!("cannot resolve {rel} (dangling or escaped link)"))?;
            if !canon.starts_with(&canon_root) {
                return Err("path escapes the harness root".to_owned());
            }
            break;
        }
        match probe.parent() {
            Some(parent) => probe = parent.to_path_buf(),
            None => break,
        }
    }
    Ok(candidate)
}

fn ensure_root() -> Result<PathBuf, String> {
    let root = root()?;
    std::fs::create_dir_all(&root).map_err(|e| format!("cannot create harness root: {e}"))?;
    Ok(root)
}

fn catalog() -> Value {
    json!({
        "root": root().map(|p| p.display().to_string()).unwrap_or_default(),
        "ops": [
            {"op": "harness.list", "args": {"path": "optional dir (default \".\")", "depth": "1-6 (default 3)"}},
            {"op": "harness.read", "args": {"path": "required", "max_bytes": "optional"}},
            {"op": "harness.write", "args": {"path": "required", "content": "required", "create_only": "optional bool"}},
            {"op": "harness.scaffold", "args": {"kind": "skill|workflow|adapter|loop|mcp_server", "name": "required", "description": "optional"}},
            {"op": "harness.validate", "args": {"path": "required"}},
            {"op": "harness.register", "args": {"kind": "skill|workflow|adapter|loop|mcp_server", "name": "required", "command/args/env/cwd": "mcp_server", "path": "optional override"}},
            {"op": "harness.registry", "args": {}},
        ],
        "layout": {
            "skills/<name>/SKILL.md": "agent skills — listed in creative-mode prompts",
            "workflows/<name>.json": "named step sequences an agent can follow",
            "extensions/<name>/": "adapter/loop/mcp_server plugin source",
            "mcp-servers.json": "MCP servers merged into the live tool map",
            "registry.json": "installed extension entries",
        },
    })
}

fn list(payload: &Value) -> Result<String, String> {
    let root = root()?;
    if !root.exists() {
        return Ok(json!({"root": root.display().to_string(), "entries": []}).to_string());
    }
    let rel = payload.get("path").and_then(Value::as_str).unwrap_or(".");
    let depth = payload
        .get("depth")
        .and_then(Value::as_u64)
        .map(|d| d.clamp(1, MAX_LIST_DEPTH) as u32)
        .unwrap_or(3);
    let dir = resolve(&root, rel)?;
    if !dir.is_dir() {
        return Err(format!("{rel} is not a directory"));
    }
    let mut entries = Vec::new();
    walk(&root, &dir, depth, &mut entries)?;
    let truncated = entries.len() >= MAX_LIST_ENTRIES;
    Ok(json!({
        "root": root.display().to_string(),
        "path": rel,
        "entries": entries,
        "truncated": truncated,
    })
    .to_string())
}

fn walk(root: &Path, dir: &Path, depth: u32, out: &mut Vec<Value>) -> Result<(), String> {
    if out.len() >= MAX_LIST_ENTRIES {
        return Ok(());
    }
    let read = std::fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    let mut children: Vec<_> = read.filter_map(|e| e.ok()).collect();
    children.sort_by_key(|e| e.file_name());
    for child in children {
        if out.len() >= MAX_LIST_ENTRIES {
            break;
        }
        let path = child.path();
        let name = child.file_name().to_string_lossy().to_string();
        // `file_type` does not follow symlinks: a symlinked dir reports
        // as a link and is never descended into — listing a tree cannot
        // leak entries from outside the root.
        let file_type = child.file_type();
        let is_dir = file_type.as_ref().map(|t| t.is_dir()).unwrap_or(false);
        let is_link = file_type.map(|t| t.is_symlink()).unwrap_or(false);
        if name.starts_with('.') || (is_dir && SKIP_DIRS.contains(&name.as_str())) {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| name.clone());
        out.push(json!({
            "path": rel,
            "kind": if is_link { "link" } else if is_dir { "dir" } else { "file" },
            "bytes": child.metadata().map(|m| m.len()).unwrap_or(0),
        }));
        if is_dir && depth > 0 {
            walk(root, &path, depth - 1, out)?;
        }
    }
    Ok(())
}

fn read(payload: &Value) -> Result<String, String> {
    let root = root()?;
    let rel = payload
        .get("path")
        .and_then(Value::as_str)
        .ok_or("missing `path`")?;
    let max_bytes = payload
        .get("max_bytes")
        .and_then(Value::as_u64)
        .map(|n| n.clamp(1, MAX_READ_BYTES as u64) as usize)
        .unwrap_or(DEFAULT_READ_BYTES);
    let path = resolve(&root, rel)?;
    // Read at most max_bytes+1 — never pull a huge file into memory
    // just to truncate the response.
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let file = std::fs::File::open(&path).map_err(|e| format!("read {rel}: {e}"))?;
    let mut bytes = Vec::new();
    file.take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read {rel}: {e}"))?;
    let truncated = bytes.len() > max_bytes;
    let mut end = max_bytes.min(bytes.len());
    // Walk back past UTF-8 continuation bytes (0b10xxxxxx) so `end`
    // never splits a multi-byte scalar.
    while end > 0 && end < bytes.len() && bytes[end] & 0xC0 == 0x80 {
        end -= 1;
    }
    let content = String::from_utf8_lossy(&bytes[..end]).to_string();
    Ok(json!({
        "path": rel,
        "content": content,
        "bytes": size,
        "truncated": truncated,
    })
    .to_string())
}

fn write(payload: &Value) -> Result<String, String> {
    let root = ensure_root()?;
    let rel = payload
        .get("path")
        .and_then(Value::as_str)
        .ok_or("missing `path`")?;
    let content = payload
        .get("content")
        .and_then(Value::as_str)
        .ok_or("missing `content`")?;
    if content.len() > MAX_WRITE_BYTES {
        return Err(format!("content exceeds {MAX_WRITE_BYTES} bytes"));
    }
    let create_only = payload
        .get("create_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let path = resolve(&root, rel)?;
    // A dangling symlink at the target counts as existing — otherwise
    // `create_only` misses it and the rename silently replaces the link.
    let existed = path.symlink_metadata().is_ok();
    if create_only && existed {
        return Err(format!("{rel} already exists (create_only)"));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create parents: {e}"))?;
    }
    // Write-then-rename so a crash mid-write cannot leave a torn file;
    // create_new on the fixed tmp name refuses to follow a planted link.
    let tmp = path.with_extension("tmp-write");
    write_tmp(&tmp, content.as_bytes())?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("rename {rel}: {e}"))?;
    Ok(json!({"path": rel, "bytes": content.len(), "existed": existed}).to_string())
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "name must be 1-64 chars of [a-zA-Z0-9_-] (it becomes a directory name)".to_owned(),
        );
    }
    Ok(())
}

fn scaffold(payload: &Value) -> Result<String, String> {
    let root = ensure_root()?;
    let kind = payload
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("missing `kind` (skill|workflow|adapter|loop|mcp_server)")?;
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .ok_or("missing `name`")?;
    validate_name(name)?;
    let description = payload
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let files: BTreeMap<String, String> = match kind {
        "skill" => skill_files(name, description),
        "workflow" => workflow_files(name, description),
        "adapter" => adapter_files(name, description, "effect.execute"),
        "loop" => adapter_files(name, description, "agent_loop"),
        "mcp_server" => mcp_server_files(name, description),
        other => {
            return Err(format!(
                "unknown kind {other} — expected skill|workflow|adapter|loop|mcp_server"
            ));
        }
    };
    for rel in files.keys() {
        let path = resolve(&root, rel)?;
        if path.symlink_metadata().is_ok() {
            return Err(format!("{rel} already exists — pick another name"));
        }
    }
    let mut written = Vec::new();
    for (rel, content) in &files {
        let path = resolve(&root, rel)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("create parents: {e}"))?;
        }
        write_tmp(&path, content.as_bytes()).map_err(|e| format!("{rel}: {e}"))?;
        written.push(rel.clone());
    }
    let next = match kind {
        "skill" => "skills load by name in creative-mode prompts; edit SKILL.md then use it",
        "workflow" => "run the steps in order yourself, or register it so agents can load it",
        "mcp_server" => {
            "harness.register kind=mcp_server to make its tools callable via mcp.call_tool"
        }
        _ => {
            "build the crate, register the adapter manifest with the daemon, then bind it to a profile"
        }
    };
    Ok(json!({
        "kind": kind,
        "name": name,
        "files": written,
        "next": next,
    })
    .to_string())
}

fn skill_files(name: &str, description: &str) -> BTreeMap<String, String> {
    let desc = if description.is_empty() {
        format!("{name} skill")
    } else {
        description.to_owned()
    };
    let body = format!(
        "---\nname: {name}\ndescription: {desc}\n---\n\n# {name}\n\n\
## When to use\n\n{desc}\n\n## Inputs\n\n- <what this skill needs>\n\n\
## Steps\n\n1. <first step>\n2. <next step>\n\n## Output\n\n<what the skill produces>\n"
    );
    BTreeMap::from([(format!("skills/{name}/SKILL.md"), body)])
}

fn workflow_files(name: &str, description: &str) -> BTreeMap<String, String> {
    let body = json!({
        "name": name,
        "description": description,
        "steps": [
            {"task": "Describe the first step's task", "tools": false},
            {"task": "Describe the second step's task"},
        ],
        "notes": "Steps run in order — as child runs via spawn_agent, or sequentially in one run.",
    });
    BTreeMap::from([(
        format!("workflows/{name}.json"),
        serde_json::to_string_pretty(&body).unwrap_or_default() + "\n",
    )])
}

fn adapter_files(name: &str, description: &str, port: &str) -> BTreeMap<String, String> {
    let crate_name = name.replace('_', "-");
    // The kernel's adapter registry requires `id` to be a UUID — mint
    // one at scaffold time so the manifest registers as-is.
    let adapter_id = uuid::Uuid::now_v7().to_string();
    let manifest = json!({
        "manifest_version": 1,
        "id": adapter_id,
        "version": "0.1.0",
        "kind": "adapter",
        "runtime": {"type": "process", "entrypoint": crate_name, "language": "rust"},
        "implements": [format!("{port}@1")],
        "requested_capabilities": {},
    });
    let main_rs = ADAPTER_MAIN
        .replace("__NAME__", name)
        .replace("__PORT__", port);
    let readme = format!(
        "# {name}\n\n{description}\n\nGenerated `{port}` adapter skeleton.\n\n\
## Build\n\nMove this crate under `agent-os/fixtures/`, add it to the workspace\n\
`members`, then `cargo build -p {crate_name}`.\n\n\
## Activate\n\nAdapter bundles are content-addressed and digest-pinned: register\n\
the manifest with the daemon, then bind `{adapter_id}@0.1.0` to a runtime\n\
profile's `{port}` slot via a config proposal (Pipelines page or `agentctl`).\n"
    );
    let cargo = format!(
        "[package]\nname = \"{crate_name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
[dependencies]\n# Path deps assume this crate lives under agent-os/fixtures/\n\
adapter-protocol = {{ path = \"../../crates/adapter-protocol\" }}\n\
domain = {{ path = \"../../crates/domain\" }}\n\
prost = {{ version = \"0.13\" }}\nserde_json = {{ version = \"1\" }}\n"
    );
    BTreeMap::from([
        (format!("extensions/{name}/Cargo.toml"), cargo),
        (format!("extensions/{name}/src/main.rs"), main_rs),
        (
            format!("extensions/{name}/adapter.manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap_or_default() + "\n",
        ),
        (format!("extensions/{name}/README.md"), readme),
    ])
}

/// Minimal framed-protocol adapter skeleton — answers the bootstrap
/// handshake and echoes a canned response per request. `__NAME__` and
/// `__PORT__` are replaced at scaffold time.
const ADAPTER_MAIN: &str = r#"//! __NAME__ — generated adapter skeleton for port __PORT__.
//!
//! Speaks the framed adapter protocol on fd 0: read AdapterFrame,
//! answer PortCallRequest frames, return PortCallResponse frames.
//! Move this crate under agent-os/fixtures/ and add it to the
//! workspace members before building.

use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::process::ExitCode;

use adapter_protocol::framing::{read_frame, write_frame};
use domain::generated::contract::{
    AdapterFrame, AdapterHello, AdapterPong, PortCallResponse, adapter_frame::Body,
};
use prost::Message;

const PORT_ID: &str = "__PORT__";
const PROTOCOL_VERSION: u32 = 1;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("__NAME__: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> std::io::Result<()> {
    // SAFETY: fd 0 is the private socketpair end mapped by the supervisor.
    let fd = unsafe { OwnedFd::from_raw_fd(0) };
    let mut stream = UnixStream::from(fd);
    stream.set_read_timeout(None)?;

    let Some(frame) = read_frame(&mut stream)? else {
        return Err(std::io::Error::other("closed before bootstrap"));
    };
    let Some(Body::Bootstrap(bootstrap)) = frame.body else {
        return Err(std::io::Error::other("expected AdapterBootstrap"));
    };
    write_frame(
        &mut stream,
        &AdapterFrame {
            body: Some(Body::Hello(AdapterHello {
                adapter_instance_id: bootstrap.adapter_instance_id,
                adapter_id: std::env::var("AGENTOS_ADAPTER_ID").unwrap_or_default(),
                adapter_version: std::env::var("AGENTOS_ADAPTER_VERSION").unwrap_or_default(),
                bundle_digest: bootstrap.expected_bundle_digest,
                protocol_version: PROTOCOL_VERSION,
                implemented_ports: vec![PORT_ID.to_owned()],
                capability_document: Vec::new(),
            })),
        },
    )?;

    while let Some(frame) = read_frame(&mut stream)? {
        let Some(body) = frame.body else {
            continue;
        };
        let request = match body {
            Body::Request(req) => req,
            Body::Ping(p) => {
                write_frame(
                    &mut stream,
                    &AdapterFrame {
                        body: Some(Body::Pong(AdapterPong { nonce: p.nonce })),
                    },
                )?;
                continue;
            }
            Body::Shutdown(_) => return Ok(()),
            _ => continue,
        };
        // TODO: implement the port operations here — return a real
        // payload instead of `unimplemented`.
        let response = PortCallResponse {
            call_id: request.call_id,
            payload: Vec::new(),
            error_code: "unimplemented".to_owned(),
        };
        write_frame(
            &mut stream,
            &AdapterFrame {
                body: Some(Body::Response(response)),
            },
        )?;
    }
    Ok(())
}
"#;

fn mcp_server_files(name: &str, description: &str) -> BTreeMap<String, String> {
    let tool = name.replace('-', "_");
    let desc_py = serde_json::to_string(&json!(description)).unwrap_or_else(|_| "\"\"".to_owned());
    let server_py = MCP_SERVER_PY
        .replace("__NAME__", name)
        .replace("__TOOL__", &tool)
        .replace("__DESC__", &desc_py);
    let descriptor = json!({
        "command": "python3",
        "args": [format!("extensions/{name}/server.py")],
        "cwd": "<harness root>",
    });
    let readme = format!(
        "# {name}\n\n{description}\n\nGenerated stdio MCP tool server.\n\n\
## Install\n\nRegister it once so runs can call its tools:\n\n\
```\nharness.register {{kind: \"mcp_server\", name: \"{name}\",\n\
  command: \"python3\", args: [\"extensions/{name}/server.py\"]}}\n```\n\n\
Then call it from an agent run: `mcp.call_tool server={name} tool={tool}`.\n"
    );
    BTreeMap::from([
        (format!("extensions/{name}/server.py"), server_py),
        (
            format!("extensions/{name}/mcp-server.json"),
            serde_json::to_string_pretty(&descriptor).unwrap_or_default() + "\n",
        ),
        (format!("extensions/{name}/README.md"), readme),
    ])
}

/// Minimal stdio MCP server (NDJSON JSON-RPC) template — same wire
/// shape as `fixtures/echo-mcp`. `__NAME__`, `__TOOL__`, `__DESC__` are
/// replaced at scaffold time; `__DESC__` carries a JSON-quoted string.
const MCP_SERVER_PY: &str = r#"#!/usr/bin/env python3
"""__NAME__ — generated stdio MCP server (NDJSON JSON-RPC)."""
import json
import sys

TOOLS = [
    {
        "name": "__TOOL__",
        "description": __DESC__,
        "inputSchema": {
            "type": "object",
            "properties": {"text": {"type": "string"}},
            "required": ["text"],
        },
    }
]


def handle(method, params):
    if method == "initialize":
        return {
            "protocolVersion": "2025-03-26",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "__NAME__", "version": "0.1.0"},
        }
    if method == "ping":
        return {}
    if method == "tools/list":
        return {"tools": TOOLS}
    if method == "tools/call":
        if params.get("name") != "__TOOL__":
            raise ValueError("unknown tool")
        text = params.get("arguments", {}).get("text", "")
        # TODO: implement the tool behavior here.
        return {"content": [{"type": "text", "text": text}], "isError": False}
    raise ValueError("method not found")


for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        msg = json.loads(line)
    except ValueError:
        continue
    if "id" not in msg:
        continue  # notifications carry no id
    try:
        reply = {
            "jsonrpc": "2.0",
            "id": msg["id"],
            "result": handle(msg.get("method"), msg.get("params") or {}),
        }
    except Exception as e:
        reply = {
            "jsonrpc": "2.0",
            "id": msg["id"],
            "error": {"code": -32602, "message": str(e)},
        }
    sys.stdout.write(json.dumps(reply) + "\n")
    sys.stdout.flush()
"#;

fn validate(payload: &Value) -> Result<String, String> {
    let root = root()?;
    let rel = payload
        .get("path")
        .and_then(Value::as_str)
        .ok_or("missing `path`")?;
    let path = resolve(&root, rel)?;
    let body = std::fs::read_to_string(&path).map_err(|e| format!("read {rel}: {e}"))?;
    let mut issues: Vec<String> = Vec::new();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    if name == "adapter.manifest.json" {
        match serde_json::from_str::<Value>(&body) {
            Ok(manifest) => {
                for field in [
                    "manifest_version",
                    "id",
                    "version",
                    "kind",
                    "runtime",
                    "implements",
                ] {
                    if manifest.get(field).is_none() {
                        issues.push(format!("missing `{field}`"));
                    }
                }
                if manifest
                    .get("implements")
                    .and_then(Value::as_array)
                    .map(|a| a.is_empty())
                    .unwrap_or(true)
                {
                    issues.push(
                        "`implements` must name at least one port (e.g. \"agent_loop@1\")"
                            .to_owned(),
                    );
                }
                if manifest
                    .get("runtime")
                    .and_then(|r| r.get("entrypoint"))
                    .is_none()
                {
                    issues.push("`runtime.entrypoint` missing".to_owned());
                }
                // The kernel registry parses `id` as an AdapterId —
                // flag non-UUID values before register attempts fail.
                match manifest.get("id").and_then(Value::as_str) {
                    Some(id) if uuid::Uuid::parse_str(id).is_ok() => {}
                    Some(_) => issues.push("`id` must be a UUID".to_owned()),
                    None => {}
                }
            }
            Err(e) => issues.push(format!("manifest is not valid JSON: {e}")),
        }
    } else if name == "SKILL.md" {
        if !body.starts_with("---") {
            issues.push("missing YAML frontmatter (`---` block)".to_owned());
        } else {
            let fm_end = body[3..].find("---").map(|i| i + 3);
            let fm = fm_end.map(|e| &body[3..e]).unwrap_or_default();
            if !fm.lines().any(|l| l.trim_start().starts_with("name:")) {
                issues.push("frontmatter missing `name`".to_owned());
            }
            if !fm
                .lines()
                .any(|l| l.trim_start().starts_with("description:"))
            {
                issues.push("frontmatter missing `description`".to_owned());
            }
        }
    } else if name.ends_with(".json") {
        match serde_json::from_str::<Value>(&body) {
            Ok(v) => {
                if rel.starts_with("workflows/") {
                    if v.get("steps")
                        .and_then(Value::as_array)
                        .map(|s| s.is_empty())
                        .unwrap_or(true)
                    {
                        issues.push("workflow needs a non-empty `steps` array".to_owned());
                    }
                    if v.get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .is_empty()
                    {
                        issues.push("workflow missing `name`".to_owned());
                    }
                }
            }
            Err(e) => issues.push(format!("not valid JSON: {e}")),
        }
    }
    Ok(json!({"path": rel, "valid": issues.is_empty(), "issues": issues}).to_string())
}

fn load_registry(path: &Path) -> BTreeMap<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|body| serde_json::from_str::<Value>(&body).ok())
        .and_then(|v| v.get("extensions").and_then(Value::as_object).cloned())
        .map(|m| m.into_iter().collect())
        .unwrap_or_default()
}

fn save_registry(path: &Path, entries: &BTreeMap<String, Value>) -> Result<(), String> {
    let body =
        serde_json::to_string_pretty(&json!({"extensions": entries})).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    write_tmp(&tmp, body.as_bytes()).map_err(|e| format!("{e} (registry)"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename registry: {e}"))
}

fn register(payload: &Value) -> Result<String, String> {
    let root = ensure_root()?;
    let kind = payload
        .get("kind")
        .and_then(Value::as_str)
        .ok_or("missing `kind`")?;
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .ok_or("missing `name`")?;
    validate_name(name)?;
    let description = payload
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();

    let (entry, status) = match kind {
        // MCP servers go live immediately: the client merges
        // mcp-servers.json on every call.
        "mcp_server" => {
            let command = payload
                .get("command")
                .and_then(Value::as_str)
                .ok_or("mcp_server registration needs `command`")?;
            let args: Vec<String> = payload
                .get("args")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let env_map: BTreeMap<String, String> = payload
                .get("env")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                        .collect()
                })
                .unwrap_or_default();
            let cwd = payload
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| root.display().to_string());
            // Same resolution as mcp.rs — MCP_SERVERS_FILE wins, then
            // <root>/mcp-servers.json. Writing anywhere else would
            // register a server the MCP client never sees.
            let file = mcp_servers_file()
                .ok_or("AGENTOS_HARNESS_ROOT unset and MCP_SERVERS_FILE unknown")?;
            let mut servers: Value = std::fs::read_to_string(&file)
                .ok()
                .and_then(|b| serde_json::from_str(&b).ok())
                .unwrap_or_else(|| json!({}));
            servers[name] = json!({
                "command": command,
                "args": args,
                "env": env_map,
                "cwd": cwd,
            });
            let tmp = file.with_extension("tmp");
            write_tmp(
                &tmp,
                serde_json::to_string_pretty(&servers)
                    .unwrap_or_default()
                    .as_bytes(),
            )?;
            std::fs::rename(&tmp, &file).map_err(|e| format!("write mcp-servers.json: {e}"))?;
            (
                json!({"command": command, "args": args, "cwd": cwd}),
                "live",
            )
        }
        "skill" => {
            let path = format!("skills/{name}/SKILL.md");
            if !resolve(&root, &path).map(|p| p.exists()).unwrap_or(false) {
                return Err(format!(
                    "{path} does not exist — scaffold or write it first"
                ));
            }
            (json!({"path": path}), "installed")
        }
        "workflow" => {
            let path = format!("workflows/{name}.json");
            if !resolve(&root, &path).map(|p| p.exists()).unwrap_or(false) {
                return Err(format!(
                    "{path} does not exist — scaffold or write it first"
                ));
            }
            (json!({"path": path}), "installed")
        }
        // Compiled adapters still need build + kernel registration; the
        // registry records them so later runs see the pending work.
        "adapter" | "loop" => {
            let dir = format!("extensions/{name}");
            if !resolve(&root, &dir).map(|p| p.is_dir()).unwrap_or(false) {
                return Err(format!("{dir} does not exist — scaffold it first"));
            }
            (
                json!({"path": dir}),
                "needs_build — compile, register the manifest, bind to a profile",
            )
        }
        other => return Err(format!("unknown kind {other}")),
    };

    let file = registry_file(&root);
    let mut entries = load_registry(&file);
    entries.insert(
        name.to_owned(),
        json!({
            "kind": kind,
            "name": name,
            "description": description,
            "status": status,
            "entry": entry,
            "registered_at_ms": now_ms(),
        }),
    );
    save_registry(&file, &entries)?;
    Ok(json!({
        "registered": name,
        "kind": kind,
        "status": status,
    })
    .to_string())
}

fn registry() -> Result<String, String> {
    let root = root()?;
    let file = registry_file(&root);
    let entries = load_registry(&file);
    Ok(json!({"root": root.display().to_string(), "extensions": entries}).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    // Tests mutate the shared process env — serialize them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());
    static SEQ: AtomicU64 = AtomicU64::new(0);

    struct RootGuard {
        dir: PathBuf,
        prev: Option<String>,
    }

    impl RootGuard {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!(
                "harness-test-{}-{}",
                std::process::id(),
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let prev = std::env::var("AGENTOS_HARNESS_ROOT").ok();
            unsafe {
                std::env::set_var("AGENTOS_HARNESS_ROOT", &dir);
            }
            Self { dir, prev }
        }
    }

    impl Drop for RootGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var("AGENTOS_HARNESS_ROOT", v),
                    None => std::env::remove_var("AGENTOS_HARNESS_ROOT"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn write_then_read_roundtrip() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _root = RootGuard::new();
        call(&json!({"op": "harness.write", "path": "a/b.txt", "content": "hello"})).unwrap();
        let out = call(&json!({"op": "harness.read", "path": "a/b.txt"})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["content"], "hello");
    }

    #[test]
    fn path_traversal_rejected() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _root = RootGuard::new();
        assert!(
            call(&json!({"op": "harness.write", "path": "../escape", "content": "x"})).is_err()
        );
        assert!(call(&json!({"op": "harness.read", "path": "/etc/passwd"})).is_err());
    }

    #[test]
    fn symlink_escape_rejected() {
        let _lock = ENV_LOCK.lock().unwrap();
        let guard = RootGuard::new();
        let outside = std::env::temp_dir().join(format!(
            "harness-outside-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret.txt"), "s3cret").unwrap();
        std::os::unix::fs::symlink(&outside, guard.dir.join("link")).unwrap();
        assert!(call(&json!({"op": "harness.read", "path": "link/secret.txt"})).is_err());
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn scaffold_skill_validates() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _root = RootGuard::new();
        call(&json!({"op": "harness.scaffold", "kind": "skill", "name": "my-skill", "description": "demo"}))
            .unwrap();
        let out =
            call(&json!({"op": "harness.validate", "path": "skills/my-skill/SKILL.md"})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["valid"], true, "{out}");
        let body =
            std::fs::read_to_string(_root.dir.join("skills").join("my-skill").join("SKILL.md"))
                .unwrap();
        assert!(body.contains("name: my-skill"));
    }

    #[test]
    fn register_mcp_server_goes_live() {
        let _lock = ENV_LOCK.lock().unwrap();
        let guard = RootGuard::new();
        call(&json!({
            "op": "harness.register", "kind": "mcp_server", "name": "mytool",
            "command": "python3", "args": ["extensions/mytool/server.py"],
        }))
        .unwrap();
        let body = std::fs::read_to_string(guard.dir.join("mcp-servers.json")).unwrap();
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["mytool"]["command"], "python3");
        let reg = call(&json!({"op": "harness.registry"})).unwrap();
        let r: Value = serde_json::from_str(&reg).unwrap();
        assert_eq!(r["extensions"]["mytool"]["status"], "live");
    }

    #[test]
    fn register_skill_requires_existing_file() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _root = RootGuard::new();
        assert!(
            call(&json!({"op": "harness.register", "kind": "skill", "name": "ghost"})).is_err()
        );
    }

    #[test]
    fn workflow_scaffold_validates() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _root = RootGuard::new();
        call(&json!({"op": "harness.scaffold", "kind": "workflow", "name": "deploy-check"}))
            .unwrap();
        let out = call(&json!({"op": "harness.validate", "path": "workflows/deploy-check.json"}))
            .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["valid"], true, "{out}");
    }
}
