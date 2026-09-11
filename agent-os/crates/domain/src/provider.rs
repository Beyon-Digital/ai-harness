//! `IdProvider` trait and the system implementation.

/// Source of new UUIDv7 identifiers.
pub trait IdProvider: Send + Sync + 'static {
    /// Returns a fresh UUIDv7.
    fn new_uuid_v7(&self) -> uuid::Uuid;
}

/// Production [`IdProvider`] backed by `uuid::Uuid::now_v7`.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemIdProvider;

impl IdProvider for SystemIdProvider {
    fn new_uuid_v7(&self) -> uuid::Uuid {
        uuid::Uuid::now_v7()
    }
}
