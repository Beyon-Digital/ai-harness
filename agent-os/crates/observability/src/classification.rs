//! Classification levels and redacted wrappers.
//!
//! `Secret` is the only access path to wrapped content: its `Debug` and
//! `Display` render `[REDACTED]` and `expose` is the sole way to read the
//! value. `Redacted` hides its content from `Debug` but permits access.

/// Sensitivity of a value, ordered from lowest to highest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Classification {
    Public = 0,
    #[default]
    Internal = 1,
    Confidential = 2,
    Secret = 3,
}

impl Classification {
    /// Reports whether this classification is more sensitive than
    /// `threshold`.
    pub fn exceeds(self, threshold: Classification) -> bool {
        self > threshold
    }
}

/// Reports whether a payload classified `payload` may be emitted by a sink
/// whose sensitivity threshold is `sink`. Payloads above the threshold are
/// redacted or omitted (R18.3).
pub fn is_visible(payload: Classification, sink: Classification) -> bool {
    !payload.exceeds(sink)
}

/// Reports the [`Classification`] of a value.
pub trait Classified {
    fn classification(&self) -> Classification;
}

/// Wrapper that renders `[REDACTED]` through `Debug` while still allowing
/// access through [`Redacted::get`] and [`Redacted::into_inner`].
pub struct Redacted<T>(T);

impl<T> Redacted<T> {
    pub fn new(inner: T) -> Self {
        Self(inner)
    }

    pub fn get(&self) -> &T {
        &self.0
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> std::fmt::Debug for Redacted<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Wrapper whose content never renders: `Debug` and `Display` print
/// `[REDACTED]`, and [`Secret::expose`] is the only access path.
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(inner: T) -> Self {
        Self(inner)
    }

    /// The only access path to the wrapped content.
    pub fn expose(&self) -> &T {
        &self.0
    }
}

impl<T> std::fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl<T> std::fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
