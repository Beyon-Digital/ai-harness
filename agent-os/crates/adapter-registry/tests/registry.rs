//! ADP-003 + ADP-004: content-addressed registry and deterministic
//! resolver behavior.

use adapter_registry::capabilities::{CapabilitySet, SandboxTier};
use adapter_registry::manifest::PortImpl;
use adapter_registry::registry::{compute_bundle_digest, register, verify_for_spawn, write_lock};
use adapter_registry::resolver::{Candidate, PortRequirement, resolve};
use domain::ids::{AdapterId, CommandId, DaemonInstanceId, PrincipalId};
use domain::security::{ConformanceState, TrustState};
use errors::codes::ErrorCode;
use kernel_store::models::AdapterRegistrationRow;
use kernel_store::{KernelStore, KernelTxn, TxContext};
use kernel_store_sqlite::{SqliteKernelStore, StoreConfig};
use tempfile::TempDir;
use testkit::ids::DeterministicIds;

const SEED: i64 = 1_700_000_000_000;
const ADAPTER_UUID: &str = "01905c5e-0000-7000-8000-000000000001";
const OTHER_UUID: &str = "01905c5e-0000-7000-8000-000000000002";

fn manifest(id: &str) -> String {
    format!(
        r#"{{
          "manifest_version": 1,
          "id": "{id}",
          "version": "1.0.0",
          "kind": "adapter",
          "runtime": {{"type": "process", "entrypoint": "run.sh"}},
          "implements": ["effect.execute@1"],
          "requested_capabilities": {{"model": ["status_lookup", "provider_idempotency"]}}
        }}"#
    )
}

fn make_bundle(dir: &TempDir, id: &str, entrypoint_content: &str) {
    std::fs::write(dir.path().join("adapter.manifest.json"), manifest(id)).expect("manifest");
    std::fs::write(dir.path().join("run.sh"), entrypoint_content).expect("entrypoint");
    write_lock(dir.path()).expect("lock");
}

