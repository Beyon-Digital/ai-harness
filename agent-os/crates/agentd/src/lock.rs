//! Daemon singleton lock management.
//!
//! The lock is an OS advisory exclusive lock on `<runtime_dir>/agentd.lock`,
//! created with mode `0600`. Holding a [`DaemonLock`] is the local
//! single-writer gate (R4.1); the OS releases the lock when the process exits,
//! so a crashed daemon leaves no stale lock for the next start (R4.6, D7).
//!
//! The composition root does not consume this module yet, so the binary
//! target would otherwise report the staged surface as dead code; the
//! two-process test `tests/lock_exclusion.rs` is the live consumer.
#![allow(dead_code)]

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Name of the lock file inside the runtime directory.
pub const LOCK_FILE_NAME: &str = "agentd.lock";

/// Failure to acquire the daemon singleton lock.
#[derive(Debug)]
pub enum LockError {
    /// Another handle or process already holds the lock.
    Held {
        /// Path of the lock file that is held.
        path: PathBuf,
    },
    /// The lock file could not be opened or locked.
    Io(io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held { path } => write!(formatter, "daemon lock is held: {}", path.display()),
            Self::Io(source) => write!(formatter, "daemon lock io error: {source}"),
        }
    }
}

impl std::error::Error for LockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Held { .. } => None,
            Self::Io(source) => Some(source),
        }
    }
}

/// Exclusive OS lock proving this process owns the daemon role.
#[derive(Debug)]
pub struct DaemonLock {
    file: File,
    path: PathBuf,
}

impl DaemonLock {
    /// Acquires the exclusive lock on `<runtime_dir>/agentd.lock`.
    ///
    /// Returns [`LockError::Held`] when another handle or process holds the
    /// lock; the caller refuses to become authoritative rather than waiting.
    pub fn acquire(runtime_dir: &Path) -> Result<Self, LockError> {
        let path = runtime_dir.join(LOCK_FILE_NAME);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)
            .map_err(LockError::Io)?;
        file.try_lock().map_err(|error| match error {
            TryLockError::WouldBlock => LockError::Held { path: path.clone() },
            TryLockError::Error(source) => LockError::Io(source),
        })?;
        Ok(Self { file, path })
    }

    /// Returns the path of the held lock file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Releases the lock by unlocking and closing the lock file.
    pub fn release(self) {
        let _ = self.file.unlock();
    }
}
