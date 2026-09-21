//! T0 local-process adapter (SBOX-001).
//!
//! Trusted local execution: explicit cwd, an environment **allowlist**
//! (the child starts from an empty environment — nothing leaks by
//! inheritance), a deadline, and a cancellation signal. stdout/stderr are
//! captured with a byte cap.
//!
//! T0 is **not a security boundary**: no filesystem, network, or
//! resource isolation is claimed. `SANDBOX_CAPABILITIES` reflects that so
//! resolved environments never carry a false security claim.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use rustix::process::{Pid, Signal, kill_process_group};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// The T0 adapter's honest capability declaration.
pub const CAPABILITIES: &[&str] = &[
    "sandbox.exec",
    "sandbox.env_allowlist",
    "sandbox.deadline",
    "sandbox.capture_output",
];

fn sandbox_error(code: ErrorCode, msg: impl Into<String>) -> KernelError {
    KernelError::new(code, RetryClass::Never, msg.into())
}

/// What to run and under which constraints.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecSpec {
    /// Program to execute (resolved on the child's PATH, which is only
    /// what `env` provides).
    pub program: String,
    /// Arguments.
    pub args: Vec<String>,
    /// Working directory — typically a workspace root the caller holds a
    /// lease on (the manager verifies the lease before calling this).
    pub cwd: PathBuf,
    /// Exact environment passed to the child — the complete allowlist,
    /// already filtered to broker-approved material by the caller.
    pub env: Vec<(String, String)>,
    /// Max wall-clock execution time.
    pub deadline: Duration,
    /// Byte cap applied to each of stdout/stderr (truncated at the cap).
    pub output_cap_bytes: usize,
}

/// How the exec ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExecStatus {
    /// Child exited on its own.
    Exited(i32),
    /// Deadline expired; the child was killed.
    TimedOut,
    /// Cancellation requested; the child was killed.
    Cancelled,
    /// Child was terminated by a signal without cancel/timeout.
    Signaled,
}

/// Captured result of a T0 exec.
#[derive(Clone, Debug)]
pub struct ExecOutcome {
    /// How the child ended.
    pub status: ExecStatus,
    /// Captured stdout (capped at `output_cap_bytes`).
    pub stdout: Vec<u8>,
    /// Captured stderr (capped at `output_cap_bytes`).
    pub stderr: Vec<u8>,
    /// Wall-clock duration.
    pub duration: Duration,
    /// True when either stream was truncated at the cap.
    pub output_truncated: bool,
}

/// Execute `spec` as a trusted local process. `cancel` resolves to
/// request termination — the child is killed and `ExecStatus::Cancelled`
/// reported. Never fails open: spawn errors are `Unavailable`.
pub async fn exec(
    spec: &ExecSpec,
    mut cancel: tokio::sync::watch::Receiver<bool>,
) -> errors::Result<ExecOutcome> {
    let mut child = Command::new(&spec.program)
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        // Own process group — termination signals the whole tree so a
        // `sh -c` child's grandchildren can't hold the pipes open.
        .process_group(0)
        .spawn()
        .map_err(|e| {
            sandbox_error(
                ErrorCode::Unavailable,
                format!("sandbox exec spawn failed: {e}"),
            )
        })?;

    let started = Instant::now();
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| sandbox_error(ErrorCode::Internal, "stdout pipe missing"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| sandbox_error(ErrorCode::Internal, "stderr pipe missing"))?;
    let cap = spec.output_cap_bytes;

    let read_out = tokio::spawn(async move { read_capped(&mut stdout, cap).await });
    let read_err = tokio::spawn(async move { read_capped(&mut stderr, cap).await });

    // Poll `try_wait` at a fine interval so deadline/cancel can `kill()`
    // the child directly — no second task owns the handle.
    const POLL: Duration = Duration::from_millis(10);
    let deadline = tokio::time::Instant::now() + spec.deadline;
    let pid = child.id();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => {
                break match s.code() {
                    Some(code) => ExecStatus::Exited(code),
                    None => ExecStatus::Signaled,
                };
            }
            Ok(None) => {}
            Err(e) => {
                return Err(sandbox_error(
                    ErrorCode::Unavailable,
                    format!("sandbox wait failed: {e}"),
                ));
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(POLL) => {}
            _ = tokio::time::sleep_until(deadline) => {
                kill_group(pid);
                let _ = child.wait().await;
                break ExecStatus::TimedOut;
            }
            _ = wait_cancel(&mut cancel) => {
                kill_group(pid);
                let _ = child.wait().await;
                break ExecStatus::Cancelled;
            }
        }
    };

    let stdout = read_out
        .await
        .map_err(|e| sandbox_error(ErrorCode::Internal, format!("stdout reader failed: {e}")))??;
    let stderr = read_err
        .await
        .map_err(|e| sandbox_error(ErrorCode::Internal, format!("stderr reader failed: {e}")))??;

    Ok(ExecOutcome {
        status,
        stdout: stdout.0,
        stderr: stderr.0,
        duration: started.elapsed(),
        output_truncated: stdout.1 || stderr.1,
    })
}

/// Resolves only when the cancel token flips `true`; a dropped sender
/// (caller lost interest or never arms cancel) pends forever instead of
/// masquerading as a cancellation.
/// SIGKILL the exec'd process group (children are group leaders via
/// `process_group(0)`), reaping any descendant that outlives the leader.
fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid.and_then(|p| Pid::from_raw(p.try_into().unwrap_or(i32::MAX))) {
        let _ = kill_process_group(pid, Signal::KILL);
    }
}

async fn wait_cancel(cancel: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if cancel.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
        if *cancel.borrow() {
            return;
        }
    }
}

async fn read_capped<R>(reader: &mut R, cap: usize) -> errors::Result<(Vec<u8>, bool)>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(cap.min(65_536));
    let mut chunk = [0u8; 16 * 1024];
    let mut truncated = false;
    loop {
        let n = reader
            .read(&mut chunk)
            .await
            .map_err(|e| sandbox_error(ErrorCode::Unavailable, format!("pipe read failed: {e}")))?;
        if n == 0 {
            return Ok((buf, truncated));
        }
        if buf.len() + n > cap {
            // Keep draining — a blocked writer would stall the child's
            // exit otherwise; only the retained prefix is capped.
            buf.extend_from_slice(&chunk[..cap - buf.len()]);
            truncated = true;
            continue;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}
