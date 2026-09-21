//! ADP-005 + ADP-006 end-to-end: the real fixture binary over the real
//! protocol, conformance report persistence, and the resolver gate.
//!
//! (`tests/adapter_conformance.rs` in the task layout resolves here —
//! the fixture crate is where `CARGO_BIN_EXE` is defined.)

use std::path::Path;

use adapter_registry::capabilities::SandboxTier;
use adapter_registry::conformance::{ConformanceReport, persist_report};
use adapter_registry::registry::{compute_bundle_digest, register, verify_for_spawn, write_lock};
use adapter_registry::resolver::{Candidate, PortRequirement, resolve};
use adapter_registry::{manifest_digest, parse_manifest};
use domain::ids::{AdapterId, CommandId, DaemonInstanceId, PrincipalId};
use domain::security::{ConformanceState, TrustState};
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use tempfile::TempDir;
use testkit::conformance::{FixtureBinary, effect_adapter_pack, process_protocol_pack};
use testkit::ids::DeterministicIds;

const SEED: i64 = 1_700_000_000_000;
const BIN: &str = env!("CARGO_BIN_EXE_fixture-effect-adapter");
const MANIFEST: &str = include_str!("../adapter.manifest.json");
const ADAPTER_ID: &str = "01905c5e-0000-7000-8000-e11ec7ad01ef";

/// Stages a bundle dir: manifest + the compiled fixture binary + lock.
fn stage_bundle(dir: &Path) {
    std::fs::write(dir.join("adapter.manifest.json"), MANIFEST).expect("manifest");
    std::fs::copy(BIN, dir.join("fixture-effect-adapter")).expect("binary");
    write_lock(dir).expect("lock");
}

async fn open() -> (SqliteKernelStore, u64, TempDir) {
    let dir = tempfile::tempdir().expect("tmp");
    let store = SqliteKernelStore::open(StoreConfig {
        path: dir.path().join("kernel.db"),
        pool_max_connections: 4,
        busy_timeout_ms: 5_000,
    })
    .await
    .expect("store");
    let ids = DeterministicIds::new(SEED);
    let fence = store
        .acquire_daemon_fence(DaemonInstanceId::new(&ids))
        .await
        .expect("fence");
    (store, fence.epoch.0, dir)
}

async fn txn<'s>(store: &'s SqliteKernelStore, epoch: u64) -> Box<dyn KernelTxn + 's> {
    let ids = DeterministicIds::new(SEED + 1);
    store
        .begin_write(TxContext {
            daemon_epoch: epoch,
            principal_id: PrincipalId::new(&ids),
            command_id: CommandId::new(&ids),
            correlation_id: None,
        })
        .await
        .expect("txn")
}

#[tokio::test]
async fn fixture_effect_adapter_conformance_pass() {
    let bundle = tempfile::tempdir().expect("bundle");
    stage_bundle(bundle.path());
    let manifest = parse_manifest(MANIFEST.as_bytes()).expect("manifest parses");
    let digest = compute_bundle_digest(bundle.path(), &manifest_digest(MANIFEST.as_bytes()))
        .expect("digest");

    let fx = FixtureBinary {
        executable: bundle.path().join("fixture-effect-adapter"),
        adapter_id: manifest.id.parse::<AdapterId>().unwrap(),
        adapter_version: manifest.version.clone(),
        bundle_digest: digest.clone(),
        env: vec![],
    };

    let mut cases = process_protocol_pack(&fx).await;
    let store_file = tempfile::tempdir().expect("store dir");
    cases.extend(
        effect_adapter_pack(&fx, &store_file.path().join("s.json").to_string_lossy()).await,
    );
    assert!(
        cases.iter().all(|c| c.passed),
        "all conformance cases must pass: {cases:#?}"
    );
    assert_eq!(cases.len(), 6);

    // persist the pass report; resolver with the gate on must select it
    let (store, epoch, _d) = open().await;
    let mut tx = txn(&store, epoch).await;
    let row = register(&mut *tx, bundle.path(), TrustState::Trusted, 1)
        .await
        .expect("register");
    assert_eq!(row.bundle_digest, digest);
    let report = ConformanceReport::from_cases(
        row.adapter_id,
        &row.version,
        &row.bundle_digest,
        cases
            .into_iter()
            .map(|c| adapter_registry::CaseResult {
                test_id: c.test_id,
                passed: c.passed,
                details: c.details,
            })
            .collect(),
        1_700_000_000_000,
    );
    persist_report(&mut *tx, &report).await.expect("persist");
    tx.commit().await.expect("commit");

    let mut tx = txn(&store, epoch).await;
    let stored = tx
        .adapters()
        .get_registration(row.adapter_id, &row.version, &row.bundle_digest)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(stored.conformance_state, ConformanceState::Passed);

    let req = PortRequirement {
        port_id: "effect.execute".to_owned(),
        port_version: 1,
        required_capabilities: vec!["provider_idempotency".to_owned()],
        sandbox_tier: SandboxTier::T0,
        pin_adapter_id: None,
        require_conformance_passed: true,
    };
    let resolved = resolve(&req, &[Candidate::decode(stored).unwrap()], &[])
        .expect("passing adapter resolves under the gate");
    assert_eq!(resolved.adapter_id.to_string(), ADAPTER_ID);
}

