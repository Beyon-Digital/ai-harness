//! EVT-001 primitives: stream keys, cursors, builder validation, and the
//! envelope codec.

use std::str::FromStr;

use domain::ids::{AdapterId, EffectId, EventId, PrincipalId, RunId, SessionId, TaskId};
use domain::security::{RetentionClass, SensitivityClass};
use errors::codes::{ErrorCode, RetryClass};
use events::cursor::{EventCursor, EventCursorExt};
use events::envelope::{
    CatalogClassificationPolicy, ClassificationPolicy, EventBuilder, EventEnvelope,
};
use events::{StreamKey, StreamKind};

const RUN: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70";
const TASK: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e71";
const SESSION: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e72";
const EFFECT: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e73";
const PRINCIPAL: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e74";
const EVENT: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e75";
const ADAPTER: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e76";
const CAUSE: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e77";

fn run_id() -> RunId {
    RunId::from_str(RUN).expect("sample run id is canonical")
}

fn event_id() -> EventId {
    EventId::from_str(EVENT).expect("sample event id is canonical")
}

fn policy() -> CatalogClassificationPolicy {
    CatalogClassificationPolicy::embedded().expect("embedded catalog parses")
}

fn builder(event_type: &str) -> EventBuilder {
    EventBuilder::new(event_type, 1, StreamKey::run(run_id()))
        .event_id(event_id())
        .sequence(7)
        .occurred_at_ms(1_700_000_000_042)
        .sensitivity(SensitivityClass::Internal)
        .payload(vec![0x07, 0x08])
}

#[test]
fn every_stream_kind_round_trips() {
    let cases = [
        (
            StreamKind::Run,
            StreamKey::run(run_id()),
            format!("run/{RUN}"),
        ),
        (
            StreamKind::Task,
            StreamKey::task(TaskId::from_str(TASK).expect("sample task id")),
            format!("task/{TASK}"),
        ),
        (
            StreamKind::Session,
            StreamKey::session(SessionId::from_str(SESSION).expect("sample session id")),
            format!("session/{SESSION}"),
        ),
        (
            StreamKind::Effect,
            StreamKey::effect(EffectId::from_str(EFFECT).expect("sample effect id")),
            format!("effect/{EFFECT}"),
        ),
        (
            StreamKind::ConfigGlobal,
            StreamKey::config_global(),
            "config/global".to_owned(),
        ),
        (
            StreamKind::Adapter,
            StreamKey::adapter(
                AdapterId::from_str(ADAPTER).expect("sample adapter id"),
                "1.0.0",
                "a1b2c3d4",
            ),
            format!("adapter/{ADAPTER}/1.0.0/a1b2c3d4"),
        ),
        (
            StreamKind::Principal,
            StreamKey::principal(PrincipalId::from_str(PRINCIPAL).expect("sample principal id")),
            format!("security/principal/{PRINCIPAL}"),
        ),
    ];

    for (kind, key, text) in cases {
        assert_eq!(key.kind(), kind);
        assert_eq!(key.as_str(), text);
        assert_eq!(key.to_string(), text);
        let parsed = StreamKey::from_str(&text).expect("canonical key parses");
        assert_eq!(parsed, key);
        assert_eq!(parsed.kind(), kind);
    }
}

#[test]
fn stream_key_parsing_rejects_non_canonical_forms() {
    let rejected = [
        "",
        "run",
        "run/",
        "run//x",
        "run/x/y",
        "runs/x",
        "task",
        "task/",
        "task/x/y",
        "session/",
        "effect//x",
        "config",
        "config/global/x",
        "config/",
        "adapter/x/1.0.0",
        "adapter/x//1.0.0",
        "adapter/x/1.0.0/a/b",
        "security/principal",
        "security/principal/",
        "security/principal/x/y",
        "security/other/x",
        "Run/x",
        "run/UPPER",
        "unknown/x",
    ];

    for text in rejected {
        let error =
            StreamKey::from_str(text).expect_err("non-canonical stream key must be refused");
        assert_eq!(error.code(), ErrorCode::InvalidArgument, "{text:?}");
        assert_eq!(error.retry_class(), RetryClass::Never, "{text:?}");
    }
}

#[test]
fn cursors_round_trip_and_malformed_forms_fail_closed() {
    let key = StreamKey::run(run_id());
    let cursor = EventCursor::for_event(&key, 42);
    assert_eq!(cursor.to_string(), format!("v1:run/{RUN}:42"));
    assert_eq!(cursor.stream_key.as_str(), key.as_str());
    assert_eq!(cursor.sequence, 42);
    let parsed = EventCursor::from_str(&cursor.to_string()).expect("canonical cursor parses");
    assert_eq!(parsed, cursor);

    let malformed = [
        "",
        "v1",
        "v1:",
        "v1::3",
        "v2:run/x:1",
        "v1:run/x",
        "v1:run/x:",
        "v1:run/x:00",
        "v1:run/x:01",
        "v1:run/x:-1",
        "v1:run/x:1x",
        "v1:run/x:1:extra",
        "V1:run/x:1",
        "v1:run/x: 1",
    ];

    for text in malformed {
        assert!(
            EventCursor::from_str(text).is_err(),
            "{text:?} must be refused"
        );
    }
}

