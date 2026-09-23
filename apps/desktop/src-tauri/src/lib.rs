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
//!
//! Every knob the services read has a built-in default; the app also
//! loads `agentos.env` from the app data dir (auto-created on first
//! launch) so non-developer users can set e.g. `OPENROUTER_API_KEY`
//! without a terminal. Precedence: process env > agentos.env > default.
//!
//! When a service fails to come up, the error page shows the tail of its
//! log inline (no hunting hidden directories) plus Retry — which re-reads
//! `agentos.env`, so editing the file and clicking Retry is enough — and
//! an Open-logs button that reveals the logs directory in the platform
//! file manager.

use std::{
    collections::BTreeMap,
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
/// `agentos.env` from the app data dir — loaded in setup and re-read on
/// every Retry, so a user can fix the file and retry without relaunching.
static ENV_FILE: Mutex<BTreeMap<String, String>> = Mutex::new(BTreeMap::new());

/// Defaults applied to every spawned service when neither the process
/// env nor `agentos.env` sets the key.
const DEFAULT_ENV: &[(&str, &str)] = &[
    ("RUST_LOG", "info"),
    ("OPENROUTER_MODEL", "openrouter/free"),
    ("OPENROUTER_APP_NAME", "Agent OS"),
];

/// Template written to `<app_data>/agentos.env` on first launch so the
/// customization surface is discoverable without docs.
const ENV_TEMPLATE: &str = "\
# Agent OS environment — KEY=VALUE per line, # comments, optional `export ` prefix.
# Real environment variables always win over values here.
# No quoting or escapes — the value is everything after the first `=`.
# (Inline `#` does NOT start a comment; put comments on their own line.)
#
# Real-LLM mode: set your OpenRouter key and the app auto-switches to the
# openrouter.yaml profile (model calls flow through as durable effects).
# OPENROUTER_API_KEY=
# OPENROUTER_MODEL=openrouter/free
# OPENROUTER_BASE_URL=https://openrouter.ai/api/v1
#
# RUST_LOG=info
#
# Connect to an existing gateway instead of running embedded services:
# AGENTOS_GATEWAY=http://127.0.0.1:7740
# Pick a different bundled config yaml from share/ (default.yaml unless
# OPENROUTER_API_KEY is set → openrouter.yaml):
# AGENTOS_CONFIG=default.yaml
#
# MCP tool servers (JSON array) for the mcp-enabled profile:
# MCP_SERVERS=[]
# ACP agent CLI for the acp-local profile:
# ACP_COMMAND=
# ACP_ARGS=
# ACP_CWD=
";

fn cancelled() -> bool {
    CANCEL.load(Ordering::Relaxed)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        // The daemon is a singleton — a second launch just focuses the
        // running instance's window instead of competing for it.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .invoke_handler(tauri::generate_handler![retry_startup, open_logs])
        .setup(|app| {
            if let Ok(dir) = app.path().app_data_dir() {
                let _ = fs::create_dir_all(&dir);
                *ENV_FILE.lock().expect("env file") = load_env_file(&dir.join("agentos.env"));
            }
            let window = open_window(app.handle())?;
            start_services(app.handle(), window);
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

/// The startup path shared by first launch and the error page's Retry:
/// honor `AGENTOS_GATEWAY` when set, else spawn the embedded services on
/// a bootstrap thread and navigate once the gateway answers.
fn start_services(handle: &tauri::AppHandle, window: WebviewWindow) {
    match app_var("AGENTOS_GATEWAY") {
        Some(gateway) => match gateway.parse::<Url>() {
            Ok(url) => navigate(&window, url),
            Err(e) => fail(
                &window,
                &format!("invalid AGENTOS_GATEWAY `{gateway}`: {e}"),
            ),
        },
        None => {
            // Skip when a bootstrap is already in flight (rapid Retries).
            let mut slot = BOOTSTRAP.lock().expect("bootstrap");
            if slot.as_ref().is_some_and(|j| !j.is_finished()) {
                return;
            }
            let h = handle.clone();
            let join = thread::spawn(move || match bootstrap(&h) {
                Ok(url) => navigate(&window, url),
                Err(e) => {
                    kill_children();
                    fail(&window, &e);
                }
            });
            *slot = Some(join);
        }
    }
}

/// Error-page Retry: re-read `agentos.env` (edits apply without an app
/// relaunch) and rerun the whole startup path.
#[tauri::command]
fn retry_startup(window: WebviewWindow) {
    if let Ok(dir) = window.app_handle().path().app_data_dir() {
        *ENV_FILE.lock().expect("env file") = load_env_file(&dir.join("agentos.env"));
    }
    let handle = window.app_handle().clone();
    start_services(&handle, window);
}

/// Opens the service logs directory in the platform file manager — the
/// error page's escape hatch for users who want the full log.
#[tauri::command]
fn open_logs(app: tauri::AppHandle) -> Result<(), String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?
        .join("logs");
    fs::create_dir_all(&dir).map_err(|e| format!("logs dir: {e}"))?;
    open_dir(&dir)
}

#[cfg(target_os = "macos")]
const OPENER: &str = "open";
#[cfg(target_os = "windows")]
const OPENER: &str = "explorer";
#[cfg(all(unix, not(target_os = "macos")))]
const OPENER: &str = "xdg-open";

fn open_dir(dir: &Path) -> Result<(), String> {
    let status = Command::new(OPENER)
        .arg(dir)
        .status()
        .map_err(|e| format!("{OPENER}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{OPENER} exited with {status}"))
    }
}

fn navigate(window: &WebviewWindow, url: Url) {
    if let Err(e) = window.navigate(url) {
        eprintln!("agent-os: navigate failed: {e}");
    }
}

fn fail(window: &WebviewWindow, msg: &str) {
    eprintln!("agent-os: {msg}");
    let _ = window.set_title("Agent OS — startup failed");
    let mut url = format!("{TAURI_ORIGIN}/error.html?msg={}", urlencode(msg));
    // The page echoes the path verbatim so non-technical users don't have
    // to know where the platform's app-data dir lives.
    if let Ok(dir) = window.app_handle().path().app_data_dir() {
        url.push_str(&format!("&dir={}", urlencode(&dir.display().to_string())));
    }
    if let Ok(url) = Url::parse(&url) {
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
    // Pin the wasm host path — adapter_registry falls back to the
    // sibling-of-agentd lookup, but explicit never depends on order.
    if let Some(host) = find_sidecar("agentos-wasm-host") {
        if env::var_os("AGENTOS_WASM_HOST").is_none() {
            env::set_var("AGENTOS_WASM_HOST", &host);
        }
    }
    let data_dir = handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("app data dir: {e}"))?;
    let runtime_dir = data_dir.join("run");
    fs::create_dir_all(&runtime_dir).map_err(|e| format!("runtime dir: {e}"))?;
    let socket = runtime_dir.join("control.sock");
    let logs = data_dir.join("logs");
    let _ = fs::create_dir_all(&logs);

    // The daemon lock is a singleton per runtime dir: when a first app
    // instance (or a manually started agentd) already answers, attach a
    // gateway to it instead of spawning a second agentd that exits on
    // the lock. The socket connect proves a live daemon — a stale file
    // alone doesn't. Check before any mutation: a surviving daemon
    // references the staged bundle paths below.
    if daemon_ready(&socket) {
        eprintln!(
            "agent-os: attaching to running daemon at {}",
            socket.display()
        );
    } else {
        // No live daemon — rewriting the staged copies is safe.
        let embedded = data_dir.join("embedded");
        copy_tree(&staged_src, &embedded)?;
        let staged_share = embedded.join("share");
        chmod_entrypoints(&staged_share.join("bundles"));

        let config = pick_config(&staged_share)?;
        let bundles = bundle_dirs(&staged_share.join("bundles"))?;
        let mut agentd_cmd = Command::new(&agentd);
        agentd_cmd
            .arg("--runtime-dir")
            .arg(&runtime_dir)
            .arg("--config")
            .arg(&config)
            .envs(child_env());
        for b in &bundles {
            agentd_cmd.arg("--adapter-bundle").arg(b);
        }
        let agentd_log = logs.join("agentd.log");
        // A service that exits before answering gets one retry — a flaky
        // first spawn shouldn't hard-fail the whole launch.
        for attempt in 0..=1 {
            let log_from = log_len(&agentd_log);
            let daemon = spawn(&mut agentd_cmd, &agentd_log)?;
            match wait_for_daemon(&socket, daemon) {
                ServiceWait::Ready => break,
                ServiceWait::Exited if attempt == 0 => {
                    eprintln!("agent-os: agentd exited during startup — retrying once");
                }
                ServiceWait::Exited => {
                    return Err(format!(
                        "agentd exited during startup{}",
                        log_tail_msg(&agentd_log, log_from)
                    ));
                }
                ServiceWait::TimedOut => {
                    return Err(format!(
                        "daemon did not create {} within {BOOT_TIMEOUT:?}{}",
                        socket.display(),
                        log_tail_msg(&agentd_log, log_from)
                    ));
                }
                ServiceWait::Cancelled => return Err("startup cancelled".to_owned()),
            }
        }
    }

    if cancelled() {
        return Err("startup cancelled".to_owned());
    }
    let port = pick_port();
    let agentgw_log = logs.join("agentgw.log");
    for attempt in 0..=1 {
        let mut agentgw_cmd = Command::new(&agentgw);
        agentgw_cmd
            .arg("--socket")
            .arg(&socket)
            .arg("--listen")
            .arg(format!("127.0.0.1:{port}"))
            .envs(child_env());
        let log_from = log_len(&agentgw_log);
        let gateway = spawn(&mut agentgw_cmd, &agentgw_log)?;
        match wait_for_tcp(port, gateway) {
            ServiceWait::Ready => break,
            ServiceWait::Exited if attempt == 0 => {
                eprintln!("agent-os: agentgw exited during startup — retrying once");
            }
            ServiceWait::Exited => {
                return Err(format!(
                    "agentgw exited during startup{}",
                    log_tail_msg(&agentgw_log, log_from)
                ));
            }
            ServiceWait::TimedOut => {
                return Err(format!(
                    "gateway did not open port {port} within {BOOT_TIMEOUT:?}{}",
                    log_tail_msg(&agentgw_log, log_from)
                ));
            }
            ServiceWait::Cancelled => return Err("startup cancelled".to_owned()),
        }
    }
    format!("http://127.0.0.1:{port}/")
        .parse()
        .map_err(|e| format!("{e}"))
}

/// How a startup poll ended for a spawned service.
enum ServiceWait {
    /// The service answers.
    Ready,
    /// The spawned process exited before answering.
    Exited,
    /// BOOT_TIMEOUT elapsed.
    TimedOut,
    /// App teardown interrupted the wait.
    Cancelled,
}

/// Gateway reachable once TCP connects — `/api/health` backs it but a
/// connect is enough to prove the listener is up. The child index is
/// watched too, so a dead gateway fails fast instead of timing out.
fn wait_for_tcp(port: u16, child: usize) -> ServiceWait {
    let deadline = Instant::now() + BOOT_TIMEOUT;
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().expect("addr");
    while Instant::now() < deadline {
        if cancelled() {
            return ServiceWait::Cancelled;
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok() {
            return ServiceWait::Ready;
        }
        {
            let mut children = CHILDREN.lock().expect("children");
            if !matches!(children[child].try_wait(), Ok(None)) {
                return ServiceWait::Exited;
            }
        }
        thread::sleep(POLL);
    }
    ServiceWait::TimedOut
}

/// True when a daemon already answers on the control socket — a stale
/// socket file left by a crashed run doesn't count.
#[cfg(unix)]
fn daemon_ready(socket: &Path) -> bool {
    // is_file() excludes S_IFSOCK — connect alone covers missing/stale.
    std::os::unix::net::UnixStream::connect(socket).is_ok()
}

#[cfg(not(unix))]
fn daemon_ready(socket: &Path) -> bool {
    socket.is_file()
}

/// Poll until the daemon's control socket answers; reports `Exited` the
/// moment the spawned agentd dies (e.g. it lost the daemon lock).
fn wait_for_daemon(socket: &Path, child: usize) -> ServiceWait {
    let deadline = Instant::now() + BOOT_TIMEOUT;
    while Instant::now() < deadline {
        if cancelled() {
            return ServiceWait::Cancelled;
        }
        // A live socket wins over a dead child: on simultaneous launches
        // the loser's agentd exits on the lock while the winner's is
        // already answering — attach instead of reporting failure.
        if daemon_ready(socket) {
            return ServiceWait::Ready;
        }
        {
            let mut children = CHILDREN.lock().expect("children");
            if !matches!(children[child].try_wait(), Ok(None)) {
                return ServiceWait::Exited;
            }
        }
        thread::sleep(POLL);
    }
    ServiceWait::TimedOut
}

/// File length captured just before a spawn — `log_tail` only reports
/// bytes appended after this point, so a process that dies silently
/// can't inherit an earlier run's output as its reported failure.
fn log_len(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// The last lines written to a service log by *this* spawn — ANSI-
/// stripped, appended to a startup failure so the error page shows *why*
/// instead of only where to look. The read is bounded: logs are append-
/// only across launches, so it seeks near the end rather than loading a
/// file that grew over the app's lifetime.
fn log_tail(path: &Path, from: u64) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    const LINES: usize = 20;
    const BYTES: usize = 4000;
    // Well past the BYTES cap applied after line selection, to cover
    // over-long lines.
    const WINDOW: u64 = (BYTES as u64) * 4;
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    if len <= from {
        return None;
    }
    let start = from.max(len.saturating_sub(WINDOW));
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).ok()?;
    let text = strip_ansi(&String::from_utf8_lossy(&raw));
    // A window starting mid-file begins mid-line — drop the partial.
    let text = if start > from {
        text.split_once('\n')
            .map(|(_, rest)| rest)
            .unwrap_or(text.as_str())
    } else {
        text.as_str()
    };
    let tail: Vec<&str> = text.lines().rev().take(LINES).collect();
    if tail.is_empty() {
        return None;
    }
    let mut out = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    if out.len() > BYTES {
        let mut idx = out.len() - BYTES;
        while !out.is_char_boundary(idx) {
            idx += 1;
        }
        out = out[idx..].to_owned();
    }
    Some(out)
}

/// Formats the log tail for an error message, or a pointer to the file
/// when nothing was captured (process died before writing anything).
fn log_tail_msg(log: &Path, from: u64) -> String {
    match log_tail(log, from) {
        Some(tail) => format!("\n\n{tail}"),
        None => " — the service wrote no log output".to_owned(),
    }
}

/// Removes ANSI SGR color sequences so a log tail stays readable when
/// embedded in the error page.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && matches!(chars.peek(), Some('[')) {
            chars.next();
            for inner in chars.by_ref() {
                if inner.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Read `agentos.env` (creating it from the template when absent).
/// Format: `KEY=VALUE` lines, `#` comments, optional `export ` prefix.
fn load_env_file(path: &Path) -> BTreeMap<String, String> {
    if !path.exists() {
        // May hold an API key — create owner-only.
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            if let Ok(mut f) = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(path)
            {
                use std::io::Write;
                let _ = f.write_all(ENV_TEMPLATE.as_bytes());
            }
        }
        #[cfg(not(unix))]
        {
            let _ = fs::write(path, ENV_TEMPLATE);
        }
    }
    let Ok(text) = fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_owned(), v.trim().to_owned());
        }
    }
    map
}

