//! Local ArtifactStore (ART-001): durable, immutable content under an
//! app-owned directory; metadata in `artifacts`.
//!
//! Writes are atomic — content lands in a temp file in the same
//! directory, fsynced, then `rename`d onto the content-addressed path;
//! a crash mid-write can only leave an unlinked temp file. The
//! `artifact://` URI and digest are the only identities agent-facing
//! APIs ever see — the physical `locator` stays adapter-private.
#![forbid(unsafe_code)]

use std::io::Write;
use std::path::{Path, PathBuf};

use domain::ids::{ArtifactId, EffectId, RunId};
use domain::security::{RetentionClass, SensitivityClass};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{ArtifactRow, NewArtifact};
use kernel_store::txn::KernelTxn;
use sha2::{Digest, Sha256};

fn artifact_error(code: ErrorCode, msg: impl Into<String>) -> KernelError {
    KernelError::new(code, RetryClass::Never, msg.into())
}

fn io_error(op: &str, e: std::io::Error) -> KernelError {
    KernelError::new(
        ErrorCode::Unavailable,
        RetryClass::Safe,
        format!("artifact {op} failed"),
    )
    .with_source(e)
}

/// Metadata attached to a stored artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PutMetadata {
    /// Media type label (`application/octet-stream` default upstream).
    pub media_type: String,
    /// Sensitivity classification.
    pub sensitivity: SensitivityClass,
    /// Retention class — controls `delete` eligibility.
    pub retention: RetentionClass,
    /// Optional effect this output originates from.
    pub origin_effect_id: Option<EffectId>,
}

/// An artifact stored under `root`, addressed by `artifact://<id>`.
pub struct LocalArtifactStore {
    root: PathBuf,
}

impl LocalArtifactStore {
    /// Open (creating) the store root.
    pub fn new(root: PathBuf) -> errors::Result<Self> {
        std::fs::create_dir_all(&root).map_err(|e| io_error("mkdir", e))?;
        Ok(Self { root })
    }

    /// Locator for `id`/`digest` — adapter-private; never leaves this
    /// crate's return values in persisted rows beyond the `locator`
    /// column, which domain records treat as opaque.
    fn locator(&self, id: ArtifactId, digest: &str) -> PathBuf {
        let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
        self.root.join(format!("{id}.{hex}.bin"))
    }

    /// Store `data` and its metadata row inside `txn`. Returns the row.
    /// Content lands atomically before the row inserts, so a committed
    /// artifact is always readable; a failed insert leaves an orphaned
    /// temp-named file that is never referenced by any URI.
    pub async fn put(
        &self,
        txn: &mut dyn KernelTxn,
        id: ArtifactId,
        data: &[u8],
        meta: PutMetadata,
        origin_run: RunId,
        now_ms: i64,
    ) -> errors::Result<ArtifactRow> {
        let digest = format!("sha256:{}", hex_encode(&Sha256::digest(data)));
        let uri = format!("artifact://{id}");
        if txn.artifacts().get_by_uri(&uri).await?.is_some() {
            return Err(artifact_error(
                ErrorCode::Conflict,
                "artifact uri already exists",
            ));
        }
        let final_path = self.locator(id, &digest);
        write_atomic(&final_path, data)?;

        let locator = final_path
            .strip_prefix(&self.root)
            .unwrap_or(&final_path)
            .to_string_lossy()
            .into_owned();
        txn.artifacts()
            .insert(NewArtifact {
                artifact_id: id,
                uri,
                digest,
                media_type: meta.media_type,
                size_bytes: data.len() as i64,
                origin_run_id: origin_run,
                origin_effect_id: meta.origin_effect_id,
                sensitivity: meta.sensitivity,
                retention: meta.retention,
                locator,
                created_at_ms: now_ms,
            })
            .await?;
        txn.artifacts()
            .get_by_id(id)
            .await?
            .ok_or_else(|| artifact_error(ErrorCode::Internal, "inserted artifact not visible"))
    }

    /// Read the artifact's full content, verifying the digest on read —
    /// a bit-rotted or swapped file fails rather than returning wrong
    /// bytes.
    pub async fn get(
        &self,
        txn: &mut dyn KernelTxn,
        uri: &str,
    ) -> errors::Result<(ArtifactRow, Vec<u8>)> {
        let row = self.lookup(txn, uri).await?;
        let data = std::fs::read(self.root.join(&row.locator)).map_err(|e| io_error("read", e))?;
        let actual = format!("sha256:{}", hex_encode(&Sha256::digest(&data)));
        if actual != row.digest {
            return Err(artifact_error(
                ErrorCode::FailedPrecondition,
                "artifact content digest mismatch",
            ));
        }
        Ok((row, data))
    }

    /// Range read `[offset, offset+len)`; out-of-range offsets clamp.
    pub async fn get_range(
        &self,
        txn: &mut dyn KernelTxn,
        uri: &str,
        offset: u64,
        len: u64,
    ) -> errors::Result<(ArtifactRow, Vec<u8>)> {
        let (row, data) = self.get(txn, uri).await?;
        let start = (offset as usize).min(data.len());
        let end = (start + len as usize).min(data.len());
        Ok((row, data[start..end].to_vec()))
    }

    /// Metadata only.
    pub async fn head(&self, txn: &mut dyn KernelTxn, uri: &str) -> errors::Result<ArtifactRow> {
        self.lookup(txn, uri).await
    }

    /// All artifacts originating from `run`.
    pub async fn list_by_run(
        &self,
        txn: &mut dyn KernelTxn,
        run: RunId,
    ) -> errors::Result<Vec<ArtifactRow>> {
        txn.artifacts().list_by_run(run).await
    }

    /// Delete is allowed only for non-audit retention — `Standard`/`Audit`
    /// rows are retention-pinned and must age out via policy elsewhere.
    pub async fn delete(&self, txn: &mut dyn KernelTxn, uri: &str) -> errors::Result<()> {
        let row = self.lookup(txn, uri).await?;
        if row.retention != RetentionClass::Ephemeral {
            return Err(artifact_error(
                ErrorCode::FailedPrecondition,
                "artifact retention class forbids delete",
            ));
        }
        let _ = txn; // deletion of the row is governed by retention policy
        // upstream of this adapter; content unlink is allowed.
        let path = self.root.join(&row.locator);
        if path.exists() {
            std::fs::remove_file(&path).map_err(|e| io_error("delete", e))?;
        }
        Ok(())
    }

    async fn lookup(&self, txn: &mut dyn KernelTxn, uri: &str) -> errors::Result<ArtifactRow> {
        txn.artifacts()
            .get_by_uri(uri)
            .await?
            .ok_or_else(|| artifact_error(ErrorCode::NotFound, "artifact not found"))
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Write `data` to `path` atomically: temp file + fsync + rename, then
/// fsync the directory so the rename itself is durable.
fn write_atomic(path: &Path, data: &[u8]) -> errors::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| artifact_error(ErrorCode::Internal, "artifact path has no parent"))?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| io_error("tempfile", e))?;
    tmp.write_all(data).map_err(|e| io_error("write", e))?;
    tmp.as_file().sync_all().map_err(|e| io_error("fsync", e))?;
    tmp.persist(path).map_err(|e| {
        artifact_error(
            ErrorCode::Unavailable,
            format!("artifact rename failed: {}", e.error),
        )
    })?;
    if let Ok(dir_fd) = std::fs::File::open(dir) {
        let _ = dir_fd.sync_all();
    }
    Ok(())
}
