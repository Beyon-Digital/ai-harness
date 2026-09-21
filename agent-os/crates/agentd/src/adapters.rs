//! `agentd adapter <sub>` — Phase-13 extension-bundle lifecycle.
//!
//! Installed bundles live under `<runtime_dir>/installed-adapters/<dir_name>/`
//! with a JSON index (`index.json`) recording identity, enable state, and the
//! latest spawn-check result. `boot` registers every *enabled* installed
//! bundle in addition to the config-supplied `adapter_bundles`, so
//! `install` → restart → the adapter is registered and spawnable.
//!
//! Lifecycle: `install` (verify + copy + optional spawn-check), `check`
//! (spawn smoke: bootstrap/hello/ping on the real IPC channel), `enable` /
//! `disable` (whether boot registers it — downgrade = install old bundle and
//! disable the newer one), `remove` (delete bundle + index entry),
//! `list` (print the index).

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use adapter_protocol::framing::{read_frame, write_frame};
use adapter_protocol::handshake::ExpectedIdentity;
use adapter_registry::manifest::{
    ExtensionManifest, LOCK_FILE, MANIFEST_FILE, manifest_digest, parse_manifest,
};
use adapter_registry::registry::compute_bundle_digest;
use domain::generated::contract::{AdapterFrame, AdapterPing, adapter_frame::Body};
use domain::ids::{AdapterId, AdapterInstanceId, DaemonInstanceId};
use domain::provider::{IdProvider, SystemIdProvider};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use process_supervisor::{self, SpawnSpec};

/// Directory under the runtime dir holding installed adapter bundles.
pub const INSTALLED_DIR: &str = "installed-adapters";
const INDEX_FILE: &str = "index.json";
/// Handshake + ping deadline for `check`.
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// One index row — the durable record of an installed bundle.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct InstalledAdapter {
    /// Registered adapter id (from the manifest).
    pub adapter_id: String,
    /// Registered version.
    pub version: String,
    /// Recomputed `sha256:` bundle digest.
    pub bundle_digest: String,
    /// Subdirectory name under `installed-adapters/`.
    pub dir_name: String,
    /// Whether boot registers this bundle.
    pub enabled: bool,
    /// Last spawn-check outcome (`pass` / `fail: <detail>`), if run.
    #[serde(default)]
    pub check: Option<String>,
    /// Install wall-clock time (unix ms).
    pub installed_at_ms: i64,
}

/// Outcome of a standalone spawn-check (bootstrap → hello → ping → pong).
#[derive(Clone, Debug, serde::Serialize)]
pub struct CheckReport {
    /// Adapter id read from the manifest.
    pub adapter_id: String,
    /// Bundle version.
    pub version: String,
    /// Bundle digest verified on disk.
    pub bundle_digest: String,
    /// `pass` or `fail: <reason>`.
    pub result: String,
    /// Per-case `name=outcome` details.
    pub cases: Vec<String>,
    /// Ports declared in the validated `Hello` (empty on failure).
    pub implemented_ports: Vec<String>,
}

fn cli_err(msg: impl Into<String>) -> KernelError {
    KernelError::new(ErrorCode::InvalidArgument, RetryClass::Never, msg.into())
}

fn io_err(op: &str, e: &std::io::Error) -> KernelError {
    KernelError::new(
        ErrorCode::Unavailable,
        RetryClass::Safe,
        format!("adapter cli: {op} failed"),
    )
    .with_source(std::io::Error::new(e.kind(), e.to_string()))
}

fn index_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(INSTALLED_DIR).join(INDEX_FILE)
}

