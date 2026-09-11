//! Temporary daemon host harness for smoke tests.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

/// How long the wait loop parks between `try_wait` polls.
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// A spawned daemon binary running against an isolated temporary tree.
///
/// The child receives `AGENTD_HOME`, `AGENTD_RUNTIME_DIR`, and `AGENTD_SOCKET`
/// pointing into the temporary tree, with stdout and stderr redirected to log
/// files in the same tree. Dropping the host kills and reaps the child and
/// removes the tree.
#[derive(Debug)]
pub struct TempDaemonHost {
    temp: TempDir,
    home: PathBuf,
    runtime_dir: PathBuf,
    socket: PathBuf,
    child: Child,
}

impl TempDaemonHost {
    /// Spawns `daemon_bin` against a fresh temporary tree.
    pub fn new(daemon_bin: &Path) -> io::Result<Self> {
        let temp = TempDir::new()?;
        let home = temp.path().join("home");
        let runtime_dir = temp.path().join("runtime");
        let socket = runtime_dir.join("agentd.sock");
        std::fs::create_dir_all(&home)?;
        std::fs::create_dir_all(&runtime_dir)?;
        let stdout = File::create(temp.path().join("daemon.stdout.log"))?;
        let stderr = File::create(temp.path().join("daemon.stderr.log"))?;

        let child = Command::new(daemon_bin)
            .env("AGENTD_HOME", &home)
            .env("AGENTD_RUNTIME_DIR", &runtime_dir)
            .env("AGENTD_SOCKET", &socket)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()?;

        Ok(Self {
            temp,
            home,
            runtime_dir,
            socket,
            child,
        })
    }

    /// Returns the daemon home directory inside the temporary tree.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Returns the daemon runtime directory inside the temporary tree.
    pub fn runtime_dir(&self) -> &Path {
        &self.runtime_dir
    }

    /// Returns the socket path inside the temporary runtime directory.
    pub fn socket_path(&self) -> &Path {
        &self.socket
    }

    /// Waits up to `timeout` for the child to exit, reaping it on success.
    ///
    /// Returns [`io::ErrorKind::TimedOut`] when the child is still running
    /// after the deadline.
    ///
    /// A timeout too large to represent as an [`Instant`] has no deadline and
    /// is treated as unbounded rather than as an immediate timeout.
    pub fn wait_for_exit(&mut self, timeout: Duration) -> io::Result<ExitStatus> {
        let deadline = Instant::now().checked_add(timeout);
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(status);
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "daemon did not exit before the timeout",
                ));
            }
            thread::park_timeout(POLL_INTERVAL);
        }
    }
}

impl Drop for TempDaemonHost {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(self.temp.path());
    }
}
