//! `FaultInjector` trait and the no-op implementation.

/// Deterministic fault trigger used by tests.
pub trait FaultInjector: Send + Sync + 'static {
    /// Triggers the fault armed at `point`, if any.
    fn trigger(&self, point: &str);
}

/// Production [`FaultInjector`] that never triggers.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoFaults;

impl FaultInjector for NoFaults {
    fn trigger(&self, _point: &str) {}
}