/// Reads the install index; a missing index is an empty install set.
pub fn load_index(runtime_dir: &Path) -> errors::Result<Vec<InstalledAdapter>> {
    match std::fs::read_to_string(index_path(runtime_dir)) {
        Ok(body) => serde_json::from_str(&body)
            .map_err(|e| cli_err(format!("installed-adapters index is corrupt: {e}"))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(io_err("read install index", &e)),
    }
}

fn save_index(runtime_dir: &Path, entries: &[InstalledAdapter]) -> errors::Result<()> {
    let dir = runtime_dir.join(INSTALLED_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| io_err("create install dir", &e))?;
    let body = serde_json::to_string_pretty(entries).map_err(|e| cli_err(e.to_string()))?;
    let tmp = dir.join("index.json.tmp");
    std::fs::write(&tmp, body).map_err(|e| io_err("write index", &e))?;
    std::fs::rename(&tmp, dir.join(INDEX_FILE)).map_err(|e| io_err("rename index", &e))
}

/// Verifies a bundle dir on disk (manifest parse + lock digests) and
/// returns the manifest + bundle digest.
fn verify_bundle(dir: &Path) -> errors::Result<(ExtensionManifest, String)> {
    let manifest_bytes =
        std::fs::read(dir.join(MANIFEST_FILE)).map_err(|e| io_err("read manifest", &e))?;
    let manifest = parse_manifest(&manifest_bytes)?;
    let digest = compute_bundle_digest(dir, &manifest_digest(&manifest_bytes))?;
    Ok((manifest, digest))
}

/// Copies `src` into `dst` recursively; only regular files and dirs are
/// allowed (no symlinks — a symlink could escape the install root).
fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        let to = dst.join(&name);
        let meta = std::fs::symlink_metadata(entry.path())?;
        if meta.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else if meta.is_file() {
            std::fs::copy(entry.path(), &to)?;
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "bundle asset '{}' is not a regular file",
                    name.to_string_lossy()
                ),
            ));
        }
    }
    Ok(())
}

/// `agentd adapter check --bundle <dir>`: spawns the bundle's entrypoint
/// on a private socketpair and runs bootstrap → hello → ping → pong.
/// The child is always terminated before returning.
pub async fn check_bundle(bundle_dir: &Path) -> errors::Result<CheckReport> {
    let (manifest, digest) = verify_bundle(bundle_dir)?;
    let adapter_id = manifest.id.parse::<AdapterId>().map_err(|e| {
        cli_err(format!(
            "manifest id '{}' is not an adapter id: {e}",
            manifest.id
        ))
    })?;
    let ids = SystemIdProvider;
    let instance = AdapterInstanceId::new(&ids);
    let daemon = DaemonInstanceId::new(&ids);
    let identity = ExpectedIdentity {
        daemon_instance_id: daemon,
        daemon_fencing_epoch: 1,
        adapter_instance_id: instance.to_string(),
        adapter_id: adapter_id.to_string(),
        adapter_version: manifest.version.clone(),
        expected_bundle_digest: digest.clone(),
        protocol_version: 1,
    };
    let mut child = process_supervisor::spawn(&SpawnSpec {
        adapter_id,
        adapter_version: manifest.version.clone(),
        expected_bundle_digest: digest.clone(),
        adapter_instance_id: instance,
        daemon_instance_id: daemon,
        daemon_fencing_epoch: 1,
        protocol_version: 1,
        executable: bundle_dir.join(&manifest.runtime.entrypoint),
        argv: Vec::new(),
        // Fixture adapters self-assert identity from these env vars — the
        // same conventions spawn_loop/spawn_effect_adapter propagate.
        env: vec![
            ("AGENTOS_ADAPTER_ID".to_owned(), adapter_id.to_string()),
            (
                "AGENTOS_ADAPTER_VERSION".to_owned(),
                manifest.version.clone(),
            ),
            ("FIXTURE_ADAPTER_ID".to_owned(), adapter_id.to_string()),
            (
                "FIXTURE_ADAPTER_VERSION".to_owned(),
                manifest.version.clone(),
            ),
            ("FIXTURE_LOOP_ADAPTER_ID".to_owned(), adapter_id.to_string()),
            (
                "FIXTURE_LOOP_ADAPTER_VERSION".to_owned(),
                manifest.version.clone(),
            ),
        ],
        cwd: None,
    })?;
    child.drain_output();
    let smoke = probe(child.ipc(), &identity, &ids).await;
    process_supervisor::terminate(child, Duration::from_secs(2)).await;
    let (cases, ports, outcome) = match smoke {
        Ok((cases, ports)) => (cases, ports, "pass".to_owned()),
        Err(e) => (vec![format!("error={e}")], Vec::new(), format!("fail: {e}")),
    };
    Ok(CheckReport {
        adapter_id: adapter_id.to_string(),
        version: manifest.version.clone(),
        bundle_digest: digest,
        result: outcome,
        cases,
        implemented_ports: ports,
    })
}

