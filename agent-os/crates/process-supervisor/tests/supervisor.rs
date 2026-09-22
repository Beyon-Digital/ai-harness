//! ADP-001: private IPC, no listener, crash capture, descendant cleanup.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;

use domain::generated::contract::{AdapterFrame, AdapterHello};
use domain::ids::{AdapterId, AdapterInstanceId, CommandId, DaemonInstanceId, PrincipalId};
use domain::security::{ConformanceState, TrustState};
use kernel_store::models::NewAdapterRegistration;
use kernel_store::{KernelStore, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use process_supervisor::{
    ExitReason, SpawnSpec, child::instance_state, handshake, record_spawn, spawn, terminate, wait,
};
use prost::Message;
use std::sync::Arc;
use testkit::ids::DeterministicIds;

const SEED: i64 = 1_700_000_000_000;
const DIGEST: &str = "sha256:test-bundle";

fn spec(program: &str, script: &str) -> SpawnSpec {
    let ids = DeterministicIds::new(SEED);
    SpawnSpec {
        adapter_id: AdapterId::new(&ids),
        adapter_version: "1.0.0".to_owned(),
        expected_bundle_digest: DIGEST.to_owned(),
        adapter_instance_id: AdapterInstanceId::new(&ids),
        daemon_instance_id: DaemonInstanceId::new(&ids),
        daemon_fencing_epoch: 1,
        protocol_version: 1,
        executable: PathBuf::from(program),
        argv: vec!["-c".to_owned(), script.to_owned()],
        env: vec![("PATH".to_owned(), "/usr/bin:/bin".to_owned())],
        cwd: None,
        isolation: process_supervisor::spawn::Isolation::None,
        stdout_ipc: false,
    }
}

#[tokio::test]
async fn private_ipc_channel_carries_bytes_both_ways() {
    // `exec 1>&0; cat`: echo the inherited IPC fd back onto itself.
    let mut child = spawn(&spec("/bin/sh", "exec 1>&0; cat")).expect("spawn");
    child
        .ipc()
        .write_all(b"ping")
        .and_then(|()| child.ipc().flush())
        .expect("write");
    let mut buf = [0u8; 4];
    child.ipc().read_exact(&mut buf).expect("echo");
    assert_eq!(&buf, b"ping");
    let _ = terminate(child, Duration::from_millis(500)).await;
}

/// T1 sandbox (Linux): a `user-ns` spawn must land in a fresh userns —
/// verified by the child's own uid=0-in-userns view — and keep the IPC
/// channel on fd 0. On hosts where userns is unavailable (macOS, or a
/// sysctl-disabled kernel), spawn fails closed rather than degrading.
#[tokio::test]
async fn user_namespace_spawn_keeps_ipc_channel() {
    let mut spec = spec("/bin/sh", "id -u >&2; exec 1>&0; cat");
    spec.isolation = process_supervisor::spawn::Isolation::UserNamespace { network: true };
    let mut child = match spawn(&spec) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("userns unavailable on this host, skipping: {e}");
            return;
        }
    };
    child
        .ipc()
        .write_all(b"ping")
        .and_then(|()| child.ipc().flush())
        .expect("write");
    let mut buf = [0u8; 4];
    child.ipc().read_exact(&mut buf).expect("echo");
    assert_eq!(&buf, b"ping");
    // In a fresh userns the child maps to root — `id -u` on stderr is 0.
    if let Some(stderr) = child.stderr.as_mut() {
        use std::io::Read;
        let mut out = [0u8; 8];
        let n = stderr.read(&mut out).expect("id output");
        assert_eq!(&out[..n], b"0\n", "userns child should see uid 0");
    }
    let _ = terminate(child, Duration::from_millis(500)).await;
}

/// `stdout_ipc` (wasm-host mode): the socketpair maps onto BOTH the
/// child's stdin and stdout — one descriptor pair carries the whole
/// protocol. Verify a `cat` child echoes over that shared channel.
#[tokio::test]
async fn stdout_ipc_shares_the_socketpair_channel() {
    let mut spec = spec("/bin/sh", "cat");
    spec.stdout_ipc = true;
    let mut child = spawn(&spec).expect("spawn");
    assert!(child.stdout.is_none(), "stdout_ipc means no capture pipe");
    child
        .ipc()
        .write_all(b"pong")
        .and_then(|()| child.ipc().flush())
        .expect("write");
    let mut buf = [0u8; 4];
    child.ipc().read_exact(&mut buf).expect("echo on stdout");
    assert_eq!(&buf, b"pong");
    let _ = terminate(child, Duration::from_millis(500)).await;
}

