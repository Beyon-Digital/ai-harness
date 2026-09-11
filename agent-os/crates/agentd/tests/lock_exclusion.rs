//! Two-process exclusion for the daemon singleton lock (R4.1, R4.6, N2).
//!
//! The test re-executes its own test binary. The child acquires the lock,
//! prints a readiness line, and then waits on stdin while the parent proves a
//! second acquire is refused and that the lock holder still responds. The
//! parent then kills the child and proves the OS released the lock, so the
//! next start can claim it. All synchronization is blocking I/O on pipes; the
//! test never sleeps.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[path = "../src/lock.rs"]
mod lock;

use lock::{DaemonLock, LOCK_FILE_NAME, LockError};

const CHILD_FLAG_ENV: &str = "AGENTD_LOCK_TEST_CHILD";
const CHILD_DIR_ENV: &str = "AGENTD_LOCK_TEST_DIR";
const READY_LINE: &str = "LOCK_READY";
const PING: &str = "ping";
const PONG_LINE: &str = "LOCK_PONG";
const DB_FILE: &str = "kernel.db";

fn child_main() -> ! {
    let dir = std::env::var_os(CHILD_DIR_ENV).expect("child runtime dir is set");
    let lock = DaemonLock::acquire(std::path::Path::new(&dir)).expect("child acquires the lock");
    println!("{READY_LINE}");
    std::io::stdout().flush().expect("child flushes readiness");

    let stdin = std::io::stdin();
    loop {
        let mut line = String::new();
        let read = stdin.read_line(&mut line).expect("child reads stdin");
        if read == 0 {
            break;
        }
        if line.trim() == PING {
            println!("{PONG_LINE}");
            std::io::stdout().flush().expect("child flushes pong");
        }
    }
    drop(lock);
    std::process::exit(0);
}

fn unique_runtime_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("agentd-lock-test-{}-{nanos}", std::process::id()))
}

fn wait_for_line(reader: &mut impl BufRead, expected: &str) -> String {
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).unwrap();
        assert!(read > 0, "child closed stdout before printing {expected}");
        let trimmed = line.trim();
        if trimmed == expected {
            return trimmed.to_owned();
        }
    }
}

#[test]
fn second_process_is_refused_and_kill_releases_the_lock() {
    if std::env::var_os(CHILD_FLAG_ENV).is_some() {
        child_main();
    }

    let dir = unique_runtime_dir();
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
    let db_path = dir.join(DB_FILE);
    fs::write(&db_path, b"sentinel database bytes").unwrap();

    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--nocapture")
        .env(CHILD_FLAG_ENV, "1")
        .env(CHILD_DIR_ENV, &dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    assert_eq!(wait_for_line(&mut stdout, READY_LINE), READY_LINE);

    let lock_path = dir.join(LOCK_FILE_NAME);
    match DaemonLock::acquire(&dir) {
        Err(LockError::Held { path }) => assert_eq!(path, lock_path),
        Err(LockError::Io(source)) => panic!("second acquire failed with io error: {source}"),
        Ok(_) => panic!("second process acquired the held daemon lock"),
    }

    assert_eq!(
        fs::read(&db_path).unwrap(),
        b"sentinel database bytes",
        "refused process touched the database"
    );
    assert_eq!(
        fs::metadata(&lock_path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    child.stdin.as_mut().unwrap().write_all(b"ping\n").unwrap();
    child.stdin.as_mut().unwrap().flush().unwrap();
    assert_eq!(wait_for_line(&mut stdout, PONG_LINE), PONG_LINE);
    assert!(
        child.try_wait().unwrap().is_none(),
        "lock holder exited early"
    );

    child.kill().unwrap();
    child.wait().unwrap();

    let acquired = DaemonLock::acquire(&dir).expect("killed holder's lock is released");
    assert_eq!(acquired.path(), lock_path.as_path());
    acquired.release();
    DaemonLock::acquire(&dir).expect("released lock is immediately acquirable");

    let _ = fs::remove_dir_all(&dir);
}