#[test]
fn builder_requires_every_required_field() {
    let cases = [
        (
            "event type",
            EventBuilder::new("", 1, StreamKey::run(run_id()))
                .event_id(event_id())
                .sequence(1)
                .occurred_at_ms(1)
                .sensitivity(SensitivityClass::Internal)
                .payload(vec![]),
        ),
        (
            "event version",
            EventBuilder::new("RunStarted", 0, StreamKey::run(run_id()))
                .event_id(event_id())
                .sequence(1)
                .occurred_at_ms(1)
                .sensitivity(SensitivityClass::Internal)
                .payload(vec![]),
        ),
        (
            "sequence",
            EventBuilder::new("RunStarted", 1, StreamKey::run(run_id()))
                .event_id(event_id())
                .occurred_at_ms(1)
                .sensitivity(SensitivityClass::Internal)
                .payload(vec![]),
        ),
        (
            "occurred time",
            EventBuilder::new("RunStarted", 1, StreamKey::run(run_id()))
                .event_id(event_id())
                .sequence(1)
                .sensitivity(SensitivityClass::Internal)
                .payload(vec![]),
        ),
        (
            "classification",
            EventBuilder::new("RunStarted", 1, StreamKey::run(run_id()))
                .event_id(event_id())
                .sequence(1)
                .occurred_at_ms(1)
                .payload(vec![]),
        ),
        (
            "classification presence",
            EventBuilder::new("RunStarted", 1, StreamKey::run(run_id()))
                .event_id(event_id())
                .sequence(1)
                .occurred_at_ms(1)
                .sensitivity(SensitivityClass::Unspecified)
                .payload(vec![]),
        ),
        (
            "payload",
            EventBuilder::new("RunStarted", 1, StreamKey::run(run_id()))
                .event_id(event_id())
                .sequence(1)
                .occurred_at_ms(1)
                .sensitivity(SensitivityClass::Internal),
        ),
    ];

    for (field, builder) in cases {
        let error = builder
            .build(&policy())
            .expect_err("incomplete event must be refused");
        assert_eq!(error.code(), ErrorCode::InvalidArgument, "{field}");
        assert_eq!(error.retry_class(), RetryClass::Never, "{field}");
    }
}

#[test]
fn builder_rejects_classification_downgrades() {
    let canary = "canary-payload-bytes";
    let principal = || StreamKey::principal(PrincipalId::from_str(PRINCIPAL).expect("principal"));
    let incomplete = || {
        EventBuilder::new("CapabilityRequested", 1, principal())
            .event_id(event_id())
            .sequence(3)
            .occurred_at_ms(5)
            .payload(canary.as_bytes().to_vec())
    };

    for downgrade in [SensitivityClass::Public, SensitivityClass::Internal] {
        let error = incomplete()
            .sensitivity(downgrade)
            .build(&policy())
            .expect_err("downgrade must be refused");
        assert_eq!(error.code(), ErrorCode::FailedPrecondition);
        assert_eq!(error.retry_class(), RetryClass::Never);
        assert!(
            !error.to_string().contains(canary),
            "payload bytes must never appear in errors: {error}"
        );
    }

    for allowed in [SensitivityClass::Confidential, SensitivityClass::Secret] {
        let envelope = incomplete()
            .sensitivity(allowed)
            .build(&policy())
            .expect("at or above the floor is allowed");
        assert_eq!(envelope.sensitivity, allowed);
    }
}

#[test]
fn embedded_policy_reads_floors_from_the_catalog() {
    let policy = policy();

    assert_eq!(
        policy.minimum("CapabilityRequested"),
        Some(SensitivityClass::Confidential)
    );
    assert_eq!(
        policy.minimum("SecretActionPerformed"),
        Some(SensitivityClass::Confidential)
    );
    assert_eq!(
        policy.minimum("SessionCreated"),
        Some(SensitivityClass::Internal)
    );
    assert_eq!(
        policy.minimum("RunStarted"),
        Some(SensitivityClass::Internal)
    );
    assert_eq!(
        policy.minimum("AdapterRegistered"),
        Some(SensitivityClass::Internal)
    );
    assert_eq!(policy.minimum("NoSuchEventType"), None);

    let envelope = builder("NoSuchEventType")
        .sensitivity(SensitivityClass::Public)
        .build(&policy)
        .expect("unknown event types have no floor");
    assert_eq!(envelope.sensitivity, SensitivityClass::Public);
}

