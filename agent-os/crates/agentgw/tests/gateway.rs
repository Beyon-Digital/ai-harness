//! End-to-end gateway test: real `agentd` + `agentgw` binaries, driven
//! over plain HTTP.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn target_bin(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/debug/{name}"));
    assert!(
        path.exists(),
        "{name} binary missing; run `cargo build --workspace` first"
    );
    path
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

struct Proc(Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn http_get(port: u16, path: &str) -> (u16, String) {
    let resp = ureq::get(&format!("http://127.0.0.1:{port}{path}")).call();
    match resp {
        Ok(mut r) => (
            r.status().as_u16(),
            r.body_mut().read_to_string().unwrap_or_default(),
        ),
        Err(ureq::Error::StatusCode(code)) => (code, String::new()),
        // Not-yet-listening / connection-level errors report as 0 so the
        // poll loop can retry.
        Err(_) => (0, String::new()),
    }
}

fn http_post_json(port: u16, path: &str, body: serde_json::Value) -> (u16, String) {
    let resp = ureq::post(&format!("http://127.0.0.1:{port}{path}"))
        .header("Content-Type", "application/json")
        .send_json(body);
    match resp {
        Ok(mut r) => (
            r.status().as_u16(),
            r.body_mut().read_to_string().unwrap_or_default(),
        ),
        Err(ureq::Error::StatusCode(code)) => (code, String::new()),
        Err(_) => (0, String::new()),
    }
}

#[test]
fn gateway_serves_health_index_and_rest() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("run");
    std::fs::create_dir_all(&runtime).unwrap();

    let agentd = Proc(
        Command::new(target_bin("agentd"))
            .arg("--runtime-dir")
            .arg(&runtime)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let socket = runtime.join("control.sock");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !socket.exists() {
        assert!(Instant::now() < deadline, "agentd socket never appeared");
        std::thread::park_timeout(Duration::from_millis(50));
    }

    let port = free_port();
    let agentgw = Proc(
        Command::new(target_bin("agentgw"))
            .arg("--socket")
            .arg(&socket)
            .arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let (code, body) = http_get(port, "/api/health");
        if code == 200 {
            let v: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(v["status"], "running");
            break;
        }
        assert!(Instant::now() < deadline, "agentgw never came up: {code}");
        std::thread::park_timeout(Duration::from_millis(50));
    }

    // GUI is served at the root.
    let (code, html) = http_get(port, "/");
    assert_eq!(code, 200);
    assert!(html.contains("Agent OS"));

    // Create a session — no id supplied, the gateway mints a uuidv7.
    let (code, body) = http_post_json(port, "/api/sessions", serde_json::json!({}));
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let session_id = v["session_id"].as_str().unwrap().to_owned();
    assert!(!session_id.is_empty());

    // Create a run against it (unbound — no spec — but accepted).
    let (code, body) = http_post_json(
        port,
        "/api/runs",
        serde_json::json!({"session_id": session_id, "task_kind": "agent"}),
    );
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let run_id = v["run_id"].as_str().unwrap().to_owned();
    assert!(!run_id.is_empty());

    // Index remembers both; run is queryable.
    let (code, body) = http_get(port, "/api/index");
    assert_eq!(code, 200);
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["sessions"][0], session_id);
    assert_eq!(v["runs"][0]["run_id"], run_id);

    let (code, body) = http_get(port, &format!("/api/runs/{run_id}"));
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["run_id"], run_id);

    // Approvals for the run list cleanly (empty).
    let (code, body) = http_get(port, &format!("/api/approvals?run_id={run_id}"));
    assert_eq!(code, 200, "{body}");

    drop(agentgw);
    drop(agentd);
}
