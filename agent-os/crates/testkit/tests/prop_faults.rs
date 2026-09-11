//! Property test: armed points fire exactly once per arm (R17.2, P3).

use domain::faults::FaultInjector;
use proptest::prelude::*;
use testkit::faults::ArmedFaults;

const POINTS: [&str; 4] = ["clock", "fs", "io", "net"];

#[derive(Clone, Copy, Debug)]
enum Action {
    Arm(usize),
    Trigger(usize),
}

fn action() -> impl Strategy<Value = Action> {
    prop_oneof![
        (0..POINTS.len()).prop_map(Action::Arm),
        (0..POINTS.len()).prop_map(Action::Trigger),
    ]
}

proptest! {
    #[test]
    fn prop_faults(actions in prop::collection::vec(action(), 0..64)) {
        let faults = ArmedFaults::new();
        let mut armed = [false; POINTS.len()];
        let mut fired = [false; POINTS.len()];

        for action in actions {
            match action {
                Action::Arm(index) => {
                    faults.arm(POINTS[index]);
                    armed[index] = true;
                    fired[index] = false;
                }
                Action::Trigger(index) => {
                    let was_fired = fired[index];
                    faults.trigger(POINTS[index]);
                    if armed[index] && !was_fired {
                        fired[index] = true;
                    }
                }
            }
            for index in 0..POINTS.len() {
                prop_assert_eq!(
                    faults.is_triggered(POINTS[index]),
                    fired[index],
                    "point {} diverged from the model",
                    POINTS[index]
                );
            }
        }

        for index in 0..POINTS.len() {
            if fired[index] {
                faults.assert_triggered(POINTS[index]);
            } else if armed[index] {
                let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    faults.assert_triggered(POINTS[index]);
                }));
                prop_assert!(panic.is_err(), "an armed but unfired point must panic");
            }
        }
    }
}
