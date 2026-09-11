//! Minimal daemon stand-in for `TempDaemonHost` tests.
//!
//! Parks without touching the filesystem and exits 0. Pass
//! `--exit-after-ms <ms>` to park for a bounded time instead of indefinitely.

use std::thread;
use std::time::Duration;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut exit_after_ms = None;
    while let Some(arg) = args.next() {
        if arg == "--exit-after-ms" {
            let value = match args.next() {
                Some(value) => value,
                None => fail("--exit-after-ms requires a value"),
            };
            exit_after_ms = Some(match value.parse::<u64>() {
                Ok(ms) => ms,
                Err(error) => fail(&format!("invalid --exit-after-ms value {value:?}: {error}")),
            });
        }
    }

    match exit_after_ms {
        Some(ms) => thread::park_timeout(Duration::from_millis(ms)),
        None => thread::park(),
    }
}

fn fail(message: &str) -> ! {
    eprintln!("fixture-daemon: {message}");
    std::process::exit(2);
}
