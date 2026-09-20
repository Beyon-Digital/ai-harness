//! Process spawn: private socketpair IPC + identity capture.
//!
//! The parent creates an `AF_UNIX SOCK_STREAM` socketpair, then maps the
//! child end onto the child's **fd 0** via `Stdio::from` — a fixed inherited
//! descriptor the runtime guarantees, so no raw-fd juggling is needed (the
//! spec's "e.g. 3" is illustrative; fd 0 is equally fixed and keeps
//! `stdout`/`stderr` as independent pipes). The child never listens on a
//! discoverable socket, so an unrelated local process cannot connect.
#![forbid(unsafe_code)]

use std::io::{Read, Write};
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child as OsChild, ChildStderr, ChildStdout, Command, Stdio};

use domain::generated::contract::{AdapterBootstrap, AdapterFrame, AdapterHello, AdapterShutdown};
use domain::ids::{AdapterId, AdapterInstanceId, DaemonInstanceId};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use prost::Message;
use rustix::process::{Pid, Signal, kill_process_group};

/// Maximum adapter frame: `adapters.max_frame_bytes` (limits.yaml).
const MAX_FRAME_BYTES: usize = 4_194_304;

/// Everything needed to launch one supervised adapter process.
#[derive(Debug)]
pub struct SpawnSpec {
    /// Registered adapter identity.
    pub adapter_id: AdapterId,
    /// Registered adapter version.
    pub adapter_version: String,
    /// Content digest the spawned binary is expected to have.
    pub expected_bundle_digest: String,
    /// Fresh adapter-instance identity for this process.
    pub adapter_instance_id: AdapterInstanceId,
    /// Owning daemon instance.
    pub daemon_instance_id: DaemonInstanceId,
    /// Daemon fencing epoch associated with the instance.
    pub daemon_fencing_epoch: u64,
    /// Protocol version offered in the bootstrap.
    pub protocol_version: u32,
    /// Executable from the installed bundle.
    pub executable: PathBuf,
    /// Argument vector (argv[0] excluded).
    pub argv: Vec<String>,
    /// Minimal environment allowlist; everything else is cleared.
    pub env: Vec<(String, String)>,
    /// Working directory for the child.
    pub cwd: Option<PathBuf>,
}

/// A spawned adapter child: its OS handle, identity, and parent end of the
/// private IPC socketpair.
pub struct Child {
    /// OS process id.
    pub pid: u32,
    /// `pid:start_identity` — start identity disambiguates PID reuse.
    pub start_identity: String,
    /// Instance identity captured at spawn.
    pub instance: AdapterInstanceId,
    /// Expected bundle digest.
    pub bundle_digest: String,
    /// Daemon fencing epoch this process belongs to.
    pub daemon_epoch: u64,
    process: OsChild,
    ipc: UnixStream,
    /// Owning daemon instance.
    daemon_instance_id: DaemonInstanceId,
    /// Protocol version offered in bootstrap.
    protocol_version: u32,
    /// Separate capture pipe.
    pub stdout: ChildStdout,
    /// Separate capture pipe.
    pub stderr: ChildStderr,
}

impl Child {
    /// Parent end of the private IPC socket.
    pub fn ipc(&mut self) -> &mut UnixStream {
        &mut self.ipc
    }

    /// Disjoint mutable access to all three channels.
    pub fn channels(&mut self) -> (&mut UnixStream, &mut ChildStdout, &mut ChildStderr) {
        (&mut self.ipc, &mut self.stdout, &mut self.stderr)
    }
}

/// Process exit reason, normalized.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitReason {
    /// Clean exit with a code.
    Exited(i32),
    /// Terminated by a signal.
    Signaled(i32),
    /// Exit could not be classified.
    Undetermined,
}

impl std::fmt::Display for ExitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exited(code) => write!(f, "exit:{code}"),
            Self::Signaled(sig) => write!(f, "signal:{sig}"),
            Self::Undetermined => f.write_str("undetermined"),
        }
    }
}

/// Spawns the adapter process with the private IPC socket on its fd 0.
///
/// stdout and stderr are independent pipes; every other descriptor the
/// child inherits is the runtime's (inheritable descriptors are cleared by
/// the launcher as non-CLOEXEC only where set).
pub fn spawn(spec: &SpawnSpec) -> errors::Result<Child> {
    let (parent_end, child_end) = UnixStream::pair().map_err(|e| io("socketpair", e))?;
    let mut command = Command::new(&spec.executable);
    command
        .args(&spec.argv)
        .env_clear()
        .envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        // Private channel: the child's fd 0 is its socketpair end.
        .stdin(Stdio::from(OwnedFd::from(child_end)))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Own process group, so descendant cleanup can signal the group.
        .process_group(0);
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    let mut process = command.spawn().map_err(|e| io("spawn", e))?;
    let stdout = process.stdout.take().expect("piped stdout");
    let stderr = process.stderr.take().expect("piped stderr");
    let pid = process.id();
    Ok(Child {
        pid,
        start_identity: process_start_identity(pid),
        instance: spec.adapter_instance_id,
        bundle_digest: spec.expected_bundle_digest.clone(),
        daemon_epoch: spec.daemon_fencing_epoch,
        daemon_instance_id: spec.daemon_instance_id,
        protocol_version: spec.protocol_version,
        process,
        ipc: parent_end,
        stdout,
        stderr,
    })
}