/// Env lookup for app-level switches — process env wins, `agentos.env`
/// supplies the value when the var isn't set in the environment.
fn app_var(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            ENV_FILE
                .lock()
                .ok()
                .and_then(|m| m.get(key).cloned())
                .filter(|v| !v.trim().is_empty())
        })
}

/// (key, value) pairs to inject into service commands: everything from
/// `agentos.env` plus the DEFAULT_ENV table, skipping keys already set
/// in the process env (which children inherit anyway).
fn child_env() -> Vec<(String, String)> {
    let file = ENV_FILE.lock().expect("env file");
    let mut out = Vec::new();
    for (k, v) in file.iter() {
        if env::var_os(k).is_none() {
            out.push((k.clone(), v.clone()));
        }
    }
    for (k, v) in DEFAULT_ENV {
        let k = (*k).to_owned();
        if env::var_os(&k).is_none() && !file.contains_key(&k) {
            out.push((k, (*v).to_owned()));
        }
    }
    out
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
    let name = app_var("AGENTOS_CONFIG")
        .or_else(|| app_var("OPENROUTER_API_KEY").map(|_| "openrouter.yaml".to_owned()))
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

/// Bundled configs give agentd a 30s drain deadline
/// (`limits.shutdown.drain_deadline_ms`) — allow it plus teardown.
const DRAIN_GRACE: Duration = Duration::from_secs(35);

fn kill_children() {
    // Signal every child before waiting: agentd begins draining while
    // the gateway exits — serial SIGTERM-then-wait would stack the
    // grace windows.
    let mut children: Vec<Child> = CHILDREN.lock().expect("children").drain(..).collect();
    for child in children.iter_mut().rev() {
        terminate(child);
    }
    let deadline = Instant::now() + DRAIN_GRACE;
    while Instant::now() < deadline
        && children
            .iter_mut()
            .any(|c| matches!(c.try_wait(), Ok(None)))
    {
        thread::sleep(Duration::from_millis(50));
    }
    for child in children.iter_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// SIGTERM when still running — agentd handles it by draining adapter
/// children and completing shutdown; escalation happens in kill_children.
#[cfg(unix)]
fn terminate(child: &mut Child) {
    if matches!(child.try_wait(), Ok(None)) {
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    }
}

#[cfg(not(unix))]
fn terminate(child: &mut Child) {
    let _ = child.kill();
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
