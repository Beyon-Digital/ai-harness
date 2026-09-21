# Task WRK-002 Report

- **Spec:** agentd-microkernel-mvp
- **Task:** WRK-002 — Local workspace adapter + git worktree fork
- **Status:** DONE
- **Commits:** `9e7b4b1` — `feat(workspace): local adapter + git worktree fork at recorded base revision [WRK-002]`
- **Branch:** `devin/1789944697-support-wave`

## What was implemented

- `git.rs`: `is_git_repo`, `head_revision`, `add_worktree` (detached at
  an explicit revision), `remove_worktree`, `list_files`
  (`ls-files -z --cached --others --exclude-standard`), `copy_tree`
  (recursive, symlink-free).
- `local.rs`: `resolve_relative` normalizes relative paths — rejects
  empty/absolute/backslash/`..`/non-Normal components and canonicalizes
  the deepest existing ancestor inside the canonical root so symlink
  traversal is denied. `read`/`write`/`list` operate only through it.
- `record_workspace` persists the workspace row with kind +
  `base_revision` (exact `HEAD` for Git); `record_lease` accepts only
  `ReadOnly`/`ExclusiveWrite`.
- `fork()`: reads the *parent workspace row's* recorded `base_revision`
  (not current HEAD — spec AC: fork starts at parent base revision) and
  creates a detached worktree child of kind `git-worktree`; non-Git
  parents get a `copy-fork` child with honestly weaker
  `AdapterCapabilities` (`is_security_boundary: false`, no git semantic).
- `remove()` tears down worktrees via `git worktree remove`.

## Evidence

`cargo test -p workspace` (4 tests): CRUD + normalized listing; Git fork
materializes the parent workspace's recorded base revision even after
HEAD advanced; non-Git fork reports `copy-fork` capability label;
absolute/`..`/encoded-backslash traversal and symlink escapes denied.
