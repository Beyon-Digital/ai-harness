//! Git worktree primitives for the local workspace adapter.
//!
//! All git operations shell out to `git` with explicit `--` end-of-options
//! markers; no path is ever interpolated into a shell string.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};

/// Whether `dir` is inside a Git work tree.
pub fn is_git_repo(dir: &Path) -> bool {
    Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
}

/// The exact current HEAD revision (`git rev-parse HEAD`).
pub fn head_revision(dir: &Path) -> errors::Result<String> {
    let out = Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .map_err(|e| git_err("rev-parse", e.to_string()))?;
    if !out.status.success() {
        return Err(git_err("rev-parse", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// `git worktree add --detach <target> <revision>` — the fork starts at the
/// parent's exact base revision, detached, with no branch mutation.
pub fn add_worktree(repo: &Path, revision: &str, target: &Path) -> errors::Result<()> {
    let out = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["worktree", "add", "--detach"])
        .arg(target)
        .arg(revision)
        .output()
        .map_err(|e| git_err("worktree add", e.to_string()))?;
    if !out.status.success() {
        return Err(git_err(
            "worktree add",
            String::from_utf8_lossy(&out.stderr),
        ));
    }
    Ok(())
}

/// `git worktree remove --force <target>` — teardown of a forked worktree.
pub fn remove_worktree(repo: &Path, target: &Path) -> errors::Result<()> {
    let out = Command::new("git")
        .args(["-C"])
        .arg(repo)
        .args(["worktree", "remove", "--force"])
        .arg(target)
        .output()
        .map_err(|e| git_err("worktree remove", e.to_string()))?;
    if !out.status.success() {
        return Err(git_err(
            "worktree remove",
            String::from_utf8_lossy(&out.stderr),
        ));
    }
    Ok(())
}

/// Revision of `dir`'s own HEAD — a worktree's recorded base.
pub fn worktree_base(dir: &Path) -> errors::Result<String> {
    head_revision(dir)
}

/// Lists tracked files relative to the worktree root (`git ls-files`),
/// plus untracked non-ignored files (`--others --exclude-standard`).
pub fn list_files(dir: &Path) -> errors::Result<Vec<PathBuf>> {
    let out = Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .map_err(|e| git_err("ls-files", e.to_string()))?;
    if !out.status.success() {
        return Err(git_err("ls-files", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// Copies a directory tree (the non-Git fork path — weaker capability:
/// copy-on-create, no revision semantics).
pub fn copy_tree(src: &Path, dst: &Path) -> errors::Result<()> {
    if !src.is_dir() {
        return Err(git_err(
            "copy",
            format!("{} is not a directory", src.display()),
        ));
    }
    std::fs::create_dir_all(dst).map_err(|e| git_err("mkdir", e.to_string()))?;
    for entry in std::fs::read_dir(src).map_err(|e| git_err("read_dir", e.to_string()))? {
        let entry = entry.map_err(|e| git_err("entry", e.to_string()))?;
        let dest = dst.join(entry.file_name());
        let meta =
            std::fs::symlink_metadata(entry.path()).map_err(|e| git_err("stat", e.to_string()))?;
        if meta.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else if meta.is_file() {
            std::fs::copy(entry.path(), &dest).map_err(|e| git_err("copy file", e.to_string()))?;
        }
        // symlinks are skipped — a copied link could point outside the fork.
    }
    Ok(())
}

fn git_err(op: &str, detail: impl ToString) -> KernelError {
    KernelError::new(
        ErrorCode::Unavailable,
        RetryClass::Never,
        format!("git {op} failed: {}", detail.to_string().trim()),
    )
}
