//! Protocol-level checks for the `echo-acp` fixture: initialize,
//! session/new, streamed `session/update` chunks during
//! `session/prompt`, and the `session/request_permission` round-trip.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ndjson_rpc::{Client, Incoming, Reply, Spawn};
use serde_json::json;

const T: Duration = Duration::from_secs(10);

fn spawn() -> Client {
    Client::spawn(Spawn {
        command: env!("CARGO_BIN_EXE_echo-acp").to_owned(),
        args: Vec::new(),
        env: Vec::new(),
        cwd: None,
    })
    .expect("spawn echo-acp")
}

fn session(client: &mut Client) -> String {
    client
        .request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {"name": "test", "version": "0"},
            }),
            T,
            &mut |_| Reply::Ignore,
        )
        .expect("initialize");
    let s = client
        .request(
            "session/new",
            json!({"cwd": ".", "mcpServers": []}),
            T,
            &mut |_| Reply::Ignore,
        )
        .expect("session/new");
    s["sessionId"].as_str().unwrap().to_owned()
}

#[test]
fn prompt_streams_chunks_and_completes() {
    let mut c = spawn();
    let session_id = session(&mut c);
    let text = Arc::new(Mutex::new(String::new()));
    let text2 = Arc::clone(&text);
    let result = c
        .request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "hi there"}],
            }),
            T,
            &mut |incoming| {
                if let Incoming::Notification { method, params } = incoming
                    && method == "session/update"
                    && params["update"]["sessionUpdate"] == "agent_message_chunk"
                    && let Some(t) = params["update"]["content"]["text"].as_str()
                {
                    text2.lock().unwrap().push_str(t);
                }
                Reply::Ignore
            },
        )
        .expect("session/prompt");
    assert_eq!(result["stopReason"], "end_turn");
    assert_eq!(*text.lock().unwrap(), "acp-echo:hi there");
}

#[test]
fn permission_request_is_answered() {
    let mut c = spawn();
    let session_id = session(&mut c);
    let result = c
        .request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": "perm-required now"}],
            }),
            T,
            &mut |incoming| match incoming {
                Incoming::Request(req) => {
                    assert_eq!(req.method, "session/request_permission");
                    let option = req.params["options"][0]["optionId"].as_str().unwrap();
                    Reply::Result(json!({
                        "outcome": {"outcome": "selected", "optionId": option},
                    }))
                }
                _ => Reply::Ignore,
            },
        )
        .expect("session/prompt with permission");
    assert_eq!(result["stopReason"], "end_turn");
}
