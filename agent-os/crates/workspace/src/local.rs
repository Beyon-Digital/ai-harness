//! Local workspace adapter (`specs/workspace.md` "Local adapter").
//!
//! Git repositories get worktree forks; anything else gets a
//! copy-on-create fork explicitly labelled `CopyFork` — the adapter never
//! claims Git semantics it doesn't have.
//!
//! Every file operation resolves a normalized relative path under the
//! workspace root; `..`, absolute paths, and symlink escapes are denied.
#![forbid(unsafe_code)]

use std::path::{Component, Path, PathBuf};

use domain::ids::WorkspaceId;
use domain::resource::WorkspaceAccessMode;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{NewWorkspace, NewWorkspaceLease};
use kernel_store::txn::KernelTxn;

use crate::git;

/// Durable `workspaces.kind` literals.
pub mod kind {
    /// Git worktree-backed workspace.
    pub const GIT_WORKTREE: &str = "git-worktree";
    /// Copy-on-create directory fork (non-Git).
    pub const COPY_FORK: &str = "copy-fork";
}

/// What the local adapter actually provides — reported honestly so
/// callers never conflate a copy fork with Git semantics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AdapterCapabilities {
    /// `git-worktree` when the root is a Git repo, else `copy-fork`.
    pub kind: &'static str,
    /// Git-revision semantics (exact base revision, worktree fork).
    pub git_semantic: bool,
    /// T0 truth: this adapter is not a filesystem security boundary.
    pub is_security_boundary: bool,
}

/// The adapter's capabilities for `root`.
pub fn capabilities(root: &Path) -> AdapterCapabilities {
    if git::is_git_repo(root) {
        AdapterCapabilities {
            kind: kind::GIT_WORKTREE,
            git_semantic: true,
            is_security_boundary: false,
        }
    } else {
        AdapterCapabilities {
            kind: kind::COPY_FORK,
            git_semantic: false,
            is_security_boundary: false,
        }
    }
}

/// Normalizes `relative` against `root` — refuses absolute paths, `..`,
/// and any path whose canonicalization escapes the canonical root.
pub fn resolve_relative(root: &Path, relative: &str) -> errors::Result<PathBuf> {
    if relative.is_empty() || relative.starts_with('/') || relative.contains('\\') {
        return Err(denied(format!(
            "path '{relative}' is not a normalized relative path"
        )));
    }
    let mut parts = PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(c) => parts.push(c),
            _ => {
                return Err(denied(format!("path '{relative}' escapes the workspace")));
            }
        }
    }
    let root_canonical = root
        .canonicalize()
        .map_err(|e| io("canonicalize workspace", e))?;
    let joined = root_canonical.join(&parts);
    // Existing paths must canonicalize inside the root; a not-yet-created
    // path is checked on its deepest existing ancestor.
    let mut probe = joined.clone();
    loop {
        match probe.canonicalize() {
            Ok(c) => {
                if !c.starts_with(&root_canonical) {
                    return Err(denied(format!("path '{relative}' escapes the workspace")));
                }
                break;
            }
            Err(_) => match probe.parent() {
                Some(p) => probe = p.to_path_buf(),
                None => break,
            },
        }
    }
    Ok(joined)
}

/// Reads `relative` under `root`.
pub fn read(root: &Path, relative: &str) -> errors::Result<Vec<u8>> {
    let path = resolve_relative(root, relative)?;
    std::fs::read(&path).map_err(|e| io("read", e))
}

/// Writes `data` at `relative` under `root` (creating parent dirs).
pub fn write(root: &Path, relative: &str, data: &[u8]) -> errors::Result<()> {
    let path = resolve_relative(root, relative)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| io("mkdir", e))?;
    }
    std::fs::write(&path, data).map_err(|e| io("write", e))
}

