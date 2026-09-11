//! Armed fault injector for deterministic fault sequences in tests.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

use domain::faults::FaultInjector;

#[derive(Clone, Copy, Debug)]
struct FaultState {
    armed: bool,
    triggered: bool,
}

/// Fault injector whose armed points fire exactly once per arm.
///
/// A point is installed with [`arm`](Self::arm) and consumed by the first
/// [`trigger`](FaultInjector::trigger) that follows, until it is armed again.
#[derive(Debug, Default)]
pub struct ArmedFaults {
    states: Mutex<HashMap<String, FaultState>>,
}

impl ArmedFaults {
    /// Creates an injector with no armed points.
    pub fn new() -> Self {
        Self::default()
    }

    /// Arms `point`, resetting any previous trigger for it.
    pub fn arm(&self, point: &str) {
        self.states().insert(
            point.to_owned(),
            FaultState {
                armed: true,
                triggered: false,
            },
        );
    }

    /// Returns whether `point` has fired since it was last armed.
    pub fn is_triggered(&self, point: &str) -> bool {
        self.states()
            .get(point)
            .is_some_and(|state| state.triggered)
    }

    /// Panics naming `point` when it was armed but never fired.
    pub fn assert_triggered(&self, point: &str) {
        if !self.is_triggered(point) {
            panic!("fault point {point:?} was armed but never triggered");
        }
    }

    fn states(&self) -> MutexGuard<'_, HashMap<String, FaultState>> {
        match self.states.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

impl FaultInjector for ArmedFaults {
    fn trigger(&self, point: &str) {
        if let Some(state) = self.states().get_mut(point)
            && state.armed
            && !state.triggered
        {
            state.triggered = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use domain::faults::FaultInjector;

    use super::ArmedFaults;

    #[test]
    fn test_fault_fires_once() {
        let faults = ArmedFaults::new();
        assert!(!faults.is_triggered("io"));
        faults.arm("io");
        assert!(!faults.is_triggered("io"));
        faults.trigger("io");
        assert!(faults.is_triggered("io"));
        faults.trigger("io");
        assert!(faults.is_triggered("io"));
        faults.assert_triggered("io");
        faults.arm("io");
        assert!(!faults.is_triggered("io"), "re-arming resets the point");
        faults.trigger("io");
        assert!(faults.is_triggered("io"));
    }

    #[test]
    fn triggering_an_unarmed_point_is_a_no_op() {
        let faults = ArmedFaults::new();
        faults.trigger("never-armed");
        assert!(!faults.is_triggered("never-armed"));
    }

    #[test]
    fn test_untriggered_fault_panics() {
        let faults = ArmedFaults::new();
        faults.arm("clock");
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            faults.assert_triggered("clock");
        }));
        let payload = match panic {
            Ok(()) => panic!("assert_triggered must panic for an unfired point"),
            Err(payload) => payload,
        };
        let message = match payload.downcast_ref::<String>() {
            Some(message) => message.as_str(),
            None => panic!("panic payload must name the point"),
        };
        assert!(
            message.contains("clock"),
            "panic must name the point: {message}"
        );
        assert!(!faults.is_triggered("clock"));
        faults.trigger("clock");
        faults.assert_triggered("clock");
    }
}
