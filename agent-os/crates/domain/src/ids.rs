//! Stable identifiers: newtypes, `UuidV7`, keys, and cursors.

use std::fmt;
use std::str::FromStr;

use uuid::Uuid;

use crate::provider::IdProvider;

/// Stable error returned when an identifier, key, or cursor fails validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct InvalidId;

impl fmt::Display for InvalidId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid identifier")
    }
}

impl std::error::Error for InvalidId {}

/// A UUID with version nibble 7 (time-sortable, RFC 9562).
///
/// Parsing accepts only the canonical serialized form: lowercase hyphenated
/// `8-4-4-4-12` hexadecimal. Uppercase and alternative UUID encodings are
/// rejected rather than normalized.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UuidV7(Uuid);

impl UuidV7 {
    /// Draws a new UUIDv7 from the injected provider.
    ///
    /// The provider is trusted to return a version-7 UUID; [`SystemIdProvider`]
    /// does so.
    ///
    /// [`SystemIdProvider`]: crate::provider::SystemIdProvider
    pub fn new(provider: &dyn IdProvider) -> Self {
        Self(provider.new_uuid_v7())
    }

    /// Borrows the underlying [`uuid::Uuid`].
    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }

    /// Returns the lowercase hyphenated serialized form.
    pub fn to_hyphenated(&self) -> String {
        self.0.hyphenated().to_string()
    }
}

impl FromStr for UuidV7 {
    type Err = InvalidId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if !is_canonical_hyphenated(text) {
            return Err(InvalidId);
        }
        let uuid = Uuid::parse_str(text).map_err(|_| InvalidId)?;
        if uuid.get_version_num() != 7 {
            return Err(InvalidId);
        }
        Ok(Self(uuid))
    }
}

impl fmt::Display for UuidV7 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.hyphenated())
    }
}

impl serde::Serialize for UuidV7 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hyphenated())
    }
}

impl<'de> serde::Deserialize<'de> for UuidV7 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let text = <String as serde::Deserialize<'de>>::deserialize(deserializer)?;
        Self::from_str(&text).map_err(serde::de::Error::custom)
    }
}

fn is_canonical_hyphenated(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => *byte == b'-',
        _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(byte),
    })
}

macro_rules! uuid_newtype {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(UuidV7);

        impl $name {
            /// Draws a new identifier from the injected provider.
            pub fn new(provider: &dyn IdProvider) -> Self {
                Self(UuidV7::new(provider))
            }

            /// Wraps an already validated UUIDv7.
            pub fn from_uuid_v7(value: UuidV7) -> Self {
                Self(value)
            }

            /// Borrows the underlying [`uuid::Uuid`].
            pub fn as_uuid(&self) -> &Uuid {
                self.0.as_uuid()
            }

            /// Borrows the underlying [`UuidV7`].
            pub fn as_uuid_v7(&self) -> &UuidV7 {
                &self.0
            }

            /// Returns the lowercase hyphenated serialized form.
            pub fn to_hyphenated(&self) -> String {
                self.0.to_hyphenated()
            }
        }

        impl FromStr for $name {
            type Err = InvalidId;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                UuidV7::from_str(text).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serde::Serialize::serialize(&self.0, serializer)
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                let value = <UuidV7 as serde::Deserialize<'de>>::deserialize(deserializer)?;
                Ok(Self(value))
            }
        }
    };
}

