//! Stable error codes, structural retry classification, and `KernelError`.
#![forbid(unsafe_code)]

pub mod codes;

use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use codes::{ErrorCode, RetryClass};

/// The kernel's single typed error: a stable code, a structural retry class,
/// a redacted message, and an optional chained source.
///
/// `Display` prints `{code}: {message}` and never renders the source, so
/// secret or payload content attached as a source cannot leak through logs.
#[derive(Debug)]
pub struct KernelError {
    code: ErrorCode,
    retry: RetryClass,
    message: Cow<'static, str>,
    source: Option<Box<dyn Error + Send + Sync + 'static>>,
}

impl KernelError {
    /// Builds an error from a code, retry class, and message.
    pub fn new(code: ErrorCode, retry: RetryClass, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            retry,
            message: message.into(),
            source: None,
        }
    }

    /// Attaches a source error without exposing its content via `Display`.
    pub fn with_source(self, source: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            source: Some(source.into()),
            ..self
        }
    }

    /// Returns the stable machine-readable code.
    pub fn code(&self) -> ErrorCode {
        self.code
    }

    /// Returns the structural retry classification.
    pub fn retry_class(&self) -> RetryClass {
        self.retry
    }

    /// Returns the redacted human-readable message.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl Error for KernelError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn Error + 'static))
    }
}

impl fmt::Display for KernelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

/// Result alias carrying [`KernelError`].
pub type Result<T> = std::result::Result<T, KernelError>;
