//! INT-002: hierarchical children + workspace delegation end-to-end — a real
//! daemon runs a fixture-loop parent that spawns two children; each child
//! gets an isolated Git-worktree fork from the parent's recorded base, writes
//! land per-fork, and the parent merges explicitly (conflict surfaced).
//! Exclusive-write transfer, stale-lease rejection, and the cancel/spawn
//! race run against the same real stack.
#![cfg(unix)]

mod common;

use std::collections::HashMap;
use std::path::Path;
use std::process::Command as Proc;
use std::str::FromStr;
use std::time::Instant;

use common::*;
use domain::generated::contract;
use domain::ids::{CommandId, PrincipalId, RunId, WorkspaceId};
use kernel_store::{KernelStore, TxContext};
use prost::Message;
use serde_json::Value;
use workspace_crate::{coordinator, local};

const SPEC_ID: &str = "01999999-0000-7000-8000-00000000a001";

fn wid(s: &str) -> WorkspaceId {
    s.parse().unwrap()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Proc::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "e2e")
        .env("GIT_AUTHOR_EMAIL", "e2e@test")
        .env("GIT_COMMITTER_NAME", "e2e")
        .env("GIT_COMMITTER_EMAIL", "e2e@test")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn init_repo(dir: &Path) -> String {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-b", "main"]);
    std::fs::write(dir.join("base.txt"), b"base\n").unwrap();
    std::fs::write(dir.join("shared.txt"), b"v0\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", "base"]);
    git(dir, &["rev-parse", "HEAD"])
}

