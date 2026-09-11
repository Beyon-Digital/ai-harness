//! Stable error code constants and their retry classes.

/// Machine-readable failure classification with a stable lowercase token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    InvalidArgument,
    NotFound,
    Conflict,
    FailedPrecondition,
    ResourceExhausted,
    Unavailable,
    Internal,
}

impl ErrorCode {
    /// Returns the stable lowercase token for this code.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::FailedPrecondition => "failed_precondition",
            Self::ResourceExhausted => "resource_exhausted",
            Self::Unavailable => "unavailable",
            Self::Internal => "internal",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Structural retry safety: never, safe to replay, or only after reconciliation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetryClass {
    Never,
    Safe,
    ReconciliationRequired,
}
