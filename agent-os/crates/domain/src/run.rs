//! Run mirror enums and immutable value types.

use std::fmt;

/// Error returned when a wire value has no mirror-enum variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UnknownEnumValue {
    pub value: i32,
    pub enum_name: &'static str,
}

impl fmt::Display for UnknownEnumValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown {} wire value {}", self.enum_name, self.value)
    }
}

impl std::error::Error for UnknownEnumValue {}

/// Error returned when a persisted state string has no matching variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownStateValue {
    pub value: String,
    pub enum_name: &'static str,
}

impl fmt::Display for UnknownStateValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown {} state {:?}", self.enum_name, self.value)
    }
}

impl std::error::Error for UnknownStateValue {}

macro_rules! mirror_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $enum_name:literal, {
            $( $(#[$variant_meta:meta])* $variant:ident = $wire:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$variant_meta])* $variant, )+
        }

        impl $name {
            /// Converts a wire integer into the matching variant.
            ///
            /// A value that has no variant in this enum is rejected with
            /// [`UnknownEnumValue`]; it is never mapped to a default.
            pub fn from_wire(value: i32) -> Result<Self, UnknownEnumValue> {
                match value {
                    $( $wire => Ok(Self::$variant), )+
                    _ => Err(UnknownEnumValue { value, enum_name: $enum_name }),
                }
            }

            /// Returns the wire integer for this variant.
            pub fn to_wire(self) -> i32 {
                match self {
                    $( Self::$variant => $wire, )+
                }
            }
        }
    };
}

pub(crate) use mirror_enum;

macro_rules! state_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $enum_name:literal, {
            $( $(#[$variant_meta:meta])* $variant:ident = $wire:literal => $text:literal ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[doc = ""]
        #[doc = "The integer mapping (`from_wire`/`to_wire`) is **provisional** and must not be"]
        #[doc = "used for persistence: the backing column is `TEXT` with a `CHECK` domain."]
        #[doc = "Use [`as_str`](Self::as_str) and [`from_state_str`](Self::from_state_str) for"]
        #[doc = "stored state; [`from_wire`](Self::from_wire) never guesses an unknown integer."]
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $( $(#[$variant_meta])* $variant, )+
        }

        impl $name {
            /// Converts a provisional wire integer into the matching variant.
            ///
            /// A value that has no variant in this enum is rejected with
            /// [`UnknownEnumValue`]; it is never mapped to a default.
            pub fn from_wire(value: i32) -> Result<Self, UnknownEnumValue> {
                match value {
                    $( $wire => Ok(Self::$variant), )+
                    _ => Err(UnknownEnumValue { value, enum_name: $enum_name }),
                }
            }

            /// Returns the provisional wire integer for this variant.
            ///
            /// Not for persistence; use [`as_str`](Self::as_str) for the stored form.
            pub fn to_wire(self) -> i32 {
                match self {
                    $( Self::$variant => $wire, )+
                }
            }

            /// Returns the exact persisted state string (the schema `CHECK` literal).
            pub fn as_str(&self) -> &'static str {
                match self {
                    $( Self::$variant => $text, )+
                }
            }

            /// Parses an exact persisted state string (the schema `CHECK` literal).
            pub fn from_state_str(value: &str) -> Result<Self, UnknownStateValue> {
                match value {
                    $( $text => Ok(Self::$variant), )+
                    _ => Err(UnknownStateValue { value: value.to_owned(), enum_name: $enum_name }),
                }
            }
        }
    };
}

pub(crate) use state_enum;

mirror_enum! {
    /// Lifecycle state of an agent run, mirroring `contract::RunState`.
    RunState, "RunState", {
        Unspecified = 0,
        Created = 1,
        Ready = 2,
        Running = 3,
        WaitingTool = 4,
        WaitingChild = 5,
        WaitingHuman = 6,
        Suspended = 7,
        Cancelling = 8,
        Completed = 9,
        Failed = 10,
        Cancelled = 11,
    }
}

impl RunState {
    /// Returns true for the absorbing terminal states `Completed`, `Failed`,
    /// and `Cancelled`.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Returns the state a cancellation request moves this run to, or `None`
    /// when cancellation no longer applies.
    ///
    /// Never-started runs (`Created`, `Ready`) end `Cancelled` directly: there
    /// is no cleanup to drain and no resolved environment to fabricate. Every
    /// other started non-terminal state drains through `Cancelling`, and
    /// `Cancelling`, terminal, and `Unspecified` runs are not cancellation
    /// targets.
    pub const fn cancellation_target(self) -> Option<Self> {
        match self {
            Self::Created | Self::Ready => Some(Self::Cancelled),
            Self::Running
            | Self::WaitingTool
            | Self::WaitingChild
            | Self::WaitingHuman
            | Self::Suspended => Some(Self::Cancelling),
            Self::Unspecified
            | Self::Cancelling
            | Self::Completed
            | Self::Failed
            | Self::Cancelled => None,
        }
    }
}

mirror_enum! {
    /// Recovery disposition of an agent run, mirroring `contract::RecoveryDisposition`.
    RecoveryDisposition, "RecoveryDisposition", {
        Unspecified = 0,
        Normal = 1,
        NeedsReconciliation = 2,
        Recovering = 3,
        BlockedUnknownEffect = 4,
        BlockedMissingResource = 5,
        RequiresHumanDecision = 6,
    }
}

#[cfg(test)]
mod tests {
    use super::{RecoveryDisposition, RunState, UnknownEnumValue};

    #[test]
    fn known_values_map_to_exact_variants() {
        assert_eq!(RunState::from_wire(3), Ok(RunState::Running));
        assert_eq!(RunState::from_wire(0), Ok(RunState::Unspecified));
        assert_eq!(
            RecoveryDisposition::from_wire(6),
            Ok(RecoveryDisposition::RequiresHumanDecision)
        );
    }

    #[test]
    fn unknown_values_are_not_guessed() {
        assert_eq!(
            RunState::from_wire(12),
            Err(UnknownEnumValue {
                value: 12,
                enum_name: "RunState"
            })
        );
        assert_eq!(
            RecoveryDisposition::from_wire(-1),
            Err(UnknownEnumValue {
                value: -1,
                enum_name: "RecoveryDisposition"
            })
        );
    }

    #[test]
    fn terminality_and_cancellation_targets_are_centralized() {
        for state in [RunState::Completed, RunState::Failed, RunState::Cancelled] {
            assert!(state.is_terminal(), "{state:?}");
            assert_eq!(state.cancellation_target(), None, "{state:?}");
        }
        for state in [RunState::Created, RunState::Ready] {
            assert!(!state.is_terminal(), "{state:?}");
            assert_eq!(
                state.cancellation_target(),
                Some(RunState::Cancelled),
                "{state:?}"
            );
        }
        for state in [
            RunState::Running,
            RunState::WaitingTool,
            RunState::WaitingChild,
            RunState::WaitingHuman,
            RunState::Suspended,
        ] {
            assert!(!state.is_terminal(), "{state:?}");
            assert_eq!(
                state.cancellation_target(),
                Some(RunState::Cancelling),
                "{state:?}"
            );
        }
        assert!(!RunState::Cancelling.is_terminal());
        assert_eq!(RunState::Cancelling.cancellation_target(), None);
        assert_eq!(RunState::Unspecified.cancellation_target(), None);
    }
}
