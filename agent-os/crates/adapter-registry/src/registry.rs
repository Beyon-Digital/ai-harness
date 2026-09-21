//! Content-addressed adapter registry.
//!
//! Bundle layout (`specs/adapter-registry.md`):
//!
//! ```text
//! <bundle>/adapter.manifest.json   extension manifest v1
//! <bundle>/bundle.lock            "sha256:<hex> <relpath>" per asset,
//!                                 sorted by path, unique
//! <bundle>/<entrypoint>           executable listed in the lock
//! ```
//!
//! `bundle_digest = sha256("agentos.bundle.v1\n" ++ manifest_digest ++
//! "\n" ++ lock_bytes)` — the kernel recomputes it from disk at register
//! and again immediately before spawn, so a mutated bundle can never
//! execute under a stale identity.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use domain::ids::AdapterId;
use domain::security::{ConformanceState, TrustState};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{AdapterRegistrationRow, NewAdapterRegistration};
use kernel_store::txn::KernelTxn;
use sha2::{Digest, Sha256};

use crate::manifest::{
    ExtensionManifest, LOCK_FILE, MANIFEST_FILE, hex, manifest_digest, parse_manifest,
};

/// Parsed lock entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockEntry {
    /// Expected SHA-256 of the file at `path`.
    pub sha256_hex: String,
    /// Bundle-root-relative path.
    pub path: String,
}

/// A verified bundle on disk — identity has been recomputed and matches.
#[derive(Clone, Debug)]
pub struct VerifiedBundle {
    /// Bundle root directory.
    pub dir: PathBuf,
    /// Parsed manifest.
    pub manifest: ExtensionManifest,
    /// Recomputed bundle digest — equals the registered digest.
    pub bundle_digest: String,
    /// Absolute path of the spawnable entrypoint (verified in the lock).
    pub entrypoint: PathBuf,
    /// Raw manifest digest.
    pub manifest_digest: String,
}

/// Reads `bundle.lock` under `dir`: each line `sha256:<64hex> <relpath>`,
/// sorted by path and unique. Rejects duplicates, escapes (`..`,
/// absolute), and non-lexicographic order.
pub fn parse_lock(lock_bytes: &[u8]) -> errors::Result<Vec<LockEntry>> {
    let text = std::str::from_utf8(lock_bytes).map_err(|e| {
        KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            "bundle.lock is not UTF-8",
        )
        .with_source(e)
    })?;
    let mut entries = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let (digest_part, path) = line.split_once(' ').ok_or_else(|| {
            invalid(format!(
                "bundle.lock line {} lacks '<digest> <path>'",
                line_no + 1
            ))
        })?;
        let sha = digest_part.strip_prefix("sha256:").ok_or_else(|| {
            invalid(format!(
                "bundle.lock line {} digest lacks sha256: prefix",
                line_no + 1
            ))
        })?;
        if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(invalid(format!(
                "bundle.lock line {} has malformed sha256",
                line_no + 1
            )));
        }
        if path.is_empty()
            || path.starts_with('/')
            || path.split('/').any(|seg| seg == ".." || seg.is_empty())
        {
            return Err(invalid(format!(
                "bundle.lock path '{path}' escapes or is malformed"
            )));
        }
        entries.push(LockEntry {
            sha256_hex: sha.to_lowercase(),
            path: path.to_owned(),
        });
    }
    let mut sorted = entries.clone();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    if sorted
        .iter()
        .zip(entries.iter())
        .any(|(s, e)| s.path != e.path)
    {
        return Err(invalid(
            "bundle.lock entries must be sorted by path".to_owned(),
        ));
    }
    if entries.windows(2).any(|w| w[0].path == w[1].path) {
        return Err(invalid("bundle.lock has duplicate paths".to_owned()));
    }
    Ok(entries)
}

