//! Proves the contract snapshot compiles hermetically and regenerates byte-identically.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use domain::generated::contract;

fn proto_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("proto")
        .canonicalize()
        .expect("agent-os/proto exists")
}

fn collect_protos(root: &Path) -> Vec<PathBuf> {
    let mut protos = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("read_dir {}: {error}", dir.display()))
            .map(|entry| entry.expect("directory entry").path())
            .collect::<Vec<_>>();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "proto")
            {
                protos.push(path);
            }
        }
    }
    protos.sort();
    protos
}

fn compile_snapshot(out_dir: &Path) {
    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc");
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }
    let root = proto_root();
    let protos = collect_protos(&root);
    assert!(!protos.is_empty(), "snapshot contains proto files");
    prost_build::Config::new()
        .out_dir(out_dir)
        .compile_protos(&protos, &[&root])
        .expect("snapshot compiles");
}

fn read_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("generated output is readable") {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let name = path
                    .strip_prefix(root)
                    .expect("generated file is inside out dir")
                    .to_string_lossy()
                    .into_owned();
                files.insert(
                    name,
                    std::fs::read(&path).expect("generated file is readable"),
                );
            }
        }
    }
    files
}

#[test]
fn contract_codegen_regenerates_the_snapshot_byte_identical() {
    let first_dir = tempfile::tempdir().expect("first temp dir");
    let second_dir = tempfile::tempdir().expect("second temp dir");
    compile_snapshot(first_dir.path());
    compile_snapshot(second_dir.path());
    let first = read_tree(first_dir.path());
    let second = read_tree(second_dir.path());
    assert!(!first.is_empty(), "generator produced output");
    assert_eq!(first, second, "generated output must be byte-identical");
}

#[test]
fn contract_codegen_exports_command_request_and_the_six_decisions() {
    assert_eq!(
        std::any::type_name::<contract::CommandRequest>(),
        "domain::generated::contract::CommandRequest"
    );
    assert_eq!(
        std::any::type_name::<contract::LoopDecision>(),
        "domain::generated::contract::LoopDecision"
    );

    let decisions = [
        std::any::type_name::<contract::Complete>(),
        std::any::type_name::<contract::Fail>(),
        std::any::type_name::<contract::Wait>(),
        std::any::type_name::<contract::SpawnAgent>(),
        std::any::type_name::<contract::InvokeEffect>(),
        std::any::type_name::<contract::RequestApproval>(),
    ];
    assert_eq!(decisions.len(), 6);
    for decision in decisions {
        assert!(
            decision.starts_with("domain::generated::contract::"),
            "decision {decision} is not a generated contract type"
        );
    }
}
