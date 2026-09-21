//! Process spawn: private socketpair IPC + identity capture.
//!
//! The parent creates an `AF_UNIX SOCK_STREAM` socketpair, then maps the
//! child end onto the child's **fd 0** via `Stdio::from` — a fixed inherited
//! descriptor the runtime guarantees, so no raw-fd juggling is needed (the
//! spec's "e.g. 3" is illustrative; fd 0 is equally fixed and keeps
//! `stdout`/`stderr` as independent pipes). The child never listens on a
//! discoverable socket, so an unrelated local process cannot connect.
#![forbid(unsafe_code)]

use std::io::Read;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child as OsChild, ChildStderr, ChildStdout, Command, Stdio};

use adapter_protocol::ExpectedIdentity;
use adapter_protocol::framing::{read_frame, write_frame};
use adapter_protocol::handshake::{SessionPhase, accept_inbound};
use domain::generated::contract::{
    AdapterFrame, AdapterHello, AdapterShutdown, adapter_frame::Body,
};
use domain::ids::{AdapterId, AdapterInstanceId, DaemonInstanceId};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use rustix::process::{Pid, Signal, kill_process_group};

/// Kernel-level isolation applied to the child (T1 sandbox tier).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Isolation {
    /// Plain supervised process (T0).
    #[default]
    None,
    /// Linux user namespace via `unshare`: the adapter runs as fake root
    /// in fresh user/IPC namespaces; `network: false` also unshares the
    /// net namespace (no sockets). Falls back to [`Isolation::None`] when
    /// `unshare` is unavailable or userns creation is denied.
    UserNamespace {
        /// Whether the child keeps the parent's network namespace.
        network: bool,
    },
}

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
    /// Requested sandbox isolation.
    pub isolation: Isolation,
    /// Map the IPC socketpair onto the child's **stdout** as well as
    /// stdin — wasm bundles spawn `agentos-wasm-host`, whose guest module
    /// uses WASI stdio as the bidirectional protocol channel.
    pub stdout_ipc: bool,
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
    /// Registered adapter id the handshake pins.
    adapter_id: AdapterId,
    /// Registered adapter version the handshake pins.
    adapter_version: String,
    /// Owning daemon instance.
    daemon_instance_id: DaemonInstanceId,
    /// Protocol version offered in bootstrap.
    protocol_version: u32,
    /// Separate capture pipe. Either consume it (see [`Child::channels`]) or
    /// hand it to [`Child::drain_output`] — an unread pipe deadlocks the
    /// child once the kernel buffer fills. `None` once drained.
    pub stdout: Option<ChildStdout>,
    /// Separate capture pipe. Same drain requirement as [`Child::stdout`].
    pub stderr: Option<ChildStderr>,
}

/// Reads one pipe to EOF and discards the bytes, so a noisy child never
/// blocks on a full kernel buffer.
fn drain<R: Read + Send + 'static>(mut pipe: R) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
    })
}

impl Child {
    /// Parent end of the private IPC socket.
    pub fn ipc(&mut self) -> &mut UnixStream {
        &mut self.ipc
    }

    /// Disjoint mutable access to all three channels.
    pub fn channels(
        &mut self,
    ) -> (
        &mut UnixStream,
        Option<&mut ChildStdout>,
        Option<&mut ChildStderr>,
    ) {
        (&mut self.ipc, self.stdout.as_mut(), self.stderr.as_mut())
    }

