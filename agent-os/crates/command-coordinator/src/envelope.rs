//! Command envelope and request digest: the routing and replay-guard fields
//! carried by every command submission (R1.1, R2.1, R2.2, R2.3, N1).

use std::fmt;
use std::str::FromStr;

use domain::ids::{ActorId, CommandId, DelegationChainId, DeviceId, IdempotencyKey, PrincipalId};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};

/// 32-byte digest of the request, rendered as 64 lowercase hexadecimal
/// characters. Parsing accepts only the canonical lowercase form.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestDigest([u8; 32]);

impl FromStr for RequestDigest {
    type Err = errors::KernelError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() != 64 {
            return Err(malformed_digest());
        }
        let mut bytes = [0u8; 32];
        for (index, pair) in text.as_bytes().chunks_exact(2).enumerate() {
            let high = hex_nibble(pair[0]).ok_or_else(malformed_digest)?;
            let low = hex_nibble(pair[1]).ok_or_else(malformed_digest)?;
            bytes[index] = (high << 4) | low;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for RequestDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

fn malformed_digest() -> KernelError {
    KernelError::new(
        ErrorCode::InvalidArgument,
        RetryClass::Never,
        "request_digest must be 64 lowercase hexadecimal characters",
    )
}

fn hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

/// Everything a command submission carries across the coordinator boundary.
#[derive(Clone, Debug)]
pub struct CommandEnvelope {
    /// Identity of this submission, recorded on the transaction.
    pub command_id: CommandId,
    /// Replay key scoped by principal.
    pub idempotency_key: IdempotencyKey,
    /// Principal on whose behalf the command runs.
    pub principal_id: PrincipalId,
    /// Actor that issued the command.
    pub actor_id: ActorId,
    /// Optional device that issued the command.
    pub device_id: Option<DeviceId>,
    /// Optional delegation chain authorizing the actor.
    pub delegation_chain_id: Option<DelegationChainId>,
    /// Digest compared on replay to detect divergent resubmissions.
    pub request_digest: RequestDigest,
    /// Optional correlation identifier for observability.
    pub correlation_id: Option<String>,
    /// Optional identifier of the command that caused this one.
    pub causation_id: Option<String>,
    /// Optional expiry deadline in UTC Unix milliseconds.
    pub deadline_unix_ms: Option<i64>,
    /// Registered command type that routes the envelope to its handler.
    pub command_type: String,
    /// Serialized command payload; never logged or echoed in errors.
    pub payload: Vec<u8>,
}

impl CommandEnvelope {
    /// Rejects empty keys, malformed digests, empty command types, and past
    /// deadlines. Malformed digests are rejected when [`RequestDigest`] is
    /// parsed, before an envelope can exist.
    pub fn validate(&self, now_unix_ms: i64) -> errors::Result<()> {
        if self.idempotency_key.as_str().trim().is_empty() {
            return Err(KernelError::new(
                ErrorCode::InvalidArgument,
                RetryClass::Never,
                "idempotency_key must not be empty",
            ));
        }
        if self.command_type.trim().is_empty() {
            return Err(KernelError::new(
                ErrorCode::InvalidArgument,
                RetryClass::Never,
                "command_type must not be empty",
            ));
        }
        if let Some(deadline_unix_ms) = self.deadline_unix_ms
            && deadline_unix_ms < now_unix_ms
        {
            return Err(KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                "deadline_unix_ms is in the past",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use errors::codes::ErrorCode;

    use super::RequestDigest;

    #[test]
    fn canonical_digests_round_trip_through_lowercase_hex() {
        let text = "ab".repeat(32);
        let digest = match RequestDigest::from_str(&text) {
            Ok(digest) => digest,
            Err(error) => panic!("canonical digest rejected: {error}"),
        };
        assert_eq!(digest.to_string(), text);
    }

    #[test]
    fn malformed_digests_are_rejected() {
        let cases = [
            String::new(),
            "zz".repeat(32),
            "a".repeat(63),
            "a".repeat(65),
            "AB".repeat(32),
            "é".repeat(32),
        ];
        for case in cases {
            let error = match RequestDigest::from_str(&case) {
                Ok(_) => panic!("malformed digest admitted"),
                Err(error) => error,
            };
            assert_eq!(error.code(), ErrorCode::InvalidArgument);
        }
    }
}