fn commit_all(dir: &Path, msg: &str) -> String {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

/// Encoded `CreateTaskRun` for a child of `parent` sharing the task/session.
fn child_request(
    task_id: &str,
    session_id: &str,
    parent: &str,
    child: &str,
    spec_digest: &str,
) -> Vec<u8> {
    contract::CreateTaskRun {
        task_id: task_id.to_owned(),
        run_id: child.to_owned(),
        session_id: session_id.to_owned(),
        task_kind: "agent".to_owned(),
        task_payload: Vec::new(),
        agent_spec_ref: Some(contract::VersionedRef {
            id: SPEC_ID.to_owned(),
            version: "v1".to_owned(),
            digest: spec_digest.to_owned(),
        }),
        parent_run_id: parent.to_owned(),
        observed_parent_cancellation_epoch: 0,
        requested_profile: "local-trusted".to_owned(),
        workspace_uri: String::new(),
        requested_capabilities: Vec::new(),
        requested_budget: Vec::new(),
    }
    .encode_to_vec()
}

async fn wtxn(daemon: &agentd::bootstrap::Daemon) -> Box<dyn kernel_store::KernelTxn + '_> {
    daemon
        .store()
        .begin_write(TxContext {
            daemon_epoch: daemon.epoch(),
            principal_id: PrincipalId::from_str("00000000-0000-7000-8000-0000000000dd").unwrap(),
            command_id: CommandId::new(daemon.ids().as_ref()),
            correlation_id: None,
        })
        .await
        .expect("write txn")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_parallel_fork_merge() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = make_fixture_bundle(tmp.path());
    let runtime_dir = tmp.path().join("runtime");

    // Parent's Git fixture workspace with a recorded base commit.
    let repo = tmp.path().join("repo");
    let base = init_repo(&repo);

    let parent = "01999999-0000-7000-8000-00000000a101".to_owned();
    let child_a = "01999999-0000-7000-8000-00000000a102".to_owned();
    let child_b = "01999999-0000-7000-8000-00000000a103".to_owned();

    // Child requests need task/session/spec — create the session + spec on a
    // first boot, then restart with the full per-run script map (scripts are
    // snapshotted at boot; the store persists across the restart).
    let (session_id, spec_digest) = {
        let d = boot_daemon(&runtime_dir, vec![bundle.clone()]).await;
        let socket = d.socket_path().to_path_buf();
        let (s, _spec, digest) = make_session_and_spec(&socket, tmp.path()).await;
        d.initiate_shutdown();
        d.wait().await.unwrap();
        (s, digest)
    };

    // Task id fixed so the parent's graph shows children.
    let task_id = "01999999-0000-7000-8000-00000000ee01";
    let spawn_a = format!(
        r#"{{"spawn_agent":{{"child_request":"{}"}}}}"#,
        b64(&child_request(
            task_id,
            &session_id,
            &parent,
            &child_a,
            &spec_digest
        ))
    );
    let spawn_b = format!(
        r#"{{"spawn_agent":{{"child_request":"{}"}}}}"#,
        b64(&child_request(
            task_id,
            &session_id,
            &parent,
            &child_b,
            &spec_digest
        ))
    );
    let parent_script =
        format!("[{spawn_a},{spawn_b},{{\"complete\":{{\"output_ref\":\"e2e://merged\"}}}}]");
    let scripts: HashMap<RunId, String> = HashMap::from([
        (parent.parse().unwrap(), parent_script),
        (child_a.parse().unwrap(), COMPLETE_SCRIPT.to_owned()),
        (child_b.parse().unwrap(), COMPLETE_SCRIPT.to_owned()),
    ]);
    let daemon = boot_daemon_scripts(&runtime_dir, vec![bundle], scripts).await;
    let socket = daemon.socket_path().to_path_buf();

    // Register the parent workspace row (the run binds workspace_uri to it).
    let ws_parent = wid("01999999-0000-7000-8000-00000000b501");
    {
        let mut txn = wtxn(&daemon).await;
        local::record_workspace(&mut *txn, ws_parent, &repo, 1_700_000_000_000)
            .await
            .unwrap();
        txn.commit().await.unwrap();
    }

    let created = cli(
        &socket,
        &[
            "create-run",
            "--session-id",
            &session_id,
            "--task-id",
            task_id,
            "--task-kind",
            "agent",
            "--profile",
            "local-trusted",
            "--run-id",
            &parent,
            "--agent-spec-id",
            SPEC_ID,
            "--spec-version",
            "v1",
            "--spec-digest",
            &spec_digest,
            "--workspace-uri",
            &format!("file://{}", repo.display()),
        ],
    )
    .await;
    assert_eq!(created["outcome_code"], "ok", "{created}");

    // Wait until both children exist (spawned by the parent's loop decisions).
    let deadline = Instant::now() + std::time::Duration::from_millis(30_000);
    loop {
        let a = try_get_run(&socket, &child_a).await;
        let b = try_get_run(&socket, &child_b).await;
        if a.is_some() && b.is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "children never spawned: {a:?} {b:?}"
        );
        tokio::task::yield_now().await;
    }

    // Each child gets an isolated fork pinned at the parent's base revision.
    let ws_a = wid("01999999-0000-7000-8000-00000000b502");
    let ws_b = wid("01999999-0000-7000-8000-00000000b503");
    let fork_a = tmp.path().join("fork-a");
    let fork_b = tmp.path().join("fork-b");
    {
        let mut txn = wtxn(&daemon).await;
        coordinator::fork_for_child(
            &mut *txn,
            daemon.ids().as_ref(),
            &repo,
            ws_parent,
            ws_a,
            &fork_a,
            child_a.parse().unwrap(),
            1_700_000_000_001,
        )
        .await
        .unwrap();
        coordinator::fork_for_child(
            &mut *txn,
            daemon.ids().as_ref(),
            &repo,
            ws_parent,
            ws_b,
            &fork_b,
            child_b.parse().unwrap(),
            1_700_000_000_002,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
    }

    // Forks inherit the exact parent base, not whatever HEAD is later.
    assert_eq!(git(&fork_a, &["rev-parse", "HEAD"]), base);
    assert_eq!(git(&fork_b, &["rev-parse", "HEAD"]), base);
    {
        let mut txn = daemon.store().begin_read().await.unwrap();
        let wa = txn.workspaces().get_workspace(ws_a).await.unwrap().unwrap();
        let wb = txn.workspaces().get_workspace(ws_b).await.unwrap().unwrap();
        assert_eq!(wa.parent_workspace_id, Some(ws_parent));
        assert_eq!(wa.base_revision.as_deref(), Some(base.as_str()));
        assert_eq!(wb.base_revision.as_deref(), Some(base.as_str()));
    }

    // Children make separate, non-conflicting changes in their own forks.
    std::fs::write(fork_a.join("child-a.txt"), b"a\n").unwrap();
    commit_all(&fork_a, "a adds");
    std::fs::write(fork_b.join("child-b.txt"), b"b\n").unwrap();
    commit_all(&fork_b, "b adds");

    // Isolation: parent repo untouched by child writes until explicit merge.
    assert!(!repo.join("child-a.txt").exists());
    assert!(!repo.join("child-b.txt").exists());

    // Parent explicitly merges each child's outputs — clean applies.
    // `merge` uses --no-commit so conflicts can't half-apply; commit between
    // merges so the next starts from a concluded state.
    for (fork, msg) in [(&fork_a, "merge a"), (&fork_b, "merge b")] {
        let mut txn = wtxn(&daemon).await;
        coordinator::merge(&mut *txn, &repo, fork).await.unwrap();
        txn.rollback().await.ok();
        commit_all(&repo, msg);
    }
    assert!(repo.join("child-a.txt").exists());
    assert!(repo.join("child-b.txt").exists());

    // Conflict case: a third fork and the parent both change shared.txt.
    // Leased to child_a — lease ownership only needs a real run row (FK).
    let ws_c = wid("01999999-0000-7000-8000-00000000b504");
    let fork_c = tmp.path().join("fork-c");
    {
        let mut txn = wtxn(&daemon).await;
        coordinator::fork_for_child(
            &mut *txn,
            daemon.ids().as_ref(),
            &repo,
            ws_parent,
            ws_c,
            &fork_c,
            child_a.parse().unwrap(),
            1_700_000_000_003,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
    }
    std::fs::write(fork_c.join("shared.txt"), b"child\n").unwrap();
    commit_all(&fork_c, "child edits shared");
    std::fs::write(repo.join("shared.txt"), b"parent\n").unwrap();
    commit_all(&repo, "parent edits shared");
    {
        let mut txn = wtxn(&daemon).await;
        let err = coordinator::merge(&mut *txn, &repo, &fork_c)
            .await
            .expect_err("conflicting merge must surface");
        assert_eq!(err.code(), errors::codes::ErrorCode::Conflict, "{err}");
        txn.rollback().await.ok();
    }

    // Whole graph terminalizes: children complete, parent completes.
    assert_eq!(wait_terminal(&socket, &child_a).await, RUN_COMPLETED);
    assert_eq!(wait_terminal(&socket, &child_b).await, RUN_COMPLETED);
    assert_eq!(wait_terminal(&socket, &parent).await, RUN_COMPLETED);
    let graph = cli(&socket, &["graph", task_id]).await;
    let runs: Vec<&Value> = graph["runs"].as_array().unwrap().iter().collect();
    assert_eq!(runs.len(), 3, "{graph}");

    daemon.initiate_shutdown();
    daemon.wait().await.expect("clean shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_exclusive_transfer_stale_lease_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    init_repo(&repo);
    let owner = "01999999-0000-7000-8000-00000000c101".to_owned();
    let child_run = "01999999-0000-7000-8000-00000000c102".to_owned();
    let other = "01999999-0000-7000-8000-00000000c103".to_owned();
    let scripts = HashMap::from([
        (owner.parse::<RunId>().unwrap(), COMPLETE_SCRIPT.to_owned()),
        (
            child_run.parse::<RunId>().unwrap(),
            COMPLETE_SCRIPT.to_owned(),
        ),
        (other.parse::<RunId>().unwrap(), COMPLETE_SCRIPT.to_owned()),
    ]);
    let daemon = boot_daemon_scripts(
        &tmp.path().join("runtime"),
        vec![make_fixture_bundle(tmp.path())],
        scripts,
    )
    .await;
    let socket = daemon.socket_path().to_path_buf();
    let (session_id, _spec, digest) = make_session_and_spec(&socket, tmp.path()).await;
    for run_id in [&owner, &child_run, &other] {
        let created = cli(
            &socket,
            &create_run_args(&session_id, run_id, SPEC_ID, &digest),
        )
        .await;
        assert_eq!(created["outcome_code"], "ok", "{created}");
    }
    let owner = owner.parse::<RunId>().unwrap();
    let child = child_run.parse::<RunId>().unwrap();
    let other = other.parse::<RunId>().unwrap();
    let ws = wid("01999999-0000-7000-8000-00000000b510");

    let lease = {
        let mut txn = wtxn(&daemon).await;
        local::record_workspace(&mut *txn, ws, &repo, 1_700_000_000_000)
            .await
            .unwrap();
        let lease = coordinator::acquire_lease(
            &mut *txn,
            daemon.ids().as_ref(),
            ws,
            owner,
            domain::resource::WorkspaceAccessMode::ExclusiveWrite,
            &[],
            Vec::new(),
            1_700_000_000_001,
        )
        .await
        .unwrap();
        txn.commit().await.unwrap();
        lease
    };

    // A second exclusive holder on the same workspace is impossible.
    {
        let mut txn = wtxn(&daemon).await;
        let err = coordinator::acquire_lease(
            &mut *txn,
            daemon.ids().as_ref(),
            ws,
            other,
            domain::resource::WorkspaceAccessMode::ExclusiveWrite,
            &[],
            Vec::new(),
            1_700_000_000_002,
        )
        .await
        .expect_err("second exclusive lease must conflict");
        assert_eq!(err.code(), errors::codes::ErrorCode::Conflict, "{err}");
        txn.rollback().await.ok();
    }

    for id in [
        "01999999-0000-7000-8000-00000000c101",
        "01999999-0000-7000-8000-00000000c102",
        "01999999-0000-7000-8000-00000000c103",
    ] {
        eprintln!(
            "get-run {id}: {:?}",
            try_get_run(&socket, id).await.map(|r| r["state"].as_i64())
        );
    }

    // Exclusive-write transfer moves authority to the child.
    let moved = {
        let mut txn = wtxn(&daemon).await;
        let moved = coordinator::transfer_exclusive(
            &mut *txn,
            lease.lease_id,
            owner,
            child,
            lease.lease_epoch,
            b"delegated".to_vec(),
        )
        .await
        .unwrap();
        if let Err(e) = txn.commit().await {
            // Debug: check which rows exist.
            let mut tx = wtxn(&daemon).await;
            for id in [
                "01999999-0000-7000-8000-00000000c101",
                "01999999-0000-7000-8000-00000000c102",
                "01999999-0000-7000-8000-00000000c103",
            ] {
                let rid: RunId = id.parse().unwrap();
                eprintln!(
                    "run {id} exists: {}",
                    tx.runs().get(rid).await.unwrap().is_some()
                );
            }
            panic!("commit failed: {e}");
        }
        moved
    };

    // The stale parent token is dead; the child's token is live.
    {
        let mut txn = wtxn(&daemon).await;
        let stale = coordinator::verify_write_authority(
            &mut *txn,
            lease.lease_id,
            ws,
            owner,
            lease.lease_epoch,
        )
        .await;
        assert_eq!(
            stale.unwrap_err().code(),
            errors::codes::ErrorCode::FailedPrecondition
        );
        coordinator::verify_write_authority(
            &mut *txn,
            lease.lease_id,
            ws,
            child,
            moved.lease_epoch,
        )
        .await
        .expect("child token is the live authority");
        txn.rollback().await.ok();
    }

    daemon.initiate_shutdown();
    daemon.wait().await.expect("clean shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_cancel_spawn_race() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = make_fixture_bundle(tmp.path());
    let runtime_dir = tmp.path().join("runtime");

    let parent = "01999999-0000-7000-8000-00000000d101".to_owned();
    let child = "01999999-0000-7000-8000-00000000d102".to_owned();
    let parent_script = format!(
        r#"[{{"request_approval":{{"approval_draft":"{}"}}}}]"#,
        b64(b"approval-draft")
    );
    // If the child materializes it also parks at WaitingHuman, so the subtree
    // cancel sweeps it to Cancelled rather than racing to a script_exhausted
    // Fail before the cancellation lands.
    let child_script = format!(
        r#"[{{"request_approval":{{"approval_draft":"{}"}}}}]"#,
        b64(b"child-approval-draft")
    );
    let scripts = HashMap::from([
        (parent.parse::<RunId>().unwrap(), parent_script),
        (child.parse::<RunId>().unwrap(), child_script),
    ]);
    let daemon = boot_daemon_scripts(&runtime_dir, vec![bundle], scripts).await;
    let socket = daemon.socket_path().to_path_buf();
    let (session_id, _spec, digest) = make_session_and_spec(&socket, tmp.path()).await;

    let task_id = "01999999-0000-7000-8000-00000000ee02";
    // Parent waits on a human approval forever, so the race window is stable.
    let created = cli(
        &socket,
        &[
            "create-run",
            "--session-id",
            &session_id,
            "--task-id",
            task_id,
            "--task-kind",
            "agent",
            "--profile",
            "local-trusted",
            "--run-id",
            &parent,
            "--agent-spec-id",
            SPEC_ID,
            "--spec-version",
            "v1",
            "--spec-digest",
            &digest,
        ],
    )
    .await;
    assert_eq!(created["outcome_code"], "ok", "{created}");

    // Wait until the parent is bound+claimed (it is Running or WaitingHuman).
    let deadline = Instant::now() + std::time::Duration::from_millis(30_000);
    loop {
        let run = cli(&socket, &["get-run", &parent]).await;
        let state = run["state"].as_i64().unwrap_or(0);
        if state == 3 || state == 6 {
            break;
        }
        assert!(Instant::now() < deadline, "parent never started: {run}");
        tokio::task::yield_now().await;
    }

    // Race `CancelRun` on the parent with a racing child creation. Whatever
    // order lands, the outcome must be consistent:
    //  - cancel first: create fails on a stale observed epoch / cancelled parent;
    //  - create first: the subtree cancellation takes the child down too.
    let child_request = contract::CreateTaskRun {
        task_id: task_id.to_owned(),
        run_id: child.clone(),
        session_id: session_id.clone(),
        task_kind: "agent".to_owned(),
        agent_spec_ref: Some(contract::VersionedRef {
            id: SPEC_ID.to_owned(),
            version: "v1".to_owned(),
            digest: digest.clone(),
        }),
        parent_run_id: parent.clone(),
        observed_parent_cancellation_epoch: 0,
        requested_profile: "local-trusted".to_owned(),
        ..Default::default()
    }
    .encode_to_vec();
    let b64_child = b64(&child_request);
    let _ = b64_child;

    let cancel_args: Vec<String> = ["cancel-run", &parent, "--reason", "race"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let cancel = agentctl::run(&cancel_args, &socket);
    let spawn_args: Vec<String> = [
        "create-run",
        "--session-id",
        &session_id,
        "--task-id",
        task_id,
        "--task-kind",
        "agent",
        "--profile",
        "local-trusted",
        "--run-id",
        &child,
        "--parent-run-id",
        &parent,
        "--agent-spec-id",
        SPEC_ID,
        "--spec-version",
        "v1",
        "--spec-digest",
        &digest,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let spawn = agentctl::run(&spawn_args, &socket);
    let (cancel_out, spawn_out) = tokio::join!(cancel, spawn);
    let spawn_out = spawn_out.unwrap_or_else(|e| {
        // Rejected spawn is a legitimate race outcome — encode as JSON.
        serde_json::json!({"outcome_code": e.0})
    });
    let _ = cancel_out;

    // Parent terminalizes.
    let parent_state = wait_terminal(&socket, &parent).await;
    if parent_state != 11 {
        let ev = cli(
            &socket,
            &[
                "events",
                "read",
                "--stream-key",
                &format!("run/{parent}"),
                "--limit",
                "100",
            ],
        )
        .await;
        panic!("expected Cancelled, got {parent_state}; events: {ev}");
    }

    // Invariant: either the child never materialized, or it was taken down
    // by the subtree cancellation.
    let child_view = try_get_run(&socket, &child).await;
    if child_view.is_some() {
        let child_state = wait_terminal(&socket, &child).await;
        assert_eq!(child_state, 11, "child survived the subtree cancel");
        assert_eq!(spawn_out["outcome_code"], "ok");
    } else {
        assert_ne!(
            spawn_out["outcome_code"], "ok",
            "spawn raced into a cancelled parent: {spawn_out}"
        );
    }
    daemon.initiate_shutdown();
    daemon.wait().await.expect("clean shutdown");
}
