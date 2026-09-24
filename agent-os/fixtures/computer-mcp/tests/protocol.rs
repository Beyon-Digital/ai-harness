use std::time::Duration;

use ndjson_rpc::{Client, Reply, Spawn};
use serde_json::json;

const TIMEOUT: Duration = Duration::from_secs(10);

fn client() -> Client {
    Client::spawn(Spawn {
        command: env!("CARGO_BIN_EXE_computer-mcp").to_owned(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: None,
    })
    .expect("spawn computer-mcp")
}

#[test]
fn exposes_computer_tools() {
    let mut client = client();
    let initialized = client
        .request("initialize", json!({}), TIMEOUT, &mut |_| Reply::Ignore)
        .expect("initialize");
    assert_eq!(initialized["serverInfo"]["name"], "computer-mcp");
    let result = client
        .request("tools/list", json!({}), TIMEOUT, &mut |_| Reply::Ignore)
        .expect("tools/list");
    let names = result["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect::<Vec<_>>();
    assert_eq!(names, ["screenshot", "click", "type", "key", "wait"]);
}

#[test]
fn wait_tool_executes() {
    let mut client = client();
    let result = client
        .request(
            "tools/call",
            json!({"name": "wait", "arguments": {"milliseconds": 1}}),
            TIMEOUT,
            &mut |_| Reply::Ignore,
        )
        .expect("tools/call");
    assert_eq!(result["isError"], false);
    assert_eq!(result["content"][0]["text"], "Waited 1 ms.");
}
