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
        if arg == "--exit-after-ms"
            && let Some(value) = args.next()
        {
            exit_after_ms = value.parse::<u64>().ok();
        }
    }

    match exit_after_ms {
        Some(ms) => thread::park_timeout(Duration::from_millis(ms)),
        None => thread::park(),
    }
}
