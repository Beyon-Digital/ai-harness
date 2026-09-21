//! `agentd adapter` lifecycle end-to-end: install copies+verifies a bundle
//! under `installed-adapters/`, `check` runs the spawn smoke, enable/
//! disable gate boot registration, and a daemon booted against the runtime
//! dir picks up installed bundles without `--adapter-bundle`.
#![cfg(unix)]

mod common;

use std::collections::HashMap;
use std::path::Path;

use agentd::adapters;
use common::*;

/// install → boot (no config bundles beyond the effect fixture) → the
/// installed loop adapter is registered and a run completes through it.
#[tokio::test]
async fn adapter_install_then_boot_registers_it() {
    let dir = tempfile::tempdir().unwrap();
    let runtime_dir = dir.path().join("runtime");
    let bundle = make_fixture_bundle(dir.path());

    let entry = adapters::install(dir.path(), Path::new(""), false, 0).await;
    assert!(entry.is_err(), "install must reject a non-bundle dir");

    let entry = adapters::install(&runtime_dir, &bundle, true, 1_700_000_000_000)
        .await
        .expect("install");
    assert_eq!(entry.adapter_id, "01905c5e-0000-7000-8000-a9e97100f1a1");
    assert_eq!(entry.check.as_deref(), Some("pass"));
    assert!(
        runtime_dir
            .join(adapters::INSTALLED_DIR)
            .join(&entry.dir_name)
            .is_dir()
    );

    // Boot WITHOUT the loop bundle in adapter_bundles — it must come from
    // the installed set. (default.yaml's local-trusted needs fixture-loop.)
    let run_id = "01905c5e-0000-7000-8000-00ada97e0001";
    let mut scripts = HashMap::new();
    scripts.insert(run_id.parse().unwrap(), COMPLETE_SCRIPT.to_owned());
    let _daemon = boot_daemon_scripts(&runtime_dir, Vec::new(), scripts).await;

    let socket = runtime_dir.join("control.sock");
    let (session_id, spec_id, digest) = make_session_and_spec(&socket, dir.path()).await;
    cli(
        &socket,
        &create_run_args(&session_id, run_id, &spec_id, &digest),
    )
    .await;
    assert_eq!(wait_terminal(&socket, run_id).await, RUN_COMPLETED);
}

/// `check` spawns the bundle entrypoint and validates bootstrap→hello→ping.
#[tokio::test]
async fn adapter_check_smoke() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = make_fixture_bundle(dir.path());
    let report = adapters::check_bundle(&bundle).await.expect("check");
    assert_eq!(report.result, "pass");
    assert!(report.implemented_ports.iter().any(|p| p == "agent_loop"));

    // A corrupt bundle fails the check rather than reporting pass.
    std::fs::write(bundle.join("fixture-agent-loop"), b"corrupt").unwrap();
    let err = adapters::check_bundle(&bundle).await;
    assert!(
        err.is_err(),
        "mutated bundle must fail verification: {err:?}"
    );
}

/// enable/disable/remove mutate the index; boot skips disabled bundles.
#[tokio::test]
async fn adapter_enable_disable_remove() {
    let dir = tempfile::tempdir().unwrap();
    let runtime_dir = dir.path().join("runtime");
    let bundle = make_fixture_bundle(dir.path());
    let entry = adapters::install(&runtime_dir, &bundle, false, 0)
        .await
        .expect("install");
    let id_prefix = &entry.adapter_id[..8];

    let e = adapters::set_enabled(&runtime_dir, id_prefix, None, false).unwrap();
    assert!(!e.enabled);
    assert!(
        adapters::enabled_bundle_dirs(&runtime_dir)
            .unwrap()
            .is_empty()
    );
    let e = adapters::set_enabled(&runtime_dir, id_prefix, None, true).unwrap();
    assert!(e.enabled);
    assert_eq!(
        adapters::enabled_bundle_dirs(&runtime_dir).unwrap().len(),
        1
    );

    adapters::remove(&runtime_dir, id_prefix, None).unwrap();
    assert!(adapters::load_index(&runtime_dir).unwrap().is_empty());
    assert!(
        !runtime_dir
            .join(adapters::INSTALLED_DIR)
            .join(&entry.dir_name)
            .exists()
    );
    assert!(adapters::remove(&runtime_dir, id_prefix, None).is_err());
}