#[tokio::test]
async fn lying_capability_declaration_fails() {
    // A bundle that declares the capability but whose binary cannot even
    // speak the protocol — every case fails and the resolver gate drops it.
    let bundle = tempfile::tempdir().expect("bundle");
    std::fs::write(
        bundle.path().join("adapter.manifest.json"),
        MANIFEST.replace(
            "\"entrypoint\": \"fixture-effect-adapter\"",
            "\"entrypoint\": \"silent\"",
        ),
    )
    .expect("manifest");
    // /bin/true is not a real binary on macOS — a shell stub is portable.
    std::fs::write(bundle.path().join("silent"), "#!/bin/sh\nexit 0\n").expect("binary");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(bundle.path().join("silent"))
            .expect("meta")
            .permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(bundle.path().join("silent"), perms).expect("chmod");
    }
    write_lock(bundle.path()).expect("lock");

    let fx = FixtureBinary {
        executable: bundle.path().join("silent"),
        adapter_id: ADAPTER_ID.parse().unwrap(),
        adapter_version: "0.1.0".to_owned(),
        bundle_digest: "sha256:any".to_owned(),
        env: vec![],
    };
    let cases = process_protocol_pack(&fx).await;
    assert!(
        cases.iter().all(|c| !c.passed),
        "a liar must fail every case"
    );

    let (store, epoch, _d) = open().await;
    let mut tx = txn(&store, epoch).await;
    let row = register(&mut *tx, bundle.path(), TrustState::Trusted, 1)
        .await
        .expect("register");
    let report = ConformanceReport::from_cases(
        row.adapter_id,
        &row.version,
        &row.bundle_digest,
        cases
            .into_iter()
            .map(|c| adapter_registry::CaseResult {
                test_id: c.test_id,
                passed: c.passed,
                details: c.details,
            })
            .collect(),
        1,
    );
    assert_eq!(report.result, "fail");
    persist_report(&mut *tx, &report).await.expect("persist");
    tx.commit().await.expect("commit");

    let mut tx = txn(&store, epoch).await;
    let stored = tx
        .adapters()
        .get_registration(row.adapter_id, &row.version, &row.bundle_digest)
        .await
        .expect("get")
        .expect("row");
    assert_eq!(stored.conformance_state, ConformanceState::Failed);
    let req = PortRequirement {
        port_id: "effect.execute".to_owned(),
        port_version: 1,
        required_capabilities: vec![],
        sandbox_tier: SandboxTier::T0,
        pin_adapter_id: None,
        require_conformance_passed: false,
    };
    assert!(
        resolve(&req, &[Candidate::decode(stored).unwrap()], &[]).is_err(),
        "a Failed registration is never eligible, even without the review gate"
    );
}

#[tokio::test]
async fn bundle_digest_mutation_invalidates_prior_report() {
    let bundle = tempfile::tempdir().expect("bundle");
    stage_bundle(bundle.path());
    let (store, epoch, _d) = open().await;
    let mut tx = txn(&store, epoch).await;
    let row = register(&mut *tx, bundle.path(), TrustState::Trusted, 1)
        .await
        .expect("register");
    let report = ConformanceReport::from_cases(
        row.adapter_id,
        &row.version,
        &row.bundle_digest,
        vec![adapter_registry::CaseResult {
            test_id: "synthetic".to_owned(),
            passed: true,
            details: String::new(),
        }],
        1,
    );
    persist_report(&mut *tx, &report).await.expect("persist");
    tx.commit().await.expect("commit");

    // Mutate the bundle binary: identity changes, report no longer applies.
    std::fs::write(bundle.path().join("extra.bin"), "tamper").expect("tamper");
    write_lock(bundle.path()).expect("relock");
    let md = manifest_digest(MANIFEST.as_bytes());
    let new_digest = compute_bundle_digest(bundle.path(), &md).expect("new digest");
    assert_ne!(new_digest, row.bundle_digest);

    let mut tx = txn(&store, epoch).await;
    assert!(
        tx.adapters()
            .get_conformance_report(row.adapter_id, &row.version, &new_digest)
            .await
            .expect("lookup")
            .is_none(),
        "the old report cannot cover a mutated bundle"
    );
    // And spawning under the new digest is impossible: it isn't registered.
    let err = verify_for_spawn(
        &mut *tx,
        row.adapter_id,
        &row.version,
        &new_digest,
        bundle.path(),
    )
    .await
    .expect_err("unregistered digest rejected");
    assert_eq!(err.code(), errors::codes::ErrorCode::NotFound);
}