uuid_newtype!(
    /// Identifier of an agent run.
    RunId
);
uuid_newtype!(
    /// Identifier of a task.
    TaskId
);
uuid_newtype!(
    /// Identifier of a session.
    SessionId
);
uuid_newtype!(
    /// Identifier of an effect record.
    EffectId
);
uuid_newtype!(
    /// Identifier of a journal event.
    EventId
);
uuid_newtype!(
    /// Identifier of a workspace.
    WorkspaceId
);
uuid_newtype!(
    /// Identifier of a workspace lease.
    LeaseId
);
uuid_newtype!(
    /// Identifier of a resource reservation.
    ReservationId
);
uuid_newtype!(
    /// Identifier of a scheduler timer.
    TimerId
);
uuid_newtype!(
    /// Identifier of a configuration generation.
    ConfigGenerationId
);
uuid_newtype!(
    /// Identifier of an approval request.
    ApprovalRequestId
);
uuid_newtype!(
    /// Identifier of a capability grant.
    CapabilityGrantId
);
uuid_newtype!(
    /// Identifier of an adapter instance.
    AdapterInstanceId
);
uuid_newtype!(
    /// Identifier of a principal.
    PrincipalId
);
uuid_newtype!(
    /// Identifier of an actor.
    ActorId
);
uuid_newtype!(
    /// Identifier of a device.
    DeviceId
);
uuid_newtype!(
    /// Identifier of a command.
    CommandId
);
uuid_newtype!(
    /// Identifier of a loop decision.
    DecisionId
);
uuid_newtype!(
    /// Identifier of a loop turn.
    TurnId
);
uuid_newtype!(
    /// Identifier of an operation.
    OperationId
);
uuid_newtype!(
    /// Identifier of an agent specification.
    AgentSpecId
);
uuid_newtype!(
    /// Identifier of an adapter.
    AdapterId
);
uuid_newtype!(
    /// Identifier of a run dependency.
    DependencyId
);
uuid_newtype!(
    /// Identifier of a delegation chain.
    DelegationChainId
);
uuid_newtype!(
    /// Identifier of an artifact.
    ArtifactId
);
uuid_newtype!(
    /// Identifier of a sandbox.
    SandboxId
);
uuid_newtype!(
    /// Identifier of a resolved run environment.
    EnvironmentId
);
uuid_newtype!(
    /// Identifier of a daemon instance.
    DaemonInstanceId
);

/// Idempotency key scoped by `(principal_id, idempotency_key)`.
///
/// A key is non-empty, at most 255 bytes, and contains no ASCII control
/// characters.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    /// Validates and wraps an idempotency key.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidId> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 255
            || value.bytes().any(|byte| byte.is_ascii_control())
        {
            return Err(InvalidId);
        }
        Ok(Self(value))
    }

    /// Borrows the key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for IdempotencyKey {
    type Err = InvalidId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::new(text)
    }
}

impl fmt::Display for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for IdempotencyKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for IdempotencyKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let text = <String as serde::Deserialize<'de>>::deserialize(deserializer)?;
        Self::new(text).map_err(serde::de::Error::custom)
    }
}

/// Canonical event stream key.
///
/// A stream key is non-empty and contains only lowercase ASCII letters,
/// digits, `-`, `.`, and `/`; in particular it contains no `:` and no
/// whitespace or control characters.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventStreamKey(String);

impl EventStreamKey {
    /// Validates and wraps a stream key.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidId> {
        let value = value.into();
        if value.is_empty() || !value.bytes().all(is_stream_key_byte) {
            return Err(InvalidId);
        }
        Ok(Self(value))
    }

    /// Borrows the stream key text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_stream_key_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.' | b'/')
}

impl FromStr for EventStreamKey {
    type Err = InvalidId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::new(text)
    }
}

impl fmt::Display for EventStreamKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl serde::Serialize for EventStreamKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for EventStreamKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let text = <String as serde::Deserialize<'de>>::deserialize(deserializer)?;
        Self::new(text).map_err(serde::de::Error::custom)
    }
}

/// Durable journal position: `v1:{stream_key}:{sequence}`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EventCursor {
    pub stream_key: EventStreamKey,
    pub sequence: u64,
}

impl EventCursor {
    /// Builds a cursor for a stream key and sequence.
    pub fn new(stream_key: EventStreamKey, sequence: u64) -> Self {
        Self {
            stream_key,
            sequence,
        }
    }
}

impl fmt::Display for EventCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v1:{}:{}", self.stream_key, self.sequence)
    }
}

impl FromStr for EventCursor {
    type Err = InvalidId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let rest = text.strip_prefix("v1:").ok_or(InvalidId)?;
        let (stream_key, sequence) = rest.rsplit_once(':').ok_or(InvalidId)?;
        Ok(Self {
            stream_key: EventStreamKey::new(stream_key)?,
            sequence: parse_sequence(sequence)?,
        })
    }
}

fn parse_sequence(text: &str) -> Result<u64, InvalidId> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(InvalidId);
    }
    if text.len() > 1 && text.starts_with('0') {
        return Err(InvalidId);
    }
    text.parse::<u64>().map_err(|_| InvalidId)
}

