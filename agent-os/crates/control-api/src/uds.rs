//! Unix-domain listener bound under the daemon lock, at `0600`.
//!
//! The socket lives at `<runtime_dir>/control.sock`. It is bound only while
//! the caller holds the daemon singleton lock, and a stale socket file is
//! removed solely after the lock proves no live daemon owns it — a second
//! daemon that lost the lock race can never steal or unlink the socket
//! (control-api.md).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use tokio::net::{UnixListener, UnixStream};

/// Name of the control-plane socket inside the runtime directory.
pub const SOCKET_FILE_NAME: &str = "control.sock";

/// A bound control-plane socket; unlinking on drop keeps restarts clean.
#[derive(Debug)]
pub struct ControlSocket {
    listener: UnixListener,
    path: PathBuf,
}

impl ControlSocket {
    /// Binds the socket, removing a stale file only after probing it: an
    /// answerable endpoint means another daemon holds the socket and we fail
    /// instead of stealing it. `daemon_lock` is proof the caller holds the
    /// singleton lock — pass `&agentd::lock::DaemonLock`; the type is kept
    /// generic here to keep `control-api` free of `agentd` dependencies.
    pub fn bind(runtime_dir: &Path, daemon_lock: &impl std::fmt::Debug) -> errors::Result<Self> {
        let _lock_is_held = daemon_lock; // the lock's existence is the proof
        let path = runtime_dir.join(SOCKET_FILE_NAME);
        fs::create_dir_all(runtime_dir).map_err(|e| io("create runtime dir", e))?;
        // The runtime dir is owner-only, so the socket is unreachable by
        // other users even in the instant before its own chmod lands.
        fs::set_permissions(runtime_dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| io("chmod runtime dir", e))?;
        if path.exists() {
            // Lock is held, so no *live* daemon owns the socket — but a live
            // endpoint could still be a squatting process: probe before
            // unlinking.
            if std::os::unix::net::UnixStream::connect(&path).is_ok() {
                return Err(KernelError::new(
                    ErrorCode::FailedPrecondition,
                    RetryClass::Never,
                    format!(
                        "control socket {} is owned by a live endpoint",
                        path.display()
                    ),
                ));
            }
            fs::remove_file(&path).map_err(|e| io("remove stale control socket", e))?;
        }
        // Restrictive permission: the parent dir is 0700, and the socket is
        // chmod 0600 immediately after bind.
        let listener = std::os::unix::net::UnixListener::bind(&path)
            .map_err(|e| io("bind control socket", e))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|e| io("chmod control socket", e))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| io("nonblocking", e))?;
        Ok(Self {
            listener: UnixListener::from_std(listener).map_err(|e| io("listener", e))?,
            path,
        })
    }

    /// Filesystem path of the bound socket.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Accepts the next local connection.
    pub async fn accept(&self) -> errors::Result<UnixStream> {
        self.listener
            .accept()
            .await
            .map(|(stream, _)| stream)
            .map_err(|e| io("accept", e))
    }
}

impl Drop for ControlSocket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn io(op: &str, source: std::io::Error) -> KernelError {
    KernelError::new(
        ErrorCode::Unavailable,
        RetryClass::Safe,
        format!("{op} failed"),
    )
    .with_source(source)
}
