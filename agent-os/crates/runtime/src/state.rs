//! Normative run state machine.
//!
//! The transition table mirrors `specs/runtime-manager.md` and is the only
//! authority for run state changes; mutation sites must ask [`allows`] before
//! writing a new state to `runs.state`.

use domain::run::RunState;

/// Terminal reason persisted when a run reaches `Completed`.
pub const REASON_COMPLETED: &str = "completed";
/// Terminal reason persisted when a run reaches `Failed`.
pub const REASON_FAILED: &str = "failed";
/// Terminal reason persisted when a run reaches `Cancelled`.
pub const REASON_CANCELLED: &str = "cancelled";
/// Reason carried by the cancellation service when it requests cancellation.
///
/// `Cancelling` is not terminal, so the transition service leaves
/// `terminal_reason` untouched while a run drains.
pub const REASON_CANCELLATION_REQUESTED: &str = "cancellation_requested";

/// Returns true when the normative transition table permits `from -> to`.
///
/// `Created -> Cancelled` is cancellation-only: a never-started run has no
/// cleanup to drain and no resolved environment, so the cancellation service
/// moves it straight to `Cancelled` rather than fabricating a `Ready`.
pub const fn allows(from: RunState, to: RunState) -> bool {
    use RunState::*;
    matches!(
        (from, to),
        (Created, Ready)
            | (Created, Cancelled) // cancellation of a never-started run
            | (Ready, Running)
            | (Ready, Cancelled)
            | (Running, WaitingTool)
            | (Running, WaitingChild)
            | (Running, WaitingHuman)
            | (Running, Suspended)
            | (Running, Cancelling)
            | (Running, Completed)
            | (Running, Failed)
            | (WaitingTool, Running)
            | (WaitingTool, Cancelling)
            | (WaitingTool, Failed)
            | (WaitingChild, Running)
            | (WaitingChild, Cancelling)
            | (WaitingChild, Failed)
            | (WaitingHuman, Running)
            | (WaitingHuman, Cancelling)
            | (WaitingHuman, Failed)
            | (Suspended, Running)
            | (Suspended, Cancelling)
            | (Suspended, Failed)
            | (Cancelling, Cancelled)
            | (Cancelling, Failed)
    )
}

/// Returns true for the absorbing terminal states `Completed`, `Failed`, and
/// `Cancelled`.
///
/// [`RunState::is_terminal`] is the single authority; this alias keeps the
/// crate-root spelling used by the state-machine suite.
pub const fn is_terminal(state: RunState) -> bool {
    state.is_terminal()
}