/// Verifies every lock entry's file content under `dir` (rejecting
/// symlinks that resolve outside `dir`), then returns the recomputed
/// `sha256:` bundle digest over the canonical lock bytes + manifest digest.
pub fn compute_bundle_digest(dir: &Path, manifest_digest: &str) -> errors::Result<String> {
    let lock_path = dir.join(LOCK_FILE);
    let lock_bytes = std::fs::read(&lock_path).map_err(|e| io("read bundle.lock", &e))?;
    let entries = parse_lock(&lock_bytes)?;
    let root = dir
        .canonicalize()
        .map_err(|e| io("canonicalize bundle dir", &e))?;
    for entry in &entries {
        let path = root.join(&entry.path);
        // Reject symlinks escaping the bundle root.
        let canonical = path.canonicalize().map_err(|e| {
            KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                format!("bundle asset '{}' unreadable", entry.path),
            )
            .with_source(e)
        })?;
        if !canonical.starts_with(&root) {
            return Err(invalid(format!(
                "bundle asset '{}' resolves outside the bundle root",
                entry.path
            )));
        }
        let bytes = std::fs::read(&canonical).map_err(|e| io("read bundle asset", &e))?;
        let got = hex(Sha256::digest(&bytes));
        if got != entry.sha256_hex {
            return Err(KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                format!("bundle asset '{}' digest mismatch", entry.path),
            ));
        }
    }
    let mut hasher = Sha256::new();
    hasher.update(b"agentos.bundle.v1\n");
    hasher.update(manifest_digest.as_bytes());
    hasher.update(b"\n");
    hasher.update(&lock_bytes);
    Ok(format!("sha256:{}", hex(hasher.finalize())))
}

/// Registers the bundle at `dir` inside `txn`: parses the manifest,
/// verifies the lock, and persists the `(id, version, bundle_digest)`
/// identity. The same triple re-registered with identical content is a
/// `Conflict` — identity is immutable.
pub async fn register(
    txn: &mut dyn KernelTxn,
    dir: &Path,
    trust_state: TrustState,
    now_ms: i64,
) -> errors::Result<AdapterRegistrationRow> {
    let manifest_bytes =
        std::fs::read(dir.join(MANIFEST_FILE)).map_err(|e| io("read manifest", &e))?;
    let manifest = parse_manifest(&manifest_bytes)?;
    let manifest_digest = manifest_digest(&manifest_bytes);
    let bundle_digest = compute_bundle_digest(dir, &manifest_digest)?;
    let adapter_id = manifest.id.parse::<AdapterId>().map_err(|e| {
        invalid(format!(
            "manifest id '{}' is not an adapter id: {e}",
            manifest.id
        ))
    })?;
    let row = NewAdapterRegistration {
        adapter_id,
        version: manifest.version.clone(),
        bundle_digest: bundle_digest.clone(),
        manifest_digest,
        runtime_type: manifest.runtime.runtime_type.clone(),
        implemented_ports: serde_json::to_vec(&manifest.implemented_ports()?)
            .map_err(|e| invalid(e.to_string()))?,
        capabilities: serde_json::to_vec(&manifest.capabilities())
            .map_err(|e| invalid(e.to_string()))?,
        trust_state,
        conformance_state: ConformanceState::Untested,
        created_at_ms: now_ms,
    };
    let key = (
        row.adapter_id,
        row.version.clone(),
        row.bundle_digest.clone(),
    );
    if txn
        .adapters()
        .get_registration(key.0, &key.1, &key.2)
        .await?
        .is_some()
    {
        return Err(KernelError::new(
            ErrorCode::Conflict,
            RetryClass::Never,
            format!("adapter identity {}@{} already registered", key.0, key.1),
        ));
    }
    txn.adapters().insert_registration(row.clone()).await?;
    Ok(AdapterRegistrationRow {
        adapter_id: row.adapter_id,
        version: row.version,
        bundle_digest: row.bundle_digest,
        manifest_digest: row.manifest_digest,
        runtime_type: row.runtime_type,
        implemented_ports: row.implemented_ports,
        capabilities: row.capabilities,
        trust_state: row.trust_state,
        conformance_state: row.conformance_state,
        created_at_ms: row.created_at_ms,
    })
}