/// The handshake + ping sequence inside a check (spawn already done).
async fn probe(
    stream: &mut std::os::unix::net::UnixStream,
    identity: &ExpectedIdentity,
    ids: &dyn IdProvider,
) -> errors::Result<(Vec<String>, Vec<String>)> {
    let nonce = ids.new_uuid_v7().to_string();
    write_frame(stream, &identity.bootstrap_frame(&nonce))?;
    stream
        .set_read_timeout(Some(CHECK_TIMEOUT))
        .map_err(|e| io_err("set read timeout", &e))?;
    let ports = match read_frame(stream)?.and_then(|f| f.body) {
        Some(Body::Hello(hello)) => {
            identity.verify_hello(&hello, &nonce)?;
            hello.implemented_ports
        }
        other => return Err(cli_err(format!("expected AdapterHello, got {other:?}"))),
    };
    let mut cases = vec!["handshake=pass".to_owned()];
    let ping_nonce = ids.new_uuid_v7().to_string();
    write_frame(
        stream,
        &AdapterFrame {
            body: Some(Body::Ping(AdapterPing {
                nonce: ping_nonce.clone(),
            })),
        },
    )?;
    match read_frame(stream)?.and_then(|f| f.body) {
        Some(Body::Pong(pong)) if pong.nonce == ping_nonce => {
            cases.push("ping_pong=pass".to_owned());
        }
        other => {
            return Err(cli_err(format!(
                "expected pong for {ping_nonce}, got {other:?}"
            )));
        }
    }
    Ok((cases, ports))
}

/// Unique-prefix match on `(adapter_id[, version])` inside the index.
fn find_entry(
    entries: &[InstalledAdapter],
    id_prefix: &str,
    version: Option<&str>,
) -> errors::Result<usize> {
    let matches: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            e.adapter_id.starts_with(id_prefix) && version.is_none_or(|v| e.version == v)
        })
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [i] => Ok(*i),
        [] => Err(cli_err(format!(
            "no installed adapter matches '{id_prefix}'"
        ))),
        _ => Err(cli_err(format!(
            "'{id_prefix}' matches {} installed adapters — be more specific",
            matches.len()
        ))),
    }
}