async fn store() -> (SqliteKernelStore, u64, TempDir) {
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

async fn txn(store: &SqliteKernelStore, epoch: u64) -> Box<dyn KernelTxn + '_> {
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
async fn register_persists_content_addressed_identity() {
    let bundle = tempfile::tempdir().expect("bundle");
    make_bundle(&bundle, ADAPTER_UUID, "#!/bin/sh\nexit 0\n");
    let (store, epoch, _d) = store().await;
    let mut tx = txn(&store, epoch).await;
    let row = register(&mut *tx, bundle.path(), TrustState::Trusted, 1000)
        .await
        .expect("register");
    assert!(row.bundle_digest.starts_with("sha256:"));
    assert_eq!(row.conformance_state, ConformanceState::Untested);
    tx.commit().await.expect("commit");
}

#[tokio::test]
async fn mutating_bundle_changes_digest() {
    let bundle = tempfile::tempdir().expect("bundle");
    make_bundle(&bundle, ADAPTER_UUID, "v1");
    let manifest_bytes = std::fs::read(bundle.path().join("adapter.manifest.json")).unwrap();
    let md = adapter_registry::manifest::manifest_digest(&manifest_bytes);
    let d1 = compute_bundle_digest(bundle.path(), &md).expect("digest1");
    std::fs::write(bundle.path().join("run.sh"), "v2-mutated").expect("mutate");
    write_lock(bundle.path()).expect("re-lock");
    let d2 = compute_bundle_digest(bundle.path(), &md).expect("digest2");
    assert_ne!(d1, d2);
}

#[tokio::test]
async fn same_name_version_different_digest_is_different_identity() {
    let a = tempfile::tempdir().expect("a");
    let b = tempfile::tempdir().expect("b");
    make_bundle(&a, ADAPTER_UUID, "content-a");
    make_bundle(&b, ADAPTER_UUID, "content-b-different");
    let (store, epoch, _d) = store().await;
    let mut tx = txn(&store, epoch).await;
    let ra = register(&mut *tx, a.path(), TrustState::Trusted, 1)
        .await
        .expect("a");
    let rb = register(&mut *tx, b.path(), TrustState::Trusted, 1)
        .await
        .expect("b");
    assert_eq!(ra.adapter_id, rb.adapter_id);
    assert_eq!(ra.version, rb.version);
    assert_ne!(ra.bundle_digest, rb.bundle_digest);
    tx.commit().await.expect("commit");
}

#[tokio::test]
async fn spawn_time_digest_mismatch_rejected() {
    let bundle = tempfile::tempdir().expect("bundle");
    make_bundle(&bundle, ADAPTER_UUID, "original");
    let (store, epoch, _d) = store().await;
    let mut tx = txn(&store, epoch).await;
    let row = register(&mut *tx, bundle.path(), TrustState::Trusted, 1)
        .await
        .expect("register");
    // Mutate the bundle after registration without re-locking: the
    // recorded lock no longer matches disk → verification fails closed.
    std::fs::write(bundle.path().join("run.sh"), "tampered").expect("tamper");
    let err = verify_for_spawn(
        &mut *tx,
        row.adapter_id,
        "1.0.0",
        &row.bundle_digest,
        bundle.path(),
    )
    .await
    .expect_err("drift must be rejected");
    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
}

#[tokio::test]
async fn verify_for_spawn_returns_verified_entrypoint() {
    let bundle = tempfile::tempdir().expect("bundle");
    make_bundle(&bundle, ADAPTER_UUID, "#!/bin/sh\nexit 0\n");
    let (store, epoch, _d) = store().await;
    let mut tx = txn(&store, epoch).await;
    let row = register(&mut *tx, bundle.path(), TrustState::Trusted, 1)
        .await
        .expect("register");
    let verified = verify_for_spawn(
        &mut *tx,
        row.adapter_id,
        "1.0.0",
        &row.bundle_digest,
        bundle.path(),
    )
    .await
    .expect("verify");
    assert!(verified.entrypoint.ends_with("run.sh"));
    assert_eq!(verified.bundle_digest, row.bundle_digest);
}

fn row(id: &str, digest: &str, caps: &[&str], ports: &[(&str, u32)]) -> Candidate {
    let ports_json = serde_json::to_vec(
        &ports
            .iter()
            .map(|(p, v)| PortImpl {
                port_id: (*p).to_owned(),
                port_version: *v,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    Candidate::decode(AdapterRegistrationRow {
        adapter_id: id.parse().expect("uuid"),
        version: "1.0.0".to_owned(),
        bundle_digest: digest.to_owned(),
        manifest_digest: "sha256:m".to_owned(),
        runtime_type: "process".to_owned(),
        implemented_ports: ports_json,
        capabilities: serde_json::to_vec(&caps.iter().map(|c| c.to_string()).collect::<Vec<_>>())
            .unwrap(),
        trust_state: TrustState::Trusted,
        conformance_state: ConformanceState::Passed,
        created_at_ms: 0,
    })
    .expect("decode")
}

#[test]
fn capability_match_and_miss() {
    let good = row(
        ADAPTER_UUID,
        "sha256:a",
        &["status_lookup", "provider_idempotency"],
        &[("model", 1)],
    );
    let missing = row(OTHER_UUID, "sha256:b", &["status_lookup"], &[("model", 1)]);
    let req = PortRequirement {
        port_id: "model".to_owned(),
        port_version: 1,
        required_capabilities: vec![
            "status_lookup".to_owned(),
            "provider_idempotency".to_owned(),
        ],
        sandbox_tier: SandboxTier::T0,
        pin_adapter_id: None,
        require_conformance_passed: false,
    };
    let resolved = resolve(&req, &[good, missing], &[]).expect("resolve");
    assert_eq!(
        resolved.adapter_id,
        ADAPTER_UUID.parse::<AdapterId>().unwrap()
    );

    let req2 = PortRequirement {
        required_capabilities: vec!["nonexistent".to_owned()],
        ..req
    };
    assert!(
        resolve(
            &req2,
            &[row(ADAPTER_UUID, "d", &["x"], &[("model", 1)])],
            &[]
        )
        .is_err()
    );
}

#[test]
fn deterministic_tie_break() {
    // Two identical-capability candidates: priority config wins; with
    // empty priority the lower identity triple wins deterministically.
    let a = row(
        ADAPTER_UUID,
        "sha256:1",
        &["status_lookup"],
        &[("model", 1)],
    );
    let b = row(OTHER_UUID, "sha256:2", &["status_lookup"], &[("model", 1)]);
    let req = PortRequirement {
        port_id: "model".to_owned(),
        port_version: 1,
        required_capabilities: vec!["status_lookup".to_owned()],
        sandbox_tier: SandboxTier::T0,
        pin_adapter_id: None,
        require_conformance_passed: false,
    };
    let r1 = resolve(&req, &[a.clone(), b.clone()], &[]).expect("r1");
    let r2 = resolve(&req, &[b.clone(), a.clone()], &[]).expect("r2");
    assert_eq!(r1, r2, "resolver must not depend on candidate order");

    let preferred = OTHER_UUID.parse::<AdapterId>().unwrap();
    let r3 = resolve(&req, &[a, b], &[preferred]).expect("r3");
    assert_eq!(r3.adapter_id, preferred);
}

#[test]
fn sandbox_t2_fails_closed_on_t0_adapter() {
    let t0_adapter = row(
        ADAPTER_UUID,
        "sha256:t0",
        &["descendant_cleanup"],
        &[("sandbox", 1)],
    );
    let req = PortRequirement {
        port_id: "sandbox".to_owned(),
        port_version: 1,
        required_capabilities: vec![],
        sandbox_tier: SandboxTier::T2,
        pin_adapter_id: None,
        require_conformance_passed: false,
    };
    let err = resolve(&req, &[t0_adapter], &[]).expect_err("T2 must not fall back");
    assert!(err.message().contains("capability unsupported"));
}

#[test]
fn port_version_mismatch_rejected() {
    let c = row(ADAPTER_UUID, "sha256:1", &[], &[("model", 1)]);
    let req = PortRequirement {
        port_id: "model".to_owned(),
        port_version: 2,
        required_capabilities: vec![],
        sandbox_tier: SandboxTier::T0,
        pin_adapter_id: None,
        require_conformance_passed: false,
    };
    assert!(resolve(&req, &[c], &[]).is_err());
}

#[test]
fn t2_capability_set_gate() {
    let full = CapabilitySet::new(adapter_registry::REQUIRED_FOR_T2.iter().copied());
    assert!(full.hosts_tier(SandboxTier::T2));
    let partial = CapabilitySet::new(["host_fs_default_deny", "cpu_limit"]);
    assert!(!partial.hosts_tier(SandboxTier::T2));
    assert!(partial.hosts_tier(SandboxTier::T0));
}

#[test]
fn lock_rejects_traversal_and_unsorted() {
    assert!(
        adapter_registry::parse_lock(
            b"sha256:0123456789012345678901234567890123456789012345678901234567890123 ../x\n"
        )
        .is_err()
    );
    assert!(
        adapter_registry::parse_lock(
            b"sha256:0123456789012345678901234567890123456789012345678901234567890123 b\nsha256:0123456789012345678901234567890123456789012345678901234567890123 a\n"
        )
        .is_err()
    );
    assert!(
        adapter_registry::parse_lock(
            b"sha256:0123456789012345678901234567890123456789012345678901234567890123 a\nsha256:0123456789012345678901234567890123456789012345678901234567890123 a\n"
        )
        .is_err()
    );
}

#[test]
fn required_t2_catalog_matches_yaml() {
    // Lockstep with proto/capabilities/adapter-capabilities.yaml.
    let yaml: serde_yaml::Value = serde_yaml::from_str(include_str!(
        "../../../proto/capabilities/adapter-capabilities.yaml"
    ))
    .expect("catalog parses");
    let listed: std::collections::BTreeSet<String> = yaml["sandbox"]["required_for_t2"]
        .as_sequence()
        .expect("list")
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let ours: std::collections::BTreeSet<String> = adapter_registry::REQUIRED_FOR_T2
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    assert_eq!(listed, ours);
}
