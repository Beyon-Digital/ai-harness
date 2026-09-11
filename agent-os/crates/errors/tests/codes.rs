use std::error::Error as StdError;

use errors::codes::{ErrorCode, RetryClass};
use errors::{KernelError, Result};

const ALL_CODES: [ErrorCode; 7] = [
    ErrorCode::InvalidArgument,
    ErrorCode::NotFound,
    ErrorCode::Conflict,
    ErrorCode::FailedPrecondition,
    ErrorCode::ResourceExhausted,
    ErrorCode::Unavailable,
    ErrorCode::Internal,
];

#[test]
fn same_code_produces_the_same_string() {
    let expected = [
        (ErrorCode::InvalidArgument, "invalid_argument"),
        (ErrorCode::NotFound, "not_found"),
        (ErrorCode::Conflict, "conflict"),
        (ErrorCode::FailedPrecondition, "failed_precondition"),
        (ErrorCode::ResourceExhausted, "resource_exhausted"),
        (ErrorCode::Unavailable, "unavailable"),
        (ErrorCode::Internal, "internal"),
    ];

    for (code, token) in expected {
        assert_eq!(code.as_str(), token);
        assert_eq!(code.as_str(), code.to_string());
        assert_eq!(code.as_str(), format!("{code}"));
    }

    let mut tokens: Vec<&str> = ALL_CODES.iter().map(|code| code.as_str()).collect();
    tokens.sort_unstable();
    tokens.dedup();
    assert_eq!(tokens.len(), ALL_CODES.len(), "tokens are not unique");
}

#[test]
fn reconciliation_required_is_structurally_distinct_from_safe() {
    assert_ne!(RetryClass::Never, RetryClass::Safe);
    assert_ne!(RetryClass::Never, RetryClass::ReconciliationRequired);
    assert_ne!(RetryClass::Safe, RetryClass::ReconciliationRequired);

    let safe = KernelError::new(ErrorCode::Unavailable, RetryClass::Safe, "retry me");
    let reconcile = KernelError::new(
        ErrorCode::Conflict,
        RetryClass::ReconciliationRequired,
        "reconcile me",
    );

    assert_eq!(safe.retry_class(), RetryClass::Safe);
    assert_eq!(reconcile.retry_class(), RetryClass::ReconciliationRequired);
    assert_ne!(safe.retry_class(), reconcile.retry_class());
}

#[test]
fn source_is_preserved_through_error_source() {
    let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "locked by policy");
    let err = KernelError::new(ErrorCode::Internal, RetryClass::Never, "io failed").with_source(io);

    let source = err.source().expect("source must be preserved");
    assert_eq!(source.to_string(), "locked by policy");
    let io_source = source
        .downcast_ref::<std::io::Error>()
        .expect("source must remain a std::io::Error");
    assert_eq!(io_source.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn display_contains_code_token_and_never_the_source() {
    let err = KernelError::new(
        ErrorCode::FailedPrecondition,
        RetryClass::ReconciliationRequired,
        "guard rejected the command",
    )
    .with_source(std::io::Error::other("SECRET_PAYLOAD_DO_NOT_PRINT"));

    let rendered = err.to_string();
    assert_eq!(rendered, "failed_precondition: guard rejected the command");
    assert!(rendered.contains(ErrorCode::FailedPrecondition.as_str()));
    assert!(!rendered.contains("SECRET_PAYLOAD_DO_NOT_PRINT"));
    assert!(!rendered.contains("Other"));

    assert_eq!(err.code(), ErrorCode::FailedPrecondition);
    assert_eq!(err.retry_class(), RetryClass::ReconciliationRequired);
    assert_eq!(err.message(), "guard rejected the command");
}

#[test]
fn every_code_and_retry_class_survives_formatting() {
    for code in ALL_CODES {
        for retry in [
            RetryClass::Never,
            RetryClass::Safe,
            RetryClass::ReconciliationRequired,
        ] {
            let err = KernelError::new(code, retry, "boom");
            let rendered = err.to_string();
            assert!(rendered.starts_with(code.as_str()));
            assert!(rendered.contains(code.as_str()));
            assert_eq!(err.code(), code);
            assert_eq!(err.retry_class(), retry);
        }
    }
}

#[test]
fn result_alias_uses_kernel_error() {
    fn fail() -> Result<()> {
        Err(KernelError::new(
            ErrorCode::NotFound,
            RetryClass::Never,
            "missing",
        ))
    }

    match fail() {
        Err(err) => assert_eq!(err.code(), ErrorCode::NotFound),
        Ok(()) => panic!("expected failure"),
    }
}