/// `agentd adapter install --runtime-dir D --bundle <dir> [--check]`.
/// Verifies the bundle, copies it under `installed-adapters/`, re-verifies
/// the copy, optionally runs the spawn-check, and records the index entry.
/// Re-installing the same `(id, version)` replaces it.
pub async fn install(
    runtime_dir: &Path,
    bundle_dir: &Path,
    run_check: bool,
    now_ms: i64,
) -> errors::Result<InstalledAdapter> {
    if !bundle_dir.join(MANIFEST_FILE).is_file() {
        return Err(cli_err(format!(
            "{} is not an adapter bundle (missing {MANIFEST_FILE})",
            bundle_dir.display()
        )));
    }
    if !bundle_dir.join(LOCK_FILE).is_file() {
        return Err(cli_err(format!(
            "bundle lacks {LOCK_FILE} — run scripts/make-adapter-bundle.sh first"
        )));
    }
    let (manifest, digest) = verify_bundle(bundle_dir)?;
    let dir_name = format!(
        "{}-{}-{}",
        manifest.id,
        manifest.version,
        &digest.strip_prefix("sha256:").unwrap_or(&digest)[..12]
    );
    let dest = runtime_dir.join(INSTALLED_DIR).join(&dir_name);
    if dest.exists() {
        std::fs::remove_dir_all(&dest).map_err(|e| io_err("replace installed bundle", &e))?;
    }
    copy_dir(bundle_dir, &dest).map_err(|e| io_err("copy bundle", &e))?;
    // Re-verify the installed copy — a torn/mutated copy fails now, not
    // at first spawn.
    let (_, copy_digest) = verify_bundle(&dest)?;
    if copy_digest != digest {
        let _ = std::fs::remove_dir_all(&dest);
        return Err(cli_err("installed copy digest mismatch — install aborted"));
    }
    let check = if run_check {
        let report = check_bundle(&dest).await?;
        // A failed smoke test must not leave an enabled bundle behind —
        // boot would register an adapter that already failed to run.
        if report.result != "pass" {
            let _ = std::fs::remove_dir_all(&dest);
            return Err(cli_err(format!("adapter check failed: {}", report.result)));
        }
        Some(report.result)
    } else {
        None
    };
    let entry = InstalledAdapter {
        adapter_id: manifest.id.clone(),
        version: manifest.version.clone(),
        bundle_digest: digest,
        dir_name,
        enabled: true,
        check,
        installed_at_ms: now_ms,
    };
    with_index_lock(runtime_dir, || {
        let mut entries = load_index(runtime_dir)?;
        entries.retain(|e| !(e.adapter_id == entry.adapter_id && e.version == entry.version));
        entries.push(entry.clone());
        save_index(runtime_dir, &entries)
    })?;
    Ok(entry)
}

/// Serializes index mutations across concurrent `agentd adapter`
/// invocations — `flock` on a sidecar lockfile wraps the load+save pair.
fn with_index_lock<R>(
    runtime_dir: &Path,
    f: impl FnOnce() -> errors::Result<R>,
) -> errors::Result<R> {
    let dir = runtime_dir.join(INSTALLED_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| io_err("create install dir", &e))?;
    let lock =
        std::fs::File::create(dir.join("index.lock")).map_err(|e| io_err("open index lock", &e))?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockExclusive)
        .map_err(|e| io_err("lock index", &std::io::Error::from(e)))?;
    let out = f();
    let _ = rustix::fs::flock(&lock, rustix::fs::FlockOperation::Unlock);
    out
}

/// `enable`/`disable` flip whether boot registers the bundle.
pub fn set_enabled(
    runtime_dir: &Path,
    id_prefix: &str,
    version: Option<&str>,
    enabled: bool,
) -> errors::Result<InstalledAdapter> {
    with_index_lock(runtime_dir, || {
        let mut entries = load_index(runtime_dir)?;
        let i = find_entry(&entries, id_prefix, version)?;
        entries[i].enabled = enabled;
        let entry = entries[i].clone();
        save_index(runtime_dir, &entries)?;
        Ok(entry)
    })
}

/// `remove` deletes the bundle dir and its index entry.
pub fn remove(runtime_dir: &Path, id_prefix: &str, version: Option<&str>) -> errors::Result<()> {
    with_index_lock(runtime_dir, || {
        let mut entries = load_index(runtime_dir)?;
        let i = find_entry(&entries, id_prefix, version)?;
        let dir = runtime_dir.join(INSTALLED_DIR).join(&entries[i].dir_name);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| io_err("remove bundle dir", &e))?;
        }
        entries.remove(i);
        save_index(runtime_dir, &entries)
    })
}

/// Bundle dirs boot should register: enabled index entries whose dir
/// still exists on disk.
pub fn enabled_bundle_dirs(runtime_dir: &Path) -> errors::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in load_index(runtime_dir)? {
        if !entry.enabled {
            continue;
        }
        let dir = runtime_dir.join(INSTALLED_DIR).join(&entry.dir_name);
        if dir.join(MANIFEST_FILE).is_file() {
            out.push(dir);
        } else {
            tracing::warn!(dir = %dir.display(), "installed adapter bundle missing — skipped");
        }
    }
    Ok(out)
}
