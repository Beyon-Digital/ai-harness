//! Agent OS desktop shell (Tauri v2).
//!
//! The app is self-contained: it ships `agentd`, `agentgw`, the adapter
//! bundles and configs as sidecars/resources, spawns the services on
//! launch, and navigates the window to the gateway once it answers —
//! double-click is all a user needs.
//!
//! `AGENTOS_GATEWAY` overrides everything: when set, no services are
//! spawned and the window loads that URL instead (local or remote
//! gateway). Platforms without bundled sidecars (e.g. Windows, where the
//! CLI is Unix-only) also fall back to that remote-gateway mode.

use std::{
    env, fs,
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use tauri::{Manager, RunEvent, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const DEFAULT_GATEWAY: &str = "http://127.0.0.1:7740";
const PREFERRED_PORT: u16 = 7740;
const BOOT_TIMEOUT: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(100);

/// Origin that bundled `frontendDist` pages are served from. Query
/// strings survive `WebviewWindow::navigate`, so the error page reads
/// its message from `location.search`.
#[cfg(not(windows))]
const TAURI_ORIGIN: &str = "tauri://localhost";
#[cfg(windows)]
const TAURI_ORIGIN: &str = "http://tauri.localhost";

static CHILDREN: Mutex<Vec<Child>> = Mutex::new(Vec::new());
static BOOTSTRAP: Mutex<Option<JoinHandle<()>>> = Mutex::new(None);
/// Set on app exit so a mid-flight bootstrap stops before spawning more.
static CANCEL: AtomicBool = AtomicBool::new(false);

fn cancelled() -> bool {
    CANCEL.load(Ordering::Relaxed)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .setup(|app| {
            let window = open_window(app.handle())?;
            match env::var("AGENTOS_GATEWAY")
                .ok()
                .filter(|v| !v.trim().is_empty())
            {
                Some(gateway) => match gateway.parse::<Url>() {
                    Ok(url) => navigate(&window, url),
                    Err(e) => fail(
                        &window,
                        &format!("invalid AGENTOS_GATEWAY `{gateway}`: {e}"),
                    ),
                },
                None => {
                    let handle = app.handle().clone();
                    let join = thread::spawn(move || match bootstrap(&handle) {
                        Ok(url) => navigate(&window, url),
                        Err(e) => {
                            kill_children();
                            fail(&window, &e);
                        }
                    });
                    *BOOTSTRAP.lock().expect("bootstrap") = Some(join);
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("agent-os desktop failed to start");
    app.run(|_, event| {
        if let RunEvent::Exit = event {
            // Join bootstrap before teardown so a child can't be spawned
            // after CHILDREN is drained.
            CANCEL.store(true, Ordering::Relaxed);
            if let Some(join) = BOOTSTRAP.lock().expect("bootstrap").take() {
                let _ = join.join();
            }
            kill_children();
        }
    });
}

fn open_window(handle: &tauri::AppHandle) -> Result<WebviewWindow, String> {
    WebviewWindowBuilder::new(handle, "main", WebviewUrl::App(PathBuf::from("index.html")))
        .title("Agent OS")
        .inner_size(1280.0, 840.0)
        .min_inner_size(900.0, 600.0)
        .build()
        .map_err(|e| format!("{e}"))
}

fn navigate(window: &WebviewWindow, url: Url) {
    if let Err(e) = window.navigate(url) {
        eprintln!("agent-os: navigate failed: {e}");
    }
}

fn fail(window: &WebviewWindow, msg: &str) {
    eprintln!("agent-os: {msg}");
    let _ = window.set_title("Agent OS — startup failed");
    if let Ok(url) = Url::parse(&format!("{TAURI_ORIGIN}/error.html?msg={}", urlencode(msg))) {
        navigate(window, url);
    }
}

/// Spawn the embedded services and return the gateway URL once it is
/// answering. Sidecars absent → remote-gateway fallback URL.
fn bootstrap(handle: &tauri::AppHandle) -> Result<Url, String> {
    let resource_dir = handle
        .path()
        .resource_dir()
        .map_err(|e| format!("resource dir: {e}"))?;
    let staged_src = resource_dir.join("agentos");
    // A packaged app ships `agentos/share` — its absence means an
    // unpackaged build (Windows, dev shell) where remote-gateway mode
    // is the intended path.
    let packaged = staged_src.join("share").is_dir();

    let (agentd, agentgw) = match (find_sidecar("agentd"), find_sidecar("agentgw")) {
        (Some(a), Some(g)) => (a, g),
        _ if packaged => {
            return Err(
                "embedded services missing from the app bundle — reinstall the app".to_owned(),
            )
        }
        _ => {
            return DEFAULT_GATEWAY.parse().map_err(|e| format!("{e}"));
        }
    };
    if !packaged {
        return Err(format!(
            "embedded runtime not staged (missing {})",
            staged_src.display()
        ));
    }
    let data_dir = handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    let embedded = data_dir.join("embedded");
    copy_tree(&staged_src, &embedded)?;
    let staged_share = embedded.join("share");
    chmod_entrypoints(&staged_share.join("bundles"));

    let runtime_dir = data_dir.join("run");
    fs::create_dir_all(&runtime_dir).map_err(|e| format!("runtime dir: {e}"))?;
    let socket = runtime_dir.join("control.sock");
    let logs = data_dir.join("logs");
    let _ = fs::create_dir_all(&logs);

    // The daemon lock is a singleton per runtime dir: when a first app
    // instance (or a manually started agentd) already answers, attach a
    // gateway to it instead of spawning a second agentd that exits on
    // the lock. The socket connect proves a live daemon — a stale file
    // alone doesn't.
    if daemon_ready(&socket) {
        eprintln!(
            "agent-os: attaching to running daemon at {}",
            socket.display()
        );
    } else {
        let config = pick_config(&staged_share)?;
        let bundles = bundle_dirs(&staged_share.join("bundles"))?;
        let mut agentd_cmd = Command::new(&agentd);
        agentd_cmd
            .arg("--runtime-dir")
            .arg(&runtime_dir)
            .arg("--config")
            .arg(&config);
        for b in &bundles {
            agentd_cmd.arg("--adapter-bundle").arg(b);
        }
        let daemon = spawn(&mut agentd_cmd, &logs.join("agentd.log"))?;
        wait_for_daemon(&socket, daemon)?;
    }

    if cancelled() {
        return Err("startup cancelled".to_owned());
    }
    let port = pick_port();
    let mut agentgw_cmd = Command::new(&agentgw);
    agentgw_cmd
        .arg("--socket")
        .arg(&socket)
        .arg("--listen")
        .arg(format!("127.0.0.1:{port}"));
    spawn(&mut agentgw_cmd, &logs.join("agentgw.log"))?;

    wait_for_tcp(port)?;
    format!("http://127.0.0.1:{port}/")
        .parse()
        .map_err(|e| format!("{e}"))
}

/// Gateway reachable once TCP connects — `/api/health` backs it but a
/// connect is enough to prove the listener is up.
fn wait_for_tcp(port: u16) -> Result<(), String> {
    let deadline = Instant::now() + BOOT_TIMEOUT;
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().expect("addr");
    while Instant::now() < deadline {
        if cancelled() {
            return Err("startup cancelled".to_owned());
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok() {
            return Ok(());
        }
        thread::sleep(POLL);
    }
    Err(format!(
        "gateway did not open port {port} within {BOOT_TIMEOUT:?} — see logs/agentgw.log in the app data dir"
    ))
}

/// True when a daemon already answers on the control socket — a stale
/// socket file left by a crashed run doesn't count.
#[cfg(unix)]
fn daemon_ready(socket: &Path) -> bool {
    socket.is_file() && std::os::unix::net::UnixStream::connect(socket).is_ok()
}

#[cfg(not(unix))]
fn daemon_ready(socket: &Path) -> bool {
    socket.is_file()
}

/// Poll until the daemon's control socket answers; fails fast if the
/// spawned agentd exits first (e.g. it lost the daemon lock).
fn wait_for_daemon(socket: &Path, child: usize) -> Result<(), String> {
    let deadline = Instant::now() + BOOT_TIMEOUT;
    while Instant::now() < deadline {
        if cancelled() {
            return Err("startup cancelled".to_owned());
        }
        {
            let mut children = CHILDREN.lock().expect("children");
            if !matches!(children[child].try_wait(), Ok(None)) {
                return Err(
                    "agentd exited during startup — see logs/agentd.log in the app data dir"
                        .to_owned(),
                );
            }
        }
        if daemon_ready(socket) {
            return Ok(());
        }
        thread::sleep(POLL);
    }
    Err(format!(
        "daemon did not create {} within {BOOT_TIMEOUT:?} — see logs/agentd.log in the app data dir",
        socket.display()
    ))
}

fn pick_port() -> u16 {
    for candidate in [PREFERRED_PORT, 0] {
        if let Ok(l) = TcpListener::bind(("127.0.0.1", candidate)) {
            if let Ok(addr) = l.local_addr() {
                return addr.port();
            }
        }
    }
    PREFERRED_PORT
}

/// `AGENTOS_CONFIG` selects a config file from the bundled share dir;
/// otherwise real-LLM config when an API key is present, else the
/// deterministic fixture profile.
fn pick_config(share: &Path) -> Result<PathBuf, String> {
    let name = env::var("AGENTOS_CONFIG")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            env::var("OPENROUTER_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(|_| "openrouter.yaml".to_owned())
        })
        .unwrap_or_else(|| "default.yaml".to_owned());
    let path = share.join(&name);
    if !path.is_file() {
        return Err(format!("config {} not found", path.display()));
    }
    Ok(path)
}

fn bundle_dirs(bundles: &Path) -> Result<Vec<PathBuf>, String> {
    let mut dirs = Vec::new();
    let entries = fs::read_dir(bundles).map_err(|e| format!("bundles dir: {e}"))?;
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            dirs.push(p);
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// Spawn `cmd`, log its output, register it for teardown, and return
/// its CHILDREN index so callers can watch for early exits.
fn spawn(cmd: &mut Command, log_path: &Path) -> Result<usize, String> {
    let prog = cmd.get_program().to_string_lossy().into_owned();
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .map_err(|e| format!("log {}: {e}", log_path.display()))?;
    let log_err = log.try_clone().map_err(|e| format!("{e}"))?;
    let child = cmd
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .map_err(|e| format!("spawn {prog}: {e}"))?;
    let mut children = CHILDREN.lock().expect("children");
    children.push(child);
    Ok(children.len() - 1)
}

fn kill_children() {
    // Spawn order is agentd then agentgw; reverse so the gateway stops
    // first and the daemon can drain its adapter children last.
    let mut children: Vec<Child> = CHILDREN.lock().expect("children").drain(..).collect();
    for child in children.iter_mut().rev() {
        stop_child(child);
    }
}

/// SIGTERM first — agentd handles it by draining adapter children and
/// completing shutdown; escalate to SIGKILL past the grace window.
fn stop_child(child: &mut Child) {
    if !matches!(child.try_wait(), Ok(None)) {
        return;
    }
    #[cfg(unix)]
    {
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(None) => thread::sleep(Duration::from_millis(50)),
                _ => return,
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Sidecars sit next to the app executable, named `<name>` or
/// `<name>-<target-triple>` (plus `.exe` on Windows).
fn find_sidecar(name: &str) -> Option<PathBuf> {
    let exe_dir = env::current_exe().ok()?.parent()?.to_path_buf();
    let direct = exe_dir.join(name);
    if direct.is_file() {
        return Some(direct);
    }
    let prefixed = format!("{name}-");
    let entries = fs::read_dir(&exe_dir).ok()?;
    for entry in entries.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        let stem = file.strip_suffix(".exe").unwrap_or(&file);
        if stem == name || stem.starts_with(&prefixed) {
            return Some(entry.path());
        }
    }
    None
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        fs::remove_dir_all(dst).map_err(|e| format!("reset {}: {e}", dst.display()))?;
    }
    copy_tree_inner(src, dst)
}

fn copy_tree_inner(src: &Path, dst: &Path) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| format!("mkdir {}: {e}", dst.display()))?;
    let entries = fs::read_dir(src).map_err(|e| format!("read {}: {e}", src.display()))?;
    for entry in entries.flatten() {
        let (s, d) = (entry.path(), dst.join(entry.file_name()));
        if s.is_dir() {
            copy_tree_inner(&s, &d)?;
        } else {
            fs::copy(&s, &d).map_err(|e| format!("copy {}: {e}", s.display()))?;
        }
    }
    Ok(())
}

/// Adapter bundles contain an executable named by the manifest's
/// `entrypoint`; file copies don't always carry the exec bit.
#[cfg(unix)]
fn chmod_entrypoints(bundles: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(entries) = fs::read_dir(bundles) else {
        return;
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        let manifest = dir.join("adapter.manifest.json");
        let Ok(text) = fs::read_to_string(&manifest) else {
            continue;
        };
        if let Some(name) = entrypoint_of(&text) {
            let bin = dir.join(name);
            if bin.is_file() {
                let _ = fs::set_permissions(&bin, fs::Permissions::from_mode(0o755));
            }
        }
    }
}

#[cfg(not(unix))]
fn chmod_entrypoints(_bundles: &Path) {}

/// Pull `"entrypoint": "<file>"` out of the manifest without a JSON dep.
fn entrypoint_of(manifest: &str) -> Option<String> {
    let idx = manifest.find("\"entrypoint\"")?;
    let after = &manifest[idx + "\"entrypoint\"".len()..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
