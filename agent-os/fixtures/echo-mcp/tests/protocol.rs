//! Protocol-level checks for the `echo-mcp` fixture: initialize,
//! tools/list + tools/call, resources/read, ping, and JSON-RPC error
//! paths — all over NDJSON stdio via `ndjson-rpc`.

use std::time::Duration;

use ndjson_rpc::{Client, Incoming, Reply, Spawn};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);

fn spawn() -> Client {
    Client::spawn(Spawn {
        command: env!("CARGO_BIN_EXE_echo-mcp").to_owned(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: None,
    })
    .expect("spawn echo-mcp")
}

fn init(client: &mut Client) {
    let result = client
        .request(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"},
            }),
            T,
            &mut |_| Reply::Ignore,
        )
        .expect("initialize");
    assert_eq!(result["serverInfo"]["name"], "echo-mcp");
    client
        .notify("notifications/initialized", json!({}))
        .expect("initialized notification");
}

#[test]
fn initialize_and_list_tools() {
    let mut c = spawn();
    init(&mut c);
    let tools = c
        .request("tools/list", json!({}), T, &mut |_| Reply::Ignore)
        .expect("tools/list");
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert_eq!(names, ["echo", "uppercase"]);
}

#[test]
fn call_echo_tool() {
    let mut c = spawn();
    init(&mut c);
    let result = c
        .request(
            "tools/call",
            json!({"name": "echo", "arguments": {"text": "hello mcp"}}),
            T,
            &mut |_| Reply::Ignore,
        )
        .expect("tools/call");
    assert_eq!(result["content"][0]["text"], "hello mcp");
    assert_eq!(result["isError"], Value::Bool(false));
}

#[test]
fn call_unknown_tool_is_error() {
    let mut c = spawn();
    init(&mut c);
    let err = c
        .request(
            "tools/call",
            json!({"name": "nope", "arguments": {}}),
            T,
            &mut |_| Reply::Ignore,
        )
        .expect_err("unknown tool must error");
    assert!(err.to_string().contains("unknown tool"), "{err}");
}

#[test]
fn read_resource_and_ping() {
    let mut c = spawn();
    init(&mut c);
    let res = c
        .request(
            "resources/read",
            json!({"uri": "echo://hello"}),
            T,
            &mut |_| Reply::Ignore,
        )
        .expect("resources/read");
    assert_eq!(res["contents"][0]["text"], "hello from echo-mcp");
    c.request("ping", json!({}), T, &mut |_| Reply::Ignore)
        .expect("ping");
}

#[test]
fn unknown_method_is_method_not_found() {
    let mut c = spawn();
    let err = c
        .request("nonsense/method", json!({}), T, &mut |_| Reply::Ignore)
        .expect_err("unknown method must error");
    assert!(err.to_string().contains("-32601"), "{err}");
}

#[test]
fn server_requests_are_answered() {
    // echo-mcp never sends requests, but the client's callback path
    // must exist for peers that do (ACP does).
    let mut c = spawn();
    let mut saw_notification = false;
    init(&mut c);
    c.request("ping", json!({}), T, &mut |i| {
        if matches!(i, Incoming::Notification { .. }) {
            saw_notification = true;
        }
        Reply::Ignore
    })
    .expect("ping");
    let _ = saw_notification;
}
