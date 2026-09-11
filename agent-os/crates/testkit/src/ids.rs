//! Deterministic `IdProvider` implementation for tests.

use std::sync::atomic::{AtomicU64, Ordering};

use domain::provider::IdProvider;
use uuid::Uuid;

/// Mask covering the 48 timestamp bits of a UUIDv7.
const TIMESTAMP_MASK: u64 = (1 << 48) - 1;

/// Mask covering the 62 random bits that follow the variant field.
const RAND_B_MASK: u64 = (1 << 62) - 1;

/// Mask covering the 56 random bits stored in the final seven octets.
const RAND_B_LOW_MASK: u64 = (1 << 56) - 1;

/// Deterministic [`IdProvider`] producing UUIDv7 values from a fixed seed
/// timestamp followed by a monotonically increasing counter.
#[derive(Debug)]
pub struct DeterministicIds {
    seed_ms: i64,
    counter: AtomicU64,
}

impl DeterministicIds {
    /// Creates a provider whose UUIDs carry `seed_ms` as their timestamp.
    pub fn new(seed_ms: i64) -> Self {
        Self {
            seed_ms,
            counter: AtomicU64::new(0),
        }
    }
}

impl IdProvider for DeterministicIds {
    fn new_uuid_v7(&self) -> Uuid {
        let counter = self.counter.fetch_add(1, Ordering::Relaxed);
        uuid_v7(self.seed_ms, counter)
    }
}

fn uuid_v7(seed_ms: i64, counter: u64) -> Uuid {
    let timestamp = seed_ms as u64 & TIMESTAMP_MASK;
    let rand_a = ((counter >> 62) & 0x0FFF) as u16;
    let rand_b = counter & RAND_B_MASK;

    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&timestamp.to_be_bytes()[2..]);
    bytes[6] = 0x70 | ((rand_a >> 8) as u8 & 0x0F);
    bytes[7] = (rand_a & 0xFF) as u8;
    bytes[8] = 0x80 | ((rand_b >> 56) as u8 & 0x3F);
    bytes[9..].copy_from_slice(&(rand_b & RAND_B_LOW_MASK).to_be_bytes()[1..]);
    Uuid::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use domain::provider::IdProvider;

    use super::DeterministicIds;

    #[test]
    fn test_ids_are_time_sorted() {
        let provider = DeterministicIds::new(1_700_000_000_000);
        let first = provider.new_uuid_v7();
        let second = provider.new_uuid_v7();
        assert_eq!(first.get_version_num(), 7);
        assert_eq!(second.get_version_num(), 7);
        assert!(first < second, "ids must be strictly increasing");

        let mut previous = second;
        let mut seen = HashSet::new();
        assert!(seen.insert(first));
        assert!(seen.insert(second));
        for _ in 0..256 {
            let next = provider.new_uuid_v7();
            assert_eq!(next.get_version_num(), 7);
            assert!(previous < next, "ids must be strictly increasing");
            assert!(seen.insert(next), "ids must be unique");
            previous = next;
        }
    }

    #[test]
    fn ids_embed_the_seed_timestamp() {
        let provider = DeterministicIds::new(1_700_000_000_000);
        let id = provider.new_uuid_v7();
        let bytes = id.as_bytes();
        let timestamp = u64::from_be_bytes([
            0, 0, bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
        ]);
        assert_eq!(timestamp, 1_700_000_000_000);
    }

    #[test]
    fn same_seed_replays_the_same_sequence() {
        let left = DeterministicIds::new(7);
        let right = DeterministicIds::new(7);
        for _ in 0..8 {
            assert_eq!(left.new_uuid_v7(), right.new_uuid_v7());
        }
    }
}
