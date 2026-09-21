//! Workspace Coordinator (WRK-003): lease acquisition, exclusive-write
//! transfer, parallel-fork defaults, and explicit merge.
//!
//! Write authority lives in `workspace_leases` rows. The schema's partial
//! unique index makes a second active `EXCLUSIVE_WRITE` lease on the same
//! workspace physically impossible; every mutation moves the epoch so a
//! stale handle is dead the moment a transfer commits.
//!
//! T0 truth: the local adapter mediates writes through this coordinator,
//! but a hostile process that already holds raw host file descriptors is
//! not contained — `AdapterCapabilities::is_security_boundary` is `false`
//! and that label flows to callers. T2+ enforcement requires an adapter
//! that actually sandboxes.
#![forbid(unsafe_code)]

use std::path::Path;

use domain::ids::{LeaseId, RunId, WorkspaceId};
use domain::provider::IdProvider;
use domain::resource::{LeaseEnforcementState, WorkspaceAccessMode};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::models::{LeasePatch, NewWorkspaceLease, WorkspaceLeaseRow};
use kernel_store::txn::KernelTxn;

use crate::{git, local};

fn coordinator_error(code: ErrorCode, msg: impl Into<String>) -> KernelError {
    KernelError::new(code, RetryClass::Never, msg.into())
}

/// Capabilities a SHARED_COORDINATED_WRITE path must advertise before the
/// coordinator will grant shared write authority.
pub mod shared_write_caps {
    /// Adapter mediates concurrent writes with locking.
    pub const LOCKING: &str = "workspace.locking";
    /// Write checks a version/expected-revision token.
    pub const VERSIONED_WRITE: &str = "workspace.versioned_write";
    /// Merge/apply reports conflicts explicitly.
    pub const CONFLICT_REPORT: &str = "workspace.conflict_report";
}

/// Acquire a lease of `mode` on `workspace_id` for `owner`.
///
/// `SHARED_COORDINATED_WRITE` requires the adapter to advertise
/// locking + versioned-write + conflict-report capabilities; the MVP
/// local adapter has none, so shared write is fail-closed. Exclusive
/// acquisition races resolve through the partial unique index (loser
/// gets `Conflict`).
#[allow(clippy::too_many_arguments)]
pub async fn acquire_lease(
    txn: &mut dyn KernelTxn,
    ids: &dyn IdProvider,
    workspace_id: WorkspaceId,
    owner: RunId,
    mode: WorkspaceAccessMode,
    adapter_capabilities: &[String],
    delegated_from: Vec<u8>,
    now_ms: i64,
) -> errors::Result<WorkspaceLeaseRow> {
    if mode == WorkspaceAccessMode::SharedCoordinatedWrite {
        let supported = [
            shared_write_caps::LOCKING,
            shared_write_caps::VERSIONED_WRITE,
            shared_write_caps::CONFLICT_REPORT,
        ]
        .iter()
        .all(|cap| adapter_capabilities.iter().any(|c| c == cap));
        if !supported {
            return Err(coordinator_error(
                ErrorCode::FailedPrecondition,
                "SHARED_COORDINATED_WRITE requires an adapter with locking, \
                 versioned-write, and conflict-report capabilities",
            ));
        }
    }
    if mode == WorkspaceAccessMode::Unspecified {
        return Err(coordinator_error(
            ErrorCode::InvalidArgument,
            "workspace access mode unspecified",
        ));
    }
    let lease_id = LeaseId::new(ids);
    txn.workspaces()
        .insert_lease(NewWorkspaceLease {
            lease_id,
            workspace_id,
            owner_run_id: owner,
            mode,
            lease_epoch: 1,
            enforcement_state: LeaseEnforcementState::Active,
            delegated_from: delegated_from.clone(),
            created_at_ms: now_ms,
        })
        .await?;
    Ok(WorkspaceLeaseRow {
        lease_id,
        workspace_id,
        owner_run_id: owner,
        mode,
        lease_epoch: 1,
        enforcement_state: LeaseEnforcementState::Active,
        delegated_from,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    })
}

/// Transfer exclusive write authority `from` -> `to` at `expected_epoch`.
///
/// One `cas_lease` does all of: verify persisted epoch, re-point the
/// owner, and advance the epoch. Any copy of the lease token at the old
/// epoch fails immediately — a stale-owner write through the coordinator
/// cannot pass the epoch check once this commits. Returns
/// `FailedPrecondition` when the lease isn't an active exclusive lease
/// owned by `from` at `expected_epoch`.
pub async fn transfer_exclusive(
    txn: &mut dyn KernelTxn,
    lease_id: LeaseId,
    from: RunId,
    to: RunId,
    expected_epoch: u64,
    delegation_proof: Vec<u8>,
) -> errors::Result<WorkspaceLeaseRow> {
    let lease = txn
        .workspaces()
        .get_lease(lease_id)
        .await?
        .ok_or_else(|| coordinator_error(ErrorCode::NotFound, "lease not found"))?;
    if lease.mode != WorkspaceAccessMode::ExclusiveWrite
        || lease.enforcement_state != LeaseEnforcementState::Active
        || lease.owner_run_id != from
        || lease.lease_epoch != expected_epoch
    {
        return Err(coordinator_error(
            ErrorCode::FailedPrecondition,
            "lease is not an active exclusive lease owned by the transferring run \
             at the expected epoch",
        ));
    }
    let moved = txn
        .workspaces()
        .cas_lease(
            lease_id,
            expected_epoch,
            LeasePatch {
                owner_run_id: Some(to),
                lease_epoch: Some(expected_epoch.saturating_add(1)),
                delegated_from: Some(delegation_proof),
                ..Default::default()
            },
        )
        .await?;
    if !moved {
        return Err(coordinator_error(
            ErrorCode::Conflict,
            "lease epoch moved during transfer",
        ));
    }
    txn.workspaces()
        .get_lease(lease_id)
        .await?
        .ok_or_else(|| coordinator_error(ErrorCode::Internal, "lease vanished mid-transfer"))
}