/// Loads the registration for `expected` identity and re-verifies the
/// on-disk bundle **immediately before spawn** — the returned
/// `VerifiedBundle` is the only lawful source of an executable path.
pub async fn verify_for_spawn(
    txn: &mut dyn KernelTxn,
    adapter_id: AdapterId,
    version: &str,
    expected_bundle_digest: &str,
    dir: &Path,
) -> errors::Result<VerifiedBundle> {
    let row = txn
        .adapters()
        .get_registration(adapter_id, version, expected_bundle_digest)
        .await?
        .ok_or_else(|| {
            KernelError::new(
                ErrorCode::NotFound,
                RetryClass::Never,
                format!("no registered adapter {adapter_id}@{version}:{expected_bundle_digest}"),
            )
        })?;
    let manifest_bytes =
        std::fs::read(dir.join(MANIFEST_FILE)).map_err(|e| io("read manifest", &e))?;
    let manifest = parse_manifest(&manifest_bytes)?;
    let manifest_digest = manifest_digest(&manifest_bytes);
    let recomputed = compute_bundle_digest(dir, &manifest_digest)?;
    if recomputed != row.bundle_digest {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            format!(
                "bundle digest drifted: registry has {}, disk computes {}",
                row.bundle_digest, recomputed
            ),
        ));
    }
    let root = dir
        .canonicalize()
        .map_err(|e| io("canonicalize bundle", &e))?;
    let entrypoint = root.join(&manifest.runtime.entrypoint);
    if !entrypoint
        .canonicalize()
        .map_err(|e| io("canonicalize entrypoint", &e))?
        .starts_with(&root)
    {
        return Err(invalid("entrypoint escapes the bundle root".to_owned()));
    }
    Ok(VerifiedBundle {
        dir: root,
        manifest,
        bundle_digest: recomputed,
        entrypoint,
        manifest_digest,
    })
}

/// Generates a `bundle.lock` for a fixture bundle under `dir`:
/// sha256 over every regular file except the manifest/lock themselves,
/// sorted by relative path. Used by fixtures and tests only.
pub fn write_lock(dir: &Path) -> errors::Result<()> {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).map_err(|e| io("read_dir", &e))? {
            let entry = entry.map_err(|e| io("read_dir entry", &e))?;
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).map_err(|e| io("stat", &e))?;
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() || meta.is_symlink() {
                let rel = path
                    .strip_prefix(dir)
                    .map_err(|e| invalid(e.to_string()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                if rel == LOCK_FILE || rel == MANIFEST_FILE {
                    continue;
                }
                files.push((rel, path));
            }
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = String::new();
    for (rel, path) in &files {
        let bytes = std::fs::read(path).map_err(|e| io("hash asset", &e))?;
        out.push_str(&format!("sha256:{} {}\n", hex(Sha256::digest(&bytes)), rel));
    }
    std::fs::write(dir.join(LOCK_FILE), out).map_err(|e| io("write bundle.lock", &e))
}

/// Locates the `agentos-wasm-host` binary that runs `runtime.type =
/// "wasm"` bundles: `AGENTOS_WASM_HOST` env override first, else a
/// sibling of the current executable (cargo keeps workspace bins in the
/// same target dir).
pub fn wasm_host_binary() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("AGENTOS_WASM_HOST") {
        let path = std::path::PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    // Sibling of the current executable — covers `target/debug/agentd`
    // in production and `target/debug/deps/<test>` under `cargo test`
    // (walk up one when the exe sits in a `deps` dir).
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    for dir in [Some(dir), dir.parent()] {
        let candidate = dir.map(|d| d.join("agentos-wasm-host"));
        if let Some(p) = candidate.filter(|p| p.is_file()) {
            return Some(p);
        }
    }
    None
}

fn invalid(message: String) -> KernelError {
    KernelError::new(ErrorCode::InvalidArgument, RetryClass::Never, message)
}

fn io(op: &str, e: &std::io::Error) -> KernelError {
    KernelError::new(
        ErrorCode::Unavailable,
        RetryClass::Safe,
        format!("adapter registry {op} failed"),
    )
    .with_source(std::io::Error::new(e.kind(), e.to_string()))
}