impl serde::Serialize for EventCursor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for EventCursor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let text = <String as serde::Deserialize<'de>>::deserialize(deserializer)?;
        Self::from_str(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::str::FromStr;

    use super::{EventCursor, EventStreamKey, IdempotencyKey, InvalidId, RunId, UuidV7};
    use crate::provider::SystemIdProvider;

    const SAMPLE: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70";

    fn parsed<T>(text: &str) -> T
    where
        T: FromStr<Err = InvalidId>,
    {
        match text.parse() {
            Ok(value) => value,
            Err(error) => panic!("{text:?} failed to parse: {error}"),
        }
    }

    fn accepted<T, E>(result: Result<T, E>) -> T
    where
        E: std::fmt::Display,
    {
        match result {
            Ok(value) => value,
            Err(error) => panic!("expected an accepted value: {error}"),
        }
    }

    #[test]
    fn uuid_v7_parses_and_formats_canonically() {
        let value: UuidV7 = parsed(SAMPLE);
        assert_eq!(value.to_hyphenated(), SAMPLE);
        assert_eq!(value.to_string(), SAMPLE);
        assert_eq!(value.as_uuid().get_version_num(), 7);
    }

    #[test]
    fn typed_ids_round_trip_through_display_and_parse() {
        let id: RunId = parsed(SAMPLE);
        assert_eq!(id.to_string(), SAMPLE);
        assert_eq!(id.to_hyphenated(), SAMPLE);
        assert_eq!(id.as_uuid().get_version_num(), 7);
        assert_eq!(RunId::from_uuid_v7(*id.as_uuid_v7()), id);
    }

    #[test]
    fn invalid_ids_are_rejected() {
        let invalid = [
            "",
            "not-a-uuid",
            "018f2b9c4a1e7c3d9f002b7a1c5d6e70",
            "018F2B9C-4A1E-7C3D-9F00-2B7A1C5D6E70",
            "018f2b9c-4a1e-4c3d-9f00-2b7a1c5d6e70",
            "00000000-0000-0000-0000-000000000000",
        ];
        for text in invalid {
            assert!(UuidV7::from_str(text).is_err(), "{text} must be rejected");
            assert!(text.parse::<RunId>().is_err(), "{text} must be rejected");
        }
        assert!(matches!("nope".parse::<RunId>(), Err(InvalidId)));
    }

    #[test]
    fn generated_ids_are_uuid_v7_and_unique() {
        let provider = SystemIdProvider;
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            let id = RunId::new(&provider);
            assert_eq!(id.as_uuid().get_version_num(), 7);
            assert!(seen.insert(*id.as_uuid()), "duplicate id generated");
        }
    }

    #[test]
    fn idempotency_keys_are_bounded_and_control_free() {
        assert!(IdempotencyKey::new("").is_err());
        assert!(IdempotencyKey::new("a".repeat(255)).is_ok());
        assert!(IdempotencyKey::new("a".repeat(256)).is_err());
        assert!(IdempotencyKey::new("bad\nkey").is_err());
        assert_eq!(parsed::<IdempotencyKey>("key-1").as_str(), "key-1");
    }

    #[test]
    fn stream_keys_reject_non_canonical_forms() {
        assert!(EventStreamKey::new("run/018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70").is_ok());
        assert!(EventStreamKey::new("config/global").is_ok());
        for bad in ["", "Run/x", "run/x y", "run:x", "run/x\n"] {
            assert!(
                EventStreamKey::new(bad).is_err(),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn event_cursors_round_trip_canonically() {
        let cursor = EventCursor::new(accepted(EventStreamKey::new(format!("run/{SAMPLE}"))), 42);
        let rendered = cursor.to_string();
        assert_eq!(rendered, format!("v1:run/{SAMPLE}:42"));
        assert_eq!(parsed::<EventCursor>(&rendered), cursor);

        let zero = EventCursor::new(accepted(EventStreamKey::new("config/global")), 0);
        assert_eq!(zero.to_string(), "v1:config/global:0");
        assert_eq!(parsed::<EventCursor>("v1:config/global:0"), zero);
    }

    #[test]
    fn malformed_cursors_are_rejected() {
        let invalid = [
            "",
            "run/x:1",
            "v1:run/x",
            "v2:run/x:1",
            "v1:run/x:01",
            "v1:run/x:-1",
            "v1:run/x:notnumeric",
            "v1::1",
            "v1:run/x:",
            "v1:run/x:18446744073709551616",
            "v1:run:x:1",
        ];
        for text in invalid {
            assert!(
                text.parse::<EventCursor>().is_err(),
                "{text} must be rejected"
            );
        }
    }

    #[test]
    fn serde_deserialization_validates_identifiers() {
        use serde::de::IntoDeserializer;
        use serde::de::value::{Error as ValueError, StrDeserializer};

        let good: StrDeserializer<'_, ValueError> = SAMPLE.into_deserializer();
        let run_id = match <RunId as serde::Deserialize>::deserialize(good) {
            Ok(value) => value,
            Err(error) => panic!("sample identifier failed to deserialize: {error}"),
        };
        assert_eq!(run_id.to_string(), SAMPLE);

        let bad: StrDeserializer<'_, ValueError> = "not-a-uuid".into_deserializer();
        assert!(<RunId as serde::Deserialize>::deserialize(bad).is_err());
    }

    #[derive(Debug)]
    struct Unsupported;

    impl std::fmt::Display for Unsupported {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("unsupported serialization type")
        }
    }

    impl std::error::Error for Unsupported {}

    impl serde::ser::Error for Unsupported {
        fn custom<T: std::fmt::Display>(_message: T) -> Self {
            Unsupported
        }
    }

    struct StringSerializer;

    impl serde::Serializer for StringSerializer {
        type Ok = String;
        type Error = Unsupported;
        type SerializeSeq = serde::ser::Impossible<String, Unsupported>;
        type SerializeTuple = serde::ser::Impossible<String, Unsupported>;
        type SerializeTupleStruct = serde::ser::Impossible<String, Unsupported>;
        type SerializeTupleVariant = serde::ser::Impossible<String, Unsupported>;
        type SerializeMap = serde::ser::Impossible<String, Unsupported>;
        type SerializeStruct = serde::ser::Impossible<String, Unsupported>;
        type SerializeStructVariant = serde::ser::Impossible<String, Unsupported>;

        fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_i8(self, _value: i8) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_i16(self, _value: i16) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_i32(self, _value: i32) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_i64(self, _value: i64) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_i128(self, _value: i128) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_u8(self, _value: u8) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_u16(self, _value: u16) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_u32(self, _value: u32) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_u64(self, _value: u64) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_u128(self, _value: u128) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_char(self, _value: char) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
            Ok(format!("\"{value}\""))
        }

        fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_some<T>(self, _value: &T) -> Result<Self::Ok, Self::Error>
        where
            T: ?Sized + serde::Serialize,
        {
            Err(Unsupported)
        }

        fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_unit_variant(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
        ) -> Result<Self::Ok, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_newtype_struct<T>(
            self,
            _name: &'static str,
            _value: &T,
        ) -> Result<Self::Ok, Self::Error>
        where
            T: ?Sized + serde::Serialize,
        {
            Err(Unsupported)
        }

        fn serialize_newtype_variant<T>(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
            _value: &T,
        ) -> Result<Self::Ok, Self::Error>
        where
            T: ?Sized + serde::Serialize,
        {
            Err(Unsupported)
        }

        fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_tuple_struct(
            self,
            _name: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeTupleStruct, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_tuple_variant(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeTupleVariant, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_struct(
            self,
            _name: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeStruct, Self::Error> {
            Err(Unsupported)
        }

        fn serialize_struct_variant(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeStructVariant, Self::Error> {
            Err(Unsupported)
        }
    }

    #[test]
    fn serialization_emits_canonical_string_forms() {
        let id: RunId = parsed(SAMPLE);
        let serialized = accepted(serde::Serialize::serialize(&id, StringSerializer));
        assert_eq!(serialized, format!("\"{SAMPLE}\""));
        assert_eq!(serialized, serialized.to_lowercase());
        assert!(serialized.contains('-'));

        let idempotency = parsed::<IdempotencyKey>("key-1");
        assert_eq!(
            accepted(serde::Serialize::serialize(&idempotency, StringSerializer)),
            "\"key-1\""
        );

        let cursor = EventCursor::new(accepted(EventStreamKey::new(format!("run/{SAMPLE}"))), 42);
        let serialized = accepted(serde::Serialize::serialize(&cursor, StringSerializer));
        assert_eq!(serialized, format!("\"v1:run/{SAMPLE}:42\""));
    }
}