/// Sends `AdapterBootstrap` and verifies the child's `AdapterHello`.
///
/// The handshake pins the instance id, adapter identity, bundle digest, and
/// protocol version — a mismatch fails closed before the endpoint is
/// registered. `timeout_ms` bounds both socket reads.
pub fn handshake(
    child: &mut Child,
    bootstrap_nonce: &str,
    timeout_ms: u64,
) -> errors::Result<AdapterHello> {
    let bootstrap = AdapterFrame {
        body: Some(domain::generated::contract::adapter_frame::Body::Bootstrap(
            AdapterBootstrap {
                daemon_instance_id: child.daemon_instance_id.to_string(),
                daemon_fencing_epoch: child.daemon_epoch,
                adapter_instance_id: child.instance.to_string(),
                expected_bundle_digest: child.bundle_digest.clone(),
                bootstrap_nonce: bootstrap_nonce.to_owned(),
                protocol_version: child.protocol_version,
            },
        )),
    };
    child
        .ipc
        .set_read_timeout(Some(std::time::Duration::from_millis(timeout_ms)))
        .map_err(|e| io("set_read_timeout", e))?;
    write_frame(&mut child.ipc, &bootstrap)?;
    let reply = read_frame(&mut child.ipc)?;
    let Some(domain::generated::contract::adapter_frame::Body::Hello(hello)) = reply.body else {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            "adapter replied to bootstrap with a non-hello frame",
        ));
    };
    if hello.adapter_instance_id != child.instance.to_string() {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            "adapter hello carries a different instance id",
        ));
    }
    if hello.bundle_digest != child.bundle_digest {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            "adapter hello digest differs from the registered bundle",
        ));
    }
    Ok(hello)
}

/// Graceful shutdown then kill escalation: an `AdapterShutdown` frame gives
/// the child `grace` to exit; a still-running process group is then
/// SIGTERMed and finally SIGKILLed, taking descendants with it (the child
/// is its own process-group leader). Returns the exit reason.
pub async fn terminate(mut child: Child, grace: std::time::Duration) -> ExitReason {
    let _ = write_frame(
        &mut child.ipc,
        &AdapterFrame {
            body: Some(domain::generated::contract::adapter_frame::Body::Shutdown(
                AdapterShutdown {
                    reason: "daemon drain".to_owned(),
                },
            )),
        },
    );
    let deadline = std::time::Instant::now() + grace;
    loop {
        if let Ok(Some(_status)) = child.process.try_wait() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            signal_group(child.pid, Signal::TERM);
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            if child.process.try_wait().ok().flatten().is_none() {
                signal_group(child.pid, Signal::KILL);
            }
            let _ = child.process.wait();
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    exit_reason(&mut child.process)
}

/// Signals the process group led by `pid` (children are group leaders, so
/// the group takes every descendant).
fn signal_group(pid: u32, sig: Signal) {
    if let Some(pid) = Pid::from_raw(pid.try_into().unwrap_or(i32::MAX)) {
        let _ = kill_process_group(pid, sig);
    }
}

/// Wait for exit, mapping the status to an [`ExitReason`].
pub fn wait(child: &mut Child) -> ExitReason {
    let _ = child.process.wait();
    exit_reason(&mut child.process)
}

fn exit_reason(process: &mut OsChild) -> ExitReason {
    match process.try_wait() {
        Ok(Some(status)) => status.code().map_or_else(
            || {
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    status
                        .signal()
                        .map_or(ExitReason::Undetermined, ExitReason::Signaled)
                }
                #[cfg(not(unix))]
                {
                    ExitReason::Undetermined
                }
            },
            ExitReason::Exited,
        ),
        _ => ExitReason::Undetermined,
    }
}

/// `pid:<start-time>` on Linux (procfs), `pid:<spawn-seq>` elsewhere — start
/// identity makes a reused PID distinguishable.
fn process_start_identity(pid: u32) -> String {
    #[cfg(target_os = "linux")]
    if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        && let Some(start) = stat.rsplit(')').next()
        && let Some(ticks) = start.split_whitespace().nth(20)
    {
        return format!("{pid}:{ticks}");
    }
    format!("{pid}:{}", std::process::id())
}

/// u32-BE-length-prefixed `AdapterFrame` on the socketpair.
fn write_frame(stream: &mut UnixStream, frame: &AdapterFrame) -> errors::Result<()> {
    let body = frame.encode_to_vec();
    let len: u32 = body
        .len()
        .try_into()
        .map_err(|_| io("frame length", std::io::Error::other("frame too large")))?;
    stream
        .write_all(&len.to_be_bytes())
        .and_then(|()| stream.write_all(&body))
        .and_then(|()| stream.flush())
        .map_err(|e| io("frame write", e))
}

fn read_frame(stream: &mut UnixStream) -> errors::Result<AdapterFrame> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .map_err(|e| io("frame header", e))?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(KernelError::new(
            ErrorCode::ResourceExhausted,
            RetryClass::Never,
            "adapter frame exceeds max_frame_bytes",
        ));
    }
    let mut body = vec![0u8; len];
    stream
        .read_exact(&mut body)
        .map_err(|e| io("frame body", e))?;
    AdapterFrame::decode(body.as_slice()).map_err(|_| {
        KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            "adapter frame is not a valid AdapterFrame message",
        )
    })
}

fn io(op: &'static str, e: std::io::Error) -> KernelError {
    KernelError::new(
        ErrorCode::Unavailable,
        RetryClass::Safe,
        format!("process supervisor {op} failed: {e}"),
    )
}
