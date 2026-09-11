use std::env;
use std::path::{Path, PathBuf};

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

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let proto_root = manifest_dir.join("..").join("..").join("proto");
    println!("cargo:rerun-if-changed={}", proto_root.display());
    println!("cargo:rerun-if-changed=build.rs");

    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc is available");
    unsafe {
        env::set_var("PROTOC", protoc);
    }

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set"));
    let protos = collect_protos(&proto_root);
    assert!(
        !protos.is_empty(),
        "no proto files under {}",
        proto_root.display()
    );
    prost_build::Config::new()
        .out_dir(out_dir)
        .compile_protos(&protos, &[&proto_root])
        .expect("contract snapshot compiles");
}
