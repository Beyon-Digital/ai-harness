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

    // The decisions endpoint answers even for a run with no decisions.
    let (code, body) = http_get(port, &format!("/api/runs/{run_id}/decisions"));
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["decisions"], serde_json::json!([]));

    // Metrics read the durable stores read-only while the daemon runs.
    let (code, body) = http_get(port, "/api/metrics");
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["index"]["runs"].as_u64().unwrap() >= 1, "{v}");
    assert!(v["kernel_db_error"].is_null(), "{v}");

    // Generation pipeline surface answers (empty — no --config at boot).
    let (code, body) = http_get(port, "/api/config/generations");
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["generations"].as_array().unwrap().is_empty(), "{v}");

    // An unbound run (no spec) has no resolved environment → 404.
    let (code, body) = http_get(port, &format!("/api/runs/{run_id}/environment"));
    assert_eq!(code, 404, "{body}");

    drop(agentgw);
    drop(agentd);
}

/// Phase-14 device auth: a per-device bearer token authenticates against
/// `--devices-file`, revocation takes effect without a restart, and the
/// master token keeps working.
#[test]
fn gateway_device_auth_and_revocation() {
    let dir = tempfile::tempdir().unwrap();
    let devices = dir.path().join("devices.json");

    // Register a device via the CLI.
    let out = Command::new(target_bin("agentgw"))
        .args([
            "device",
            "add",
            "--devices-file",
            devices.to_str().unwrap(),
            "--label",
            "laptop",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let device_id = v["device_id"].as_str().unwrap().to_owned();
    let device_token = v["token"].as_str().unwrap().to_owned();
    assert!(device_token.starts_with("gwdev_"));

    // The file stores only the hash.
    let on_disk = std::fs::read_to_string(&devices).unwrap();
    assert!(!on_disk.contains(&device_token));
    assert!(on_disk.contains("sha256:"));

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
            .arg("--auth-token")
            .arg("master-secret")
            .arg("--devices-file")
            .arg(&devices)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );

    let authed_get = |token: &str| -> u16 {
        let req = ureq::get(&format!("http://127.0.0.1:{port}/api/health"));
        let req = if token.is_empty() {
            req
        } else {
            req.header("Authorization", &format!("Bearer {token}"))
        };
        match req.call() {
            Ok(r) => r.status().as_u16(),
            Err(ureq::Error::StatusCode(c)) => c,
            Err(_) => 0,
        }
    };

    // Poll until up (master token).
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if authed_get("master-secret") == 200 {
            break;
        }
        assert!(Instant::now() < deadline, "agentgw never came up");
        std::thread::park_timeout(Duration::from_millis(50));
    }

    assert_eq!(authed_get(""), 401);
    assert_eq!(authed_get("wrong-token"), 401);
    assert_eq!(authed_get(&device_token), 200);

    // Revoke — the next request re-reads the file, no restart.
    let out = Command::new(target_bin("agentgw"))
        .args([
            "device",
            "revoke",
            &device_id,
            "--devices-file",
            devices.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(authed_get(&device_token), 401);

    drop(agentgw);
    drop(agentd);
}

/// A scripted run under `local-trusted` produces LoopDecisionAccepted
/// events the decisions endpoint decodes into readable JSON.
#[test]
fn gateway_decisions_decodes_run_stream() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = dir.path().join("run");
    std::fs::create_dir_all(&runtime).unwrap();

    // default.yaml's local-trusted profile needs the fixture bundles.
    let ws = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mk = |bin: &str, manifest: &str, out: &str| {
        Command::new("bash")
            .arg(ws.join("scripts/make-adapter-bundle.sh"))
            .arg(ws.join(format!("target/debug/{bin}")))
            .arg(ws.join(manifest))
            .arg(dir.path().join(out))
            .output()
            .unwrap();
    };
    mk(
        "fixture-agent-loop",
        "fixtures/agent-loop/adapter.manifest.json",
        "fixture-loop",
    );
    mk(
        "fixture-effect-adapter",
        "fixtures/effect-adapter/adapter.manifest.json",
        "fixture-effect",
    );

    let agentd = Proc(
        Command::new(target_bin("agentd"))
            .arg("--runtime-dir")
            .arg(&runtime)
            .arg("--config")
            .arg(ws.join("config/default.yaml"))
            .arg("--adapter-bundle")
            .arg(dir.path().join("fixture-loop"))
            .arg("--adapter-bundle")
            .arg(dir.path().join("fixture-effect"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let socket = runtime.join("control.sock");
    let deadline = Instant::now() + Duration::from_secs(15);
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
        let (code, _) = http_get(port, "/api/health");
        if code == 200 {
            break;
        }
        assert!(Instant::now() < deadline, "agentgw never came up");
        std::thread::park_timeout(Duration::from_millis(50));
    }

    // Session + spec (local-trusted) + run. The fixture loop has no script
    // for this run id, so it answers `fail` — still an accepted decision.
    let (code, body) = http_post_json(port, "/api/sessions", serde_json::json!({}));
    let session_id: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(code, 200, "{body}");
    let session_id = session_id["session_id"].as_str().unwrap().to_owned();

    let spec_body =
        serde_json::to_vec(&serde_json::json!({"runtime_profile_name": "local-trusted"})).unwrap();
    use sha2::Digest;
    let digest = sha2::Sha256::digest(&spec_body)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let spec_id = "01999999-0000-7000-8000-00000000b0b1";
    let (code, body) = http_post_json(
        port,
        "/api/specs",
        serde_json::json!({
            "agent_spec_id": spec_id,
            "version": "v1",
            "body": std::str::from_utf8(&spec_body).unwrap(),
        }),
    );
    assert_eq!(code, 200, "{body}");

    let run_id = "01905c5e-0000-7000-8000-00de51510001";
    let (code, body) = http_post_json(
        port,
        "/api/runs",
        serde_json::json!({
            "session_id": session_id,
            "task_id": run_id,
            "run_id": run_id,
            "agent_spec_id": spec_id,
            "spec_version": "v1",
            "spec_digest": digest,
            "task_payload": "say hi",
            "requested_profile": "local-trusted",
        }),
    );
    assert_eq!(code, 200, "{body}");

    // Wait for the terminal state, then read the decoded decisions.
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (code, body) = http_get(port, &format!("/api/runs/{run_id}"));
        if code == 200 {
            let v: serde_json::Value = serde_json::from_str(&body).unwrap();
            if ["completed", "failed", "cancelled"]
                .contains(&v["state_name"].as_str().unwrap_or(""))
            {
                break;
            }
        }
        assert!(Instant::now() < deadline, "run never settled");
        std::thread::park_timeout(Duration::from_millis(50));
    }
    // The journal trails the run row slightly: a terminal run can read
    // back before its LoopDecisionAccepted event is flushed, so poll for
    // the decision rather than reading once.
    let deadline = Instant::now() + Duration::from_secs(10);
    let decisions = loop {
        let (code, body) = http_get(port, &format!("/api/runs/{run_id}/decisions"));
        assert_eq!(code, 200, "{body}");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let decisions = v["decisions"].as_array().unwrap().clone();
        if !decisions.is_empty() {
            break decisions;
        }
        if Instant::now() >= deadline {
            let (_, ev) = http_get(
                port,
                &format!("/api/events/read?stream_key=run/{run_id}&limit=50"),
            );
            panic!("expected at least one decision; events={ev}");
        }
        std::thread::park_timeout(Duration::from_millis(50));
    };
    assert_eq!(decisions[0]["kind"], "fail");
    assert_eq!(
        decisions[0]["detail"]["reason_code"].as_str().unwrap(),
        "script_exhausted"
    );

    // Bound run → frozen environment row + effect_execute binding.
    let (code, body) = http_get(port, &format!("/api/runs/{run_id}/environment"));
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let env = &v["environment"];
    assert_eq!(env["run_id"], run_id);
    assert_eq!(
        env["agent_loop_id"].as_str().unwrap(),
        "01905c5e-0000-7000-8000-a9e97100f1a1"
    );
    assert_eq!(env["config_generation_id"].as_str().unwrap().len(), 36);
    let bindings = v["bindings"].as_array().unwrap();
    assert!(
        bindings.iter().any(|b| b["port_id"] == "effect.execute"),
        "{bindings:?}"
    );

    // Boot --config created one generation and activated it.
    let (code, body) = http_get(port, "/api/config/generations");
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let gens = v["generations"].as_array().unwrap();
    assert_eq!(gens.len(), 1, "{v}");
    assert_eq!(gens[0]["active"], true);
    assert_eq!(gens[0]["generation_id"], env["config_generation_id"]);

    // Metrics now see a terminal run + the committed effect rows.
    let (code, body) = http_get(port, "/api/metrics");
    assert_eq!(code, 200, "{body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let states = v["runs_by_state"].as_array().unwrap();
    assert!(
        states.iter().any(|s| s["state"] == 10 && s["count"] == 1),
        "{states:?}"
    );

    drop(agentgw);
    drop(agentd);
}