#[test]
fn envelope_round_trips_every_field() {
    let stream = StreamKey::effect(EffectId::from_str(EFFECT).expect("sample effect id"));
    let payload = vec![0x00, 0xff, 0x80, 0x07, b'{', b'}'];
    let envelope = EventBuilder::new("EffectPrepared", 1, stream.clone())
        .event_id(event_id())
        .sequence(9)
        .occurred_at_ms(1_700_000_000_999)
        .run_id(run_id())
        .task_id(TaskId::from_str(TASK).expect("sample task id"))
        .session_id(SessionId::from_str(SESSION).expect("sample session id"))
        .effect_id(EffectId::from_str(EFFECT).expect("sample effect id"))
        .causation_id(CAUSE)
        .correlation_id("corr-9")
        .sensitivity(SensitivityClass::Confidential)
        .retention(RetentionClass::Audit)
        .payload(payload.clone())
        .build(&policy())
        .expect("complete event builds");

    let bytes = envelope.to_bytes();
    let decoded = EventEnvelope::from_bytes(&bytes).expect("envelope bytes decode");
    assert_eq!(decoded, envelope);
    assert_eq!(decoded.event_id, event_id());
    assert_eq!(decoded.event_type, "EffectPrepared");
    assert_eq!(decoded.event_version, 1);
    assert_eq!(decoded.stream_key, stream);
    assert_eq!(decoded.sequence, 9);
    assert_eq!(decoded.occurred_unix_ms, 1_700_000_000_999);
    assert_eq!(decoded.run_id, Some(run_id()));
    assert_eq!(
        decoded.task_id,
        Some(TaskId::from_str(TASK).expect("sample task id"))
    );
    assert_eq!(
        decoded.session_id,
        Some(SessionId::from_str(SESSION).expect("sample session id"))
    );
    assert_eq!(
        decoded.effect_id,
        Some(EffectId::from_str(EFFECT).expect("sample effect id"))
    );
    assert_eq!(decoded.causation_id.as_deref(), Some(CAUSE));
    assert_eq!(decoded.correlation_id.as_deref(), Some("corr-9"));
    assert_eq!(decoded.sensitivity, SensitivityClass::Confidential);
    assert_eq!(decoded.retention, RetentionClass::Audit);
    assert_eq!(decoded.payload, payload);
    assert_eq!(decoded.cursor(), EventCursor::for_event(&stream, 9));
    assert_eq!(
        decoded.cursor().to_string(),
        format!("v1:effect/{EFFECT}:9")
    );

    let minimal = EventBuilder::new("RunStarted", 1, StreamKey::run(run_id()))
        .event_id(event_id())
        .sequence(0)
        .occurred_at_ms(0)
        .sensitivity(SensitivityClass::Internal)
        .payload(Vec::new())
        .build(&policy())
        .expect("minimal event builds");
    let decoded_minimal =
        EventEnvelope::from_bytes(&minimal.to_bytes()).expect("minimal envelope decodes");
    assert_eq!(decoded_minimal, minimal);
    assert!(decoded_minimal.run_id.is_none());
    assert!(decoded_minimal.task_id.is_none());
    assert!(decoded_minimal.session_id.is_none());
    assert!(decoded_minimal.effect_id.is_none());
    assert!(decoded_minimal.causation_id.is_none());
    assert!(decoded_minimal.correlation_id.is_none());
}

#[test]
fn envelope_decoding_fails_closed_without_echoing_payload() {
    let canary = b"payload-canary";

    let error = EventEnvelope::from_bytes(b"payload-canary-not-an-envelope")
        .expect_err("bogus envelope bytes must be refused");
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert_eq!(error.retry_class(), RetryClass::Never);
    assert!(
        !error.to_string().contains("payload-canary"),
        "input bytes must never appear in errors: {error}"
    );

    let unspecified = domain::generated::contract::EventEnvelope {
        event_id: EVENT.to_owned(),
        event_type: "RunStarted".to_owned(),
        event_version: 1,
        stream_key: format!("run/{RUN}"),
        sequence: 1,
        occurred_unix_ms: 7,
        run_id: String::new(),
        task_id: String::new(),
        session_id: String::new(),
        effect_id: String::new(),
        causation_id: String::new(),
        correlation_id: String::new(),
        sensitivity: 0,
        retention: 2,
        payload: canary.to_vec(),
    };
    let error = EventEnvelope::from_bytes(&prost::Message::encode_to_vec(&unspecified))
        .expect_err("unspecified sensitivity must be refused");
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
    assert!(!error.to_string().contains("payload-canary"));

    let unknown_retention = domain::generated::contract::EventEnvelope {
        sensitivity: 2,
        retention: 9,
        ..unspecified
    };
    let error = EventEnvelope::from_bytes(&prost::Message::encode_to_vec(&unknown_retention))
        .expect_err("unknown retention must be refused");
    assert_eq!(error.code(), ErrorCode::InvalidArgument);

    let bad_stream_key = domain::generated::contract::EventEnvelope {
        sensitivity: 2,
        retention: 2,
        stream_key: "nonsense".to_owned(),
        ..unknown_retention
    };
    let error = EventEnvelope::from_bytes(&prost::Message::encode_to_vec(&bad_stream_key))
        .expect_err("non-canonical stream key must be refused");
    assert_eq!(error.code(), ErrorCode::InvalidArgument);
}