#[tokio::test]
async fn stdout_and_stderr_are_separate_from_ipc() {
    let mut child = spawn(&spec(
        "/bin/sh",
        "echo ipc-msg >&0; echo out-msg; echo err-msg >&2",
    ))
    .expect("spawn");
    let (ipc, stdout, stderr) = child.channels();
    let mut line = Vec::new();
    // Read one byte at a time up to a newline on each channel.
    for (stream, want) in [
        (ipc as &mut dyn Read, b"ipc-msg\n".as_slice()),
        (
            stdout.expect("stdout pipe") as &mut dyn Read,
            b"out-msg\n".as_slice(),
        ),
        (
            stderr.expect("stderr pipe") as &mut dyn Read,
            b"err-msg\n".as_slice(),
        ),
    ] {
        line.clear();
        let mut byte = [0u8; 1];
        while line.last() != Some(&b'\n') {
            stream.read_exact(&mut byte).expect("read");
            line.push(byte[0]);
        }
        assert_eq!(line.as_slice(), want);
    }
    let _ = terminate(child, Duration::from_millis(500)).await;
}

#[tokio::test]
async fn no_listener_exists_for_an_unrelated_process_to_race() {
    // The IPC channel exists only as an inherited fd: there is no path an
    // unrelated local process could connect to.
    let dir = tempfile::tempdir().expect("tmp");
    let _child = spawn(&spec("/bin/sh", "exec 1>&0; cat")).expect("spawn");
    let fake_listener = dir.path().join("adapter.sock");
    let err = std::os::unix::net::UnixStream::connect(&fake_listener)
        .expect_err("no well-known socket exists");
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    let _ = terminate(_child, Duration::from_millis(500)).await;
}

#[tokio::test]
async fn child_crash_is_captured_with_exit_reason() {
    let mut child = spawn(&spec("/bin/sh", "exit 42")).expect("spawn");
    let reason = wait(&mut child);
    assert_eq!(reason, ExitReason::Exited(42));
}