/// Lists files under `root` as normalized relative paths — `git ls-files`
/// for a Git workspace, recursive walk for a copy workspace.
pub fn list(root: &Path) -> errors::Result<Vec<String>> {
    if git::is_git_repo(root) {
        return Ok(git::list_files(root)?
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect());
    }
    let root_canonical = root.canonicalize().map_err(|e| io("canonicalize", e))?;
    let mut out = Vec::new();
    let mut stack = vec![root_canonical.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|e| io("read_dir", e))? {
            let entry = entry.map_err(|e| io("entry", e))?;
            let meta = std::fs::symlink_metadata(entry.path()).map_err(|e| io("stat", e))?;
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                let rel = entry
                    .path()
                    .strip_prefix(&root_canonical)
                    .map_err(|e| io("strip", std::io::Error::other(e.to_string())))?
                    .to_string_lossy()
                    .to_string();
                out.push(rel);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// The exact base revision of `root` — HEAD for Git, `None` otherwise.
pub fn base_revision(root: &Path) -> errors::Result<Option<String>> {
    if git::is_git_repo(root) {
        git::head_revision(root).map(Some)
    } else {
        Ok(None)
    }
}

/// Forks `parent_root` into `child_root` and records the child workspace
/// row (kind labelled by actual mechanism) inside `txn`.
///
/// For Git, the fork is `git worktree add --detach <child> <base>` — the
/// exact parent base revision, not floating HEAD.
pub async fn fork(
    txn: &mut dyn KernelTxn,
    parent_root: &Path,
    parent_workspace_id: WorkspaceId,
    child_workspace_id: WorkspaceId,
    child_root: &Path,
    now_ms: i64,
) -> errors::Result<AdapterCapabilities> {
    let caps = capabilities(parent_root);
    if caps.git_semantic {
        // The fork is pinned at the parent's recorded base revision — not
        // whatever HEAD happens to be when `fork` is invoked.
        let parent = txn
            .workspaces()
            .get_workspace(parent_workspace_id)
            .await?
            .ok_or_else(|| {
                KernelError::new(
                    ErrorCode::NotFound,
                    RetryClass::Never,
                    "parent workspace row missing",
                )
            })?;
        let base = parent.base_revision.clone().ok_or_else(|| {
            KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                "parent workspace has no recorded base revision",
            )
        })?;
        git::add_worktree(parent_root, &base, child_root)?;
        txn.workspaces()
            .insert_workspace(NewWorkspace {
                workspace_id: child_workspace_id,
                kind: kind::GIT_WORKTREE.to_owned(),
                base_revision: Some(base),
                parent_workspace_id: Some(parent_workspace_id),
                created_at_ms: now_ms,
            })
            .await?;
    } else {
        git::copy_tree(parent_root, child_root)?;
        txn.workspaces()
            .insert_workspace(NewWorkspace {
                workspace_id: child_workspace_id,
                kind: kind::COPY_FORK.to_owned(),
                base_revision: None,
                parent_workspace_id: Some(parent_workspace_id),
                created_at_ms: now_ms,
            })
            .await?;
    }
    Ok(caps)
}

/// Records the initial workspace row for a root created via `create`.
pub async fn record_workspace(
    txn: &mut dyn KernelTxn,
    workspace_id: WorkspaceId,
    root: &Path,
    now_ms: i64,
) -> errors::Result<AdapterCapabilities> {
    let caps = capabilities(root);
    txn.workspaces()
        .insert_workspace(NewWorkspace {
            workspace_id,
            kind: caps.kind.to_owned(),
            base_revision: base_revision(root)?,
            parent_workspace_id: None,
            created_at_ms: now_ms,
        })
        .await?;
    Ok(caps)
}

/// Records an `EXCLUSIVE_WRITE` lease row inside `txn` — the epoch-1
/// lease minted with a workspace.
pub async fn record_lease(txn: &mut dyn KernelTxn, lease: NewWorkspaceLease) -> errors::Result<()> {
    if lease.mode != WorkspaceAccessMode::ExclusiveWrite
        && lease.mode != WorkspaceAccessMode::ReadOnly
    {
        return Err(KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            "local adapter only supports read-only and exclusive-write leases",
        ));
    }
    txn.workspaces().insert_lease(lease).await
}

/// Removes a forked worktree (Git) or directory (copy fork).
pub fn remove(root: &Path, fork_root: &Path) -> errors::Result<()> {
    if git::is_git_repo(root) {
        git::remove_worktree(root, fork_root)
    } else {
        std::fs::remove_dir_all(fork_root).map_err(|e| io("remove", e))
    }
}

fn denied(message: String) -> KernelError {
    KernelError::new(ErrorCode::FailedPrecondition, RetryClass::Never, message)
}

fn io(op: &str, e: std::io::Error) -> KernelError {
    KernelError::new(
        ErrorCode::Unavailable,
        RetryClass::Safe,
        format!("workspace {op} failed"),
    )
    .with_source(e)
}
