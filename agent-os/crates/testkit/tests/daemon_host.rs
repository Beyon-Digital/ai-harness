//! Integration tests for the fixture daemon and `TempDaemonHost` (R17.1, R17.3, N2).

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use testkit::process::TempDaemonHost;

const FIXTURE: &str = env!("CARGO_BIN_EXE_fixture-daemon");

#[test]
fn test_temp_daemon_exits_cleanly() {
    let status = Command::new(FIXTURE)
        .args(["--exit-after-ms", "0"])
        .status();
    match status {
        Ok(status) => assert!(status.success(), "fixture exited with {status}"),
        Err(error) => panic!("failed to spawn fixture daemon: {error}"),
    }
}

#[test]
fn test_temp_daemon_rejects_invalid_exit_after() {
    let status = Command::new(FIXTURE)
        .args(["--exit-after-ms", "not-a-number"])
        .status();
    match status {
        Ok(status) => assert!(
            !status.success(),
            "a bad --exit-after-ms value must fail loudly"
        ),
        Err(error) => panic!("failed to spawn fixture daemon: {error}"),
    }
}

#[test]
fn test_temp_daemon_host_reaps_on_drop() {
    let mut host = match TempDaemonHost::new(Path::new(FIXTURE)) {
        Ok(host) => host,
        Err(error) => panic!("failed to start the daemon host: {error}"),
    };
    assert!(host.home().is_dir(), "home directory must exist");
    assert!(host.runtime_dir().is_dir(), "runtime directory must exist");
    assert_ne!(
        host.home(),
        host.runtime_dir(),
        "home and runtime dirs must be separate"
    );

    let timeout = host.wait_for_exit(Duration::from_millis(20));
    assert!(
        timeout.is_err(),
        "a parked fixture daemon must not exit on its own"
    );

    let home = host.home().to_path_buf();
    let runtime = host.runtime_dir().to_path_buf();
    drop(host);
    assert!(!home.exists(), "temporary home must be removed on drop");
    assert!(
        !runtime.exists(),
        "temporary runtime dir must be removed on drop"
    );
}

#[test]
fn test_killed_daemon_is_reaped() {
    let mut child = match Command::new(FIXTURE).spawn() {
        Ok(child) => child,
        Err(error) => panic!("failed to spawn fixture daemon: {error}"),
    };
    match child.kill() {
        Ok(()) => {}
        Err(error) => panic!("failed to kill fixture daemon: {error}"),
    }
    let status = match child.wait() {
        Ok(status) => status,
        Err(error) => panic!("failed to reap fixture daemon: {error}"),
    };
    assert!(
        !status.success(),
        "a killed daemon must not report a clean exit"
    );
}
