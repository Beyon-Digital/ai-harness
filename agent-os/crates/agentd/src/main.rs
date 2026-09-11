//! Placeholder composition root for the agent daemon; starts and exits cleanly.
#![forbid(unsafe_code)]

mod api;
mod lock;
mod recovery;
mod workers;

fn main() -> std::process::ExitCode {
    std::process::ExitCode::SUCCESS
}
