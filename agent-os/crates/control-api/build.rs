use std::env;
use std::path::PathBuf;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let proto_root = manifest_dir.join("..").join("..").join("proto");
    let service_proto = proto_root.join("control-api").join("mvp_control.proto");
    println!("cargo:rerun-if-changed={}", proto_root.display());
    println!("cargo:rerun-if-changed=build.rs");

    let protoc = protoc_bin_vendored::protoc_bin_path().expect("vendored protoc is available");
    // SAFETY: safe Rust cannot invoke pre-exec hooks; build scripts may use
    // env mutation (process-local, before any threads spawn is violated only
    // if other threads exist — prost_build spawns none before this).
    unsafe {
        env::set_var("PROTOC", protoc);
    }

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        // Message types come from the domain crate's generated contract
        // module; only the service stubs are generated here.
        .extern_path(".agentos.spec.v1", "::domain::generated::contract")
        .compile_protos(&[service_proto], &[proto_root])
        .expect("control-api service compiles");
}
