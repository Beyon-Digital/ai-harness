//! `agentos-wasm-host` — runs a `runtime.type = "wasm"` adapter module.
//!
//! The daemon spawns this host with the adapter socketpair on fd 0/1
//! (`SpawnSpec.stdout_ipc`); the guest module's WASI stdin/stdout inherit
//! those descriptors, so its framed `AdapterFrame` protocol flows over
//! the same channel a native adapter would use. The module IS the
//! sandbox: no sockets, no filesystem, no clock unless the WASI ctx
//! grants it — only inherited stdio plus the curated environment.
#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let module = match std::env::args().nth(1).map(PathBuf::from) {
        Some(p) => p,
        None => {
            eprintln!("usage: agentos-wasm-host <module.wasm>");
            return ExitCode::FAILURE;
        }
    };
    match run(&module) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("agentos-wasm-host: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(module: &PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::from_file(&engine, module)?;
    // WASI p1: stdio map onto this process's fds — the spawn sites place
    // the private IPC socketpair on both 0 and 1 for wasm bundles.
    let wasi = wasmtime_wasi::WasiCtxBuilder::new()
        .inherit_stdin()
        .inherit_stdout()
        .inherit_env()
        .build_p1();
    let mut store = wasmtime::Store::new(&engine, wasi);
    let mut linker = wasmtime::Linker::new(&engine);
    wasmtime_wasi::p1::add_to_linker_sync(&mut linker, |t| t)?;
    let instance = linker.instantiate(&mut store, &module)?;
    let start = instance.get_typed_func::<(), ()>(&mut store, "_start")?;
    tracing::info!(exports = %module.exports().count(), "wasm module instantiated");
    start.call(&mut store, ())?;
    Ok(())
}