    /// Detaches stdout/stderr onto drain threads that read them to EOF.
    ///
    /// Callers that never read the capture pipes (the daemon path) must call
    /// this right after spawn: the adapter protocol carries no output on
    /// those descriptors, and a full pipe buffer would otherwise block the
    /// child's next `write` mid-call — stalling IPC behind a writer that can
    /// never proceed. The drain threads exit on EOF when the child dies or
    /// the pipes close.
    pub fn drain_output(&mut self) {
        if let Some(pipe) = self.stdout.take() {
            drain(pipe);
        }
        if let Some(pipe) = self.stderr.take() {
            drain(pipe);
        }
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
    let mut command = match spec.isolation {
        Isolation::None => {
            let mut c = Command::new(&spec.executable);
            c.args(&spec.argv);
            c
        }
        Isolation::UserNamespace { network } => {
            // Preflight: `Command::spawn` only proves `unshare` launched,
            // not that unshare(2) succeeded inside it — a denied userns
            // would surface later as a silent EOF at handshake. Probe the
            // same flags up front; failure means the sandbox is
            // unavailable and the spawn must fail closed — a T1 adapter
            // never runs with T0 access.
            let mut probe = Command::new("unshare");
            probe.args(["--user", "--map-root-user", "--ipc"]);
            if !network {
                probe.arg("--net");
            }
            let available = probe
                .arg("true")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !available {
                return Err(KernelError::new(
                    ErrorCode::FailedPrecondition,
                    RetryClass::Never,
                    "user-namespace isolation unavailable on this host",
                ));
            }
            // `unshare --user --map-root-user` needs no privilege: the
            // child becomes root inside a fresh userns only.
            let mut c = Command::new("unshare");
            c.args(["--user", "--map-root-user", "--ipc", "--kill-child"]);
            if !network {
                c.arg("--net");
            }
            c.arg(&spec.executable).args(&spec.argv);
            c
        }
    };
    // Wasm host: guest stdout must ALSO land on the child's socketpair
    // end — clone it before stdin consumes `child_end`.
    let child_stdout = if spec.stdout_ipc {
        Some(OwnedFd::from(
            child_end
                .try_clone()
                .map_err(|e| io("clone socketpair end", e))?,
        ))
    } else {
        None
    };
    command
        .env_clear()
        .envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        // Private channel: the child's fd 0 is its socketpair end.
        .stdin(Stdio::from(OwnedFd::from(child_end)))
        .stderr(Stdio::piped())
        // Own process group, so descendant cleanup can signal the group.
        .process_group(0);
    if let Some(fd) = child_stdout {
        command.stdout(Stdio::from(fd));
    } else {
        command.stdout(Stdio::piped());
    }
    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    let mut process = command.spawn().map_err(|e| io("spawn", e))?;
    let stdout = if spec.stdout_ipc {
        None
    } else {
        process.stdout.take()
    };
    let stderr = process.stderr.take().expect("piped stderr");
    let pid = process.id();
    Ok(Child {
        pid,
        start_identity: process_start_identity(pid),
        instance: spec.adapter_instance_id,
        bundle_digest: spec.expected_bundle_digest.clone(),
        daemon_epoch: spec.daemon_fencing_epoch,
        adapter_id: spec.adapter_id,
        adapter_version: spec.adapter_version.clone(),
        daemon_instance_id: spec.daemon_instance_id,
        protocol_version: spec.protocol_version,
        process,
        ipc: parent_end,
        stdout,
        stderr: Some(stderr),
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
    let expected = ExpectedIdentity {
        daemon_instance_id: child.daemon_instance_id,
        daemon_fencing_epoch: child.daemon_epoch,
        adapter_instance_id: child.instance.to_string(),
        adapter_id: child.adapter_id.to_string(),
        adapter_version: child.adapter_version.clone(),
        expected_bundle_digest: child.bundle_digest.clone(),
        protocol_version: child.protocol_version,
    };
    child
        .ipc
        .set_read_timeout(Some(std::time::Duration::from_millis(timeout_ms)))
        .map_err(|e| io("set_read_timeout", e))?;
    write_frame(&mut child.ipc, &expected.bootstrap_frame(bootstrap_nonce))?;
    let Some(reply) = read_frame(&mut child.ipc)? else {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            "adapter closed the channel instead of replying to bootstrap",
        ));
    };
    accept_inbound(SessionPhase::AwaitingHello, &reply)?;
    let Some(Body::Hello(hello)) = reply.body else {
        unreachable!("AwaitingHello only admits Hello");
    };
    expected.verify_hello(&hello, bootstrap_nonce)?;
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
            body: Some(Body::Shutdown(AdapterShutdown {
                reason: "daemon drain".to_owned(),
            })),
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

fn io(op: &'static str, e: std::io::Error) -> KernelError {
    KernelError::new(
        ErrorCode::Unavailable,
        RetryClass::Safe,
        format!("process supervisor {op} failed: {e}"),
    )
}