/// Revoke a lease at `expected_epoch` (release path — owner drops
/// authority without transferring it).
pub async fn revoke_lease(
    txn: &mut dyn KernelTxn,
    lease_id: LeaseId,
    expected_epoch: u64,
) -> errors::Result<()> {
    let moved = txn
        .workspaces()
        .cas_lease(
            lease_id,
            expected_epoch,
            LeasePatch {
                enforcement_state: Some(LeaseEnforcementState::Revoked),
                lease_epoch: Some(expected_epoch.saturating_add(1)),
                ..Default::default()
            },
        )
        .await?;
    if !moved {
        return Err(coordinator_error(
            ErrorCode::FailedPrecondition,
            "lease epoch moved during revoke",
        ));
    }
    Ok(())
}

/// Verify a lease token is the live exclusive authority for `workspace_id`
/// — the gate every coordinator-mediated write passes through.
pub async fn verify_write_authority(
    txn: &mut dyn KernelTxn,
    lease_id: LeaseId,
    workspace_id: WorkspaceId,
    owner: RunId,
    epoch: u64,
) -> errors::Result<()> {
    let lease = txn
        .workspaces()
        .get_lease(lease_id)
        .await?
        .ok_or_else(|| coordinator_error(ErrorCode::NotFound, "lease not found"))?;
    if lease.workspace_id != workspace_id
        || lease.owner_run_id != owner
        || lease.lease_epoch != epoch
        || lease.enforcement_state != LeaseEnforcementState::Active
        || lease.mode != WorkspaceAccessMode::ExclusiveWrite
    {
        return Err(coordinator_error(
            ErrorCode::FailedPrecondition,
            "stale or non-exclusive lease token",
        ));
    }
    Ok(())
}

/// A child forked for parallel write work: isolated workspace + its own
/// exclusive lease on the fork. The child never touches the parent's
/// workspace — authority is never amplified sideways.
#[allow(clippy::too_many_arguments)]
pub async fn fork_for_child(
    txn: &mut dyn KernelTxn,
    ids: &dyn IdProvider,
    parent_root: &Path,
    parent_workspace_id: WorkspaceId,
    child_workspace_id: WorkspaceId,
    child_root: &Path,
    child_owner: RunId,
    now_ms: i64,
) -> errors::Result<WorkspaceLeaseRow> {
    local::fork(
        txn,
        parent_root,
        parent_workspace_id,
        child_workspace_id,
        child_root,
        now_ms,
    )
    .await?;
    acquire_lease(
        txn,
        ids,
        child_workspace_id,
        child_owner,
        WorkspaceAccessMode::ExclusiveWrite,
        &[],
        Vec::new(),
        now_ms,
    )
    .await
}

/// Result of an explicit merge of a source workspace into a target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeOutcome {
    /// Files that changed on the target.
    pub applied_files: Vec<String>,
}

/// Explicit merge/apply of a Git-worktree child into the parent repo.
/// Conflicts surface as a `Conflict` error carrying the unmerged file
/// list — nothing is auto-resolved or silently dropped. The merge is
/// aborted on conflict so the target returns to its pre-merge state.
pub async fn merge(
    txn: &mut dyn KernelTxn,
    target_root: &Path,
    source_root: &Path,
) -> errors::Result<MergeOutcome> {
    let _ = txn; // merge is a filesystem operation; the caller records
    if !git::is_git_repo(target_root) || !git::is_git_repo(source_root) {
        return Err(coordinator_error(
            ErrorCode::FailedPrecondition,
            "merge requires Git-worktree workspaces; copy forks are applied by \
             the caller with content-addressed artifacts",
        ));
    }
    let source_head = git::head_revision(source_root)?;
    // Fetch the source commit object into the target's object store, then
    // merge it. Worktrees share the object db via the common dir, so
    // `git fetch <source_root> <sha>` is a local no-network fetch.
    run_git(
        target_root,
        &["fetch", &source_root.to_string_lossy(), &source_head],
    )?;
    let merge = run_git(
        target_root,
        &["merge", "--no-commit", "--no-ff", "FETCH_HEAD"],
    );
    match merge {
        Ok(_) => Ok(MergeOutcome {
            applied_files: git::list_files(target_root)?
                .into_iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
        }),
        Err(err) => {
            let conflicts = conflicted_paths(target_root).unwrap_or_default();
            // Best-effort rollback: leave the target clean rather than
            // half-merged.
            let _ = run_git(target_root, &["merge", "--abort"]);
            Err(KernelError::new(
                ErrorCode::Conflict,
                RetryClass::Never,
                format!("merge conflicted on {conflicts:?}: {}", err.message()),
            ))
        }
    }
}

fn run_git(dir: &Path, args: &[&str]) -> errors::Result<()> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| coordinator_error(ErrorCode::Internal, format!("git spawn: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(coordinator_error(
        ErrorCode::Internal,
        String::from_utf8_lossy(&output.stderr).trim().to_owned(),
    ))
}

fn conflicted_paths(dir: &Path) -> errors::Result<Vec<String>> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output()
        .map_err(|e| coordinator_error(ErrorCode::Internal, format!("git spawn: {e}")))?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.to_owned())
        .collect())
}
