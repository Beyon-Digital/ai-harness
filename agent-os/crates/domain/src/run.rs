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
}
