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

#[test]
fn inject_unarmed_point_returns_false() {
    let faults = ArmedFaults::new();
    assert!(!faults.inject("io"), "an unarmed point must not fire");
    assert!(!faults.is_triggered("io"));
}

#[test]
fn inject_armed_point_fires_exactly_once() {
    let faults = ArmedFaults::new();
    faults.arm("io");
    assert!(faults.inject("io"), "the first inject must fire");
    assert!(faults.is_triggered("io"));
    faults.assert_triggered("io");
    assert!(!faults.inject("io"), "the second inject must not fire");
}

#[test]
fn inject_rearmed_point_fires_again() {
    let faults = ArmedFaults::new();
    faults.arm("io");
    assert!(faults.inject("io"));
    faults.arm("io");
    assert!(!faults.is_triggered("io"), "re-arming resets the point");
    assert!(faults.inject("io"), "the re-armed point must fire again");
}

#[test]
fn inject_marks_point_triggered_for_assertions() {
    let faults = ArmedFaults::new();
    faults.arm("clock");
    assert!(faults.inject("clock"));
    faults.assert_triggered("clock");
}

#[derive(Clone, Copy, Debug)]
enum SeamAction {
    Arm(usize),
    Inject(usize),
}

fn seam_action() -> impl Strategy<Value = SeamAction> {
    prop_oneof![
        (0..POINTS.len()).prop_map(SeamAction::Arm),
        (0..POINTS.len()).prop_map(SeamAction::Inject),
    ]
}

proptest! {
    #[test]
    fn prop_inject_once_per_arm(actions in prop::collection::vec(seam_action(), 0..64)) {
        let faults = ArmedFaults::new();
        let mut armed = [false; POINTS.len()];
        let mut fired = [false; POINTS.len()];

        for action in actions {
            match action {
                SeamAction::Arm(index) => {
                    faults.arm(POINTS[index]);
                    armed[index] = true;
                    fired[index] = false;
                }
                SeamAction::Inject(index) => {
                    let expected = armed[index] && !fired[index];
                    let fired_now = faults.inject(POINTS[index]);
                    prop_assert!(
                        fired_now == expected,
                        "inject return diverged for {}",
                        POINTS[index]
                    );
                    if expected {
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
    }
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