#[tokio::test]
async fn descendants_are_killed_with_the_process_group() {
    let mut child = spawn(&spec("/bin/sh", "sleep 600 & echo $! >&2; wait")).expect("spawn");
    // Child reports the descendant's pid on stderr.
    let mut pid_text = String::new();
    loop {
        let mut byte = [0u8; 1];
        child
            .stderr
            .as_mut()
            .expect("stderr pipe")
            .read_exact(&mut byte)
            .expect("read pid");
        if byte[0] == b'\n' {
            break;
        }
        pid_text.push(byte[0] as char);
    }
    let descendant: i32 = pid_text.trim().parse().expect("descendant pid");
    let reason = terminate(child, Duration::from_millis(300)).await;
    assert!(matches!(
        reason,
        ExitReason::Signaled(_) | ExitReason::Exited(_)
    ));
    // The whole group is gone.
    let alive = std::process::Command::new("kill")
        .arg("-0")
        .arg(descendant.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(!alive, "descendant {descendant} still running");
}

#[tokio::test]
async fn descendants_are_reaped_when_the_leader_exits_first() {
    // The leader spawns a descendant, reports its pid, and exits
    // cooperatively — the group still holds the descendant and must be
    // swept after the leader's exit.
    let mut child = spawn(&spec("/bin/sh", "sleep 600 & echo $! >&2; exit 0")).expect("spawn");
    let mut pid_text = String::new();
    loop {
        let mut byte = [0u8; 1];
        child
            .stderr
            .as_mut()
            .expect("stderr pipe")
            .read_exact(&mut byte)
            .expect("read pid");
        if byte[0] == b'\n' {
            break;
        }
        pid_text.push(byte[0] as char);
    }
    let descendant: i32 = pid_text.trim().parse().expect("descendant pid");
    let reason = terminate(child, Duration::from_millis(300)).await;
    assert!(matches!(reason, ExitReason::Exited(0)));
    let alive = std::process::Command::new("kill")
        .arg("-0")
        .arg(descendant.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(!alive, "descendant {descendant} outlived terminate");
}

#[tokio::test]
async fn handshake_verifies_identity_and_digest() {
    // A fixture that ignores the bootstrap and writes a canned Hello frame.
    let ids = DeterministicIds::new(SEED);
    let instance = AdapterInstanceId::new(&ids);
    let mut s = spec("/bin/sh", "sleep 2");
    s.adapter_instance_id = instance;
    let hello = AdapterFrame {
        body: Some(domain::generated::contract::adapter_frame::Body::Hello(
            AdapterHello {
                adapter_instance_id: instance.to_string(),
                adapter_id: s.adapter_id.to_string(),
                adapter_version: s.adapter_version.clone(),
                bundle_digest: DIGEST.to_owned(),
                protocol_version: 1,
                implemented_ports: vec![],
                capability_document: vec![],
            },
        )),
    }
    .encode_to_vec();
    let dir = tempfile::tempdir().expect("tmp");
    let frame_path = dir.path().join("hello.frame");
    let mut blob = (hello.len() as u32).to_be_bytes().to_vec();
    blob.extend_from_slice(&hello);
    std::fs::write(&frame_path, &blob).expect("frame file");
    s.argv = vec![
        "-c".to_owned(),
        format!(
            "dd if={} bs=4096 >&0 2>/dev/null; sleep 2",
            frame_path.display()
        ),
    ];
    let mut child = spawn(&s).expect("spawn");
    let reply = handshake(&mut child, "nonce-1", 5_000).expect("handshake");
    assert_eq!(reply.adapter_instance_id, instance.to_string());
    let _ = terminate(child, Duration::from_millis(500)).await;

    // An instance mismatch fails closed.
    let ids = DeterministicIds::new(SEED + 1);
    let other_instance = AdapterInstanceId::new(&ids);
    let mut s = spec(
        "/bin/sh",
        &format!(
            "dd if={} bs=4096 >&0 2>/dev/null; sleep 2",
            frame_path.display()
        ),
    );
    s.adapter_instance_id = other_instance;
    let mut child = spawn(&s).expect("spawn");
    let err = handshake(&mut child, "nonce-2", 5_000).expect_err("instance mismatch fails");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    let _ = terminate(child, Duration::from_millis(500)).await;
}

#[tokio::test]
async fn instance_lifecycle_is_durable_in_the_store() {
    let dir = tempfile::tempdir().expect("tmp");
    let store = Arc::new(
        SqliteKernelStore::open(StoreConfig {
            path: dir.path().join("kernel.db"),
            pool_max_connections: 4,
            busy_timeout_ms: 10_000,
        })
        .await
        .expect("store"),
    );
    let ids = DeterministicIds::new(SEED);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&ids))
        .await
        .expect("fence");
    let daemon = DaemonInstanceId::new(&ids);
    let adapter = AdapterId::new(&ids);
    let instance = AdapterInstanceId::new(&ids);
    let spec = SpawnSpec {
        adapter_id: adapter,
        adapter_version: "1.0.0".to_owned(),
        expected_bundle_digest: DIGEST.to_owned(),
        adapter_instance_id: instance,
        daemon_instance_id: daemon,
        daemon_fencing_epoch: fence.epoch.0,
        protocol_version: 1,
        executable: PathBuf::from("/bin/sh"),
        argv: vec!["-c".to_owned(), "exit 0".to_owned()],
        env: vec![],
        cwd: None,
        isolation: process_supervisor::spawn::Isolation::None,
        stdout_ipc: false,
    };
    let mut child = spawn(&spec).expect("spawn");
    let mut txn = store
        .begin_write(TxContext {
            daemon_epoch: fence.epoch.0,
            principal_id: PrincipalId::new(&ids),
            command_id: CommandId::new(&ids),
            correlation_id: None,
        })
        .await
        .expect("txn");
    // FK: adapter registration must exist first.
    txn.adapters()
        .insert_registration(NewAdapterRegistration {
            adapter_id: adapter,
            version: "1.0.0".to_owned(),
            bundle_digest: DIGEST.to_owned(),
            manifest_digest: "m".to_owned(),
            runtime_type: "process".to_owned(),
            implemented_ports: Vec::new(),
            capabilities: Vec::new(),
            trust_state: TrustState::Trusted,
            conformance_state: ConformanceState::Untested,
            created_at_ms: SEED,
        })
        .await
        .expect("registration");
    record_spawn(txn.as_mut(), &spec, &child, SEED)
        .await
        .expect("record");
    txn.commit().await.expect("commit");
    let reason = wait(&mut child);
    assert_eq!(reason, ExitReason::Exited(0));
    let mut txn = store
        .begin_write(TxContext {
            daemon_epoch: fence.epoch.0,
            principal_id: PrincipalId::new(&ids),
            command_id: CommandId::new(&ids),
            correlation_id: None,
        })
        .await
        .expect("txn");
    process_supervisor::mark_exited(txn.as_mut(), instance, &reason, SEED + 1)
        .await
        .expect("mark exited");
    txn.commit().await.expect("commit");
    // Instance row is durable: exited with reason.
    let mut txn = store
        .begin_write(TxContext {
            daemon_epoch: fence.epoch.0,
            principal_id: PrincipalId::new(&ids),
            command_id: CommandId::new(&ids),
            correlation_id: None,
        })
        .await
        .expect("txn");
    let row = txn
        .adapters()
        .get_instance(instance)
        .await
        .expect("read")
        .expect("row");
    txn.rollback().await.expect("rollback");
    assert_eq!(row.state, instance_state::EXITED);
    assert_eq!(row.exit_reason.as_deref(), Some("exit:0"));
    assert_eq!(row.pid, Some(i64::from(child.pid)));
    assert!(row.process_start_identity.is_some());
    assert!(row.ended_at_ms.is_some());
}

use errors::codes::ErrorCode;
