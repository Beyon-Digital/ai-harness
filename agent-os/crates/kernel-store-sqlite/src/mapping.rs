//! SQLite error classification and persisted-value decoding helpers.
//!
//! `sqlx` errors never cross the store port: every database failure is
//! classified into the stable foundation taxonomy (design `Error handling`,
//! decision D5) and every persisted value is decoded through the domain mirror
//! types, failing closed with `Internal`/`Never` on unknown values (R3.1).

use std::fmt::Display;
use std::str::FromStr;

use domain::run::{UnknownEnumValue, UnknownStateValue};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use sqlx::sqlite::SqliteRow;
use sqlx::{Error as SqlxError, Row};

/// SQLite primary result code for constraint failures.
const SQLITE_CONSTRAINT: i32 = 19;
/// Extended code for a `UNIQUE` constraint failure.
const SQLITE_CONSTRAINT_UNIQUE: i32 = 2067;
/// Extended code for a primary-key constraint failure.
const SQLITE_CONSTRAINT_PRIMARYKEY: i32 = 1555;

/// Converts a `sqlx` failure into the stable kernel error taxonomy.
pub(crate) fn from_sqlx(source: SqlxError) -> KernelError {
    let (code, retry) = classify(&source);
    KernelError::new(code, retry, message(code)).with_source(source)
}

/// Classifies a `sqlx` failure without consuming it.
pub(crate) fn classify(source: &SqlxError) -> (ErrorCode, RetryClass) {
    if let Some(database) = source.as_database_error() {
        return match database.code().and_then(|code| code.parse::<i32>().ok()) {
            Some(code) => classify_sqlite(code),
            None => (ErrorCode::Internal, RetryClass::Never),
        };
    }
    match source {
        SqlxError::PoolClosed | SqlxError::PoolTimedOut | SqlxError::Io(_) => {
            (ErrorCode::Unavailable, RetryClass::Safe)
        }
        SqlxError::RowNotFound => (ErrorCode::NotFound, RetryClass::Never),
        _ => (ErrorCode::Internal, RetryClass::Never),
    }
}

/// Classifies a raw SQLite extended result code.
pub(crate) fn classify_sqlite(code: i32) -> (ErrorCode, RetryClass) {
    match code & 0xFF {
        SQLITE_CONSTRAINT
            if matches!(
                code,
                SQLITE_CONSTRAINT_UNIQUE | SQLITE_CONSTRAINT_PRIMARYKEY
            ) =>
        {
            (ErrorCode::Conflict, RetryClass::Never)
        }
        SQLITE_CONSTRAINT => (ErrorCode::FailedPrecondition, RetryClass::Never),
        // SQLITE_BUSY (5) and SQLITE_LOCKED (6), including their extended codes (D5).
        5 | 6 => (ErrorCode::Unavailable, RetryClass::Safe),
        // SQLITE_IOERR (10) and SQLITE_CANTOPEN (14) are retry-safe availability failures.
        10 | 14 => (ErrorCode::Unavailable, RetryClass::Safe),
        _ => (ErrorCode::Internal, RetryClass::Never),
    }
}

fn message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Conflict => "storage constraint conflict",
        ErrorCode::FailedPrecondition => "storage constraint rejected the write",
        ErrorCode::Unavailable => "storage is unavailable",
        ErrorCode::NotFound => "required row is missing",
        _ => "storage operation failed",
    }
}

/// Fails closed for a persisted enum value with no domain mirror variant.
pub(crate) fn decode_wire<T>(
    column: &'static str,
    value: i64,
    decode: fn(i32) -> Result<T, UnknownEnumValue>,
) -> errors::Result<T> {
    let value =
        i32::try_from(value).map_err(|_| invariant(column, "is outside the enum wire range"))?;
    decode(value).map_err(|error| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            format!("{column}: {error}"),
        )
    })
}

/// Fails closed for a persisted state string with no domain mirror variant.
pub(crate) fn decode_state<T>(
    column: &'static str,
    value: &str,
    decode: fn(&str) -> Result<T, UnknownStateValue>,
) -> errors::Result<T> {
    decode(value).map_err(|error| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            format!("{column}: {error}"),
        )
    })
}

/// Decodes a persisted identifier through its domain newtype.
pub(crate) fn decode_id<T>(column: &'static str, value: &str) -> errors::Result<T>
where
    T: FromStr,
    T::Err: Display,
{
    value.parse().map_err(|error| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            format!("{column} is not a valid identifier: {error}"),
        )
    })
}

/// Decodes an optional persisted identifier.
pub(crate) fn decode_opt_id<T>(
    column: &'static str,
    value: Option<String>,
) -> errors::Result<Option<T>>
where
    T: FromStr,
    T::Err: Display,
{
    value.map(|value| decode_id(column, &value)).transpose()
}

/// Decodes a non-negative persisted integer.
pub(crate) fn decode_u64(column: &'static str, value: i64) -> errors::Result<u64> {
    u64::try_from(value).map_err(|_| invariant(column, "is negative"))
}

/// Encodes a caller-supplied counter for storage.
pub(crate) fn encode_u64(column: &'static str, value: u64) -> errors::Result<i64> {
    i64::try_from(value).map_err(|_| {
        KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            format!("{column} exceeds the SQLite integer range"),
        )
    })
}

/// Reads a non-null `TEXT` column.
pub(crate) fn text(row: &SqliteRow, column: &'static str) -> errors::Result<String> {
    row.try_get(column).map_err(|source| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            format!("column {column} is not readable as TEXT"),
        )
        .with_source(source)
    })
}

/// Reads a nullable `TEXT` column.
pub(crate) fn opt_text(row: &SqliteRow, column: &'static str) -> errors::Result<Option<String>> {
    row.try_get(column).map_err(|source| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            format!("column {column} is not readable as TEXT"),
        )
        .with_source(source)
    })
}

/// Reads a non-null `INTEGER` column.
pub(crate) fn int(row: &SqliteRow, column: &'static str) -> errors::Result<i64> {
    row.try_get(column).map_err(|source| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            format!("column {column} is not readable as INTEGER"),
        )
        .with_source(source)
    })
}

/// Reads a nullable `INTEGER` column.
pub(crate) fn opt_int(row: &SqliteRow, column: &'static str) -> errors::Result<Option<i64>> {
    row.try_get(column).map_err(|source| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            format!("column {column} is not readable as INTEGER"),
        )
        .with_source(source)
    })
}

/// Reads a non-null `BLOB` column.
pub(crate) fn blob(row: &SqliteRow, column: &'static str) -> errors::Result<Vec<u8>> {
    row.try_get(column).map_err(|source| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            format!("column {column} is not readable as BLOB"),
        )
        .with_source(source)
    })
}

/// Reads a nullable `BLOB` column.
pub(crate) fn opt_blob(row: &SqliteRow, column: &'static str) -> errors::Result<Option<Vec<u8>>> {
    row.try_get(column).map_err(|source| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            format!("column {column} is not readable as BLOB"),
        )
        .with_source(source)
    })
}

/// Current UTC Unix milliseconds.
pub(crate) fn unix_ms_now() -> errors::Result<i64> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| {
            KernelError::new(
                ErrorCode::Internal,
                RetryClass::Never,
                "system clock is before the unix epoch",
            )
        })?;
    i64::try_from(elapsed.as_millis()).map_err(|_| {
        KernelError::new(
            ErrorCode::Internal,
            RetryClass::Never,
            "system clock exceeds the unix millisecond range",
        )
    })
}

fn invariant(column: &'static str, detail: &'static str) -> KernelError {
    KernelError::new(
        ErrorCode::Internal,
        RetryClass::Never,
        format!("{column} {detail}"),
    )
}

#[cfg(test)]
mod tests {
    use domain::run::RunState;
    use errors::codes::{ErrorCode, RetryClass};

    use super::{classify_sqlite, decode_id, decode_u64, decode_wire};

    #[test]
    fn unique_and_primary_key_codes_are_conflicts() {
        for code in [2067, 1555] {
            assert_eq!(
                classify_sqlite(code),
                (ErrorCode::Conflict, RetryClass::Never),
                "code {code}"
            );
        }
    }

    #[test]
    fn constraint_codes_are_failed_preconditions() {
        // Base constraint, CHECK, FOREIGN KEY, NOT NULL, TRIGGER, ROWID.
        for code in [19, 275, 787, 1299, 1811, 2579] {
            assert_eq!(
                classify_sqlite(code),
                (ErrorCode::FailedPrecondition, RetryClass::Never),
                "code {code}"
            );
        }
    }

    #[test]
    fn busy_and_locked_codes_are_retry_safe_unavailable() {
        // BUSY, BUSY_RECOVERY, BUSY_SNAPSHOT, BUSY_TIMEOUT, LOCKED, LOCKED_SHAREDCACHE.
        for code in [5, 261, 517, 773, 6, 262] {
            assert_eq!(
                classify_sqlite(code),
                (ErrorCode::Unavailable, RetryClass::Safe),
                "code {code}"
            );
        }
    }

    #[test]
    fn unknown_codes_are_internal() {
        assert_eq!(
            classify_sqlite(999),
            (ErrorCode::Internal, RetryClass::Never)
        );
    }

    #[test]
    fn unknown_persisted_enums_fail_closed() {
        let error = decode_wire("runs.state", 99, RunState::from_wire).unwrap_err();
        assert_eq!(error.code(), ErrorCode::Internal);
        assert_eq!(error.retry_class(), RetryClass::Never);

        let overflow = decode_wire("runs.state", i64::MAX, RunState::from_wire).unwrap_err();
        assert_eq!(overflow.code(), ErrorCode::Internal);
    }

    #[test]
    fn malformed_identifiers_and_negative_counters_fail_closed() {
        let id = decode_id::<domain::ids::RunId>("runs.run_id", "not-a-uuid").unwrap_err();
        assert_eq!(id.code(), ErrorCode::Internal);
        assert_eq!(id.retry_class(), RetryClass::Never);

        let negative = decode_u64("runs.run_revision", -1).unwrap_err();
        assert_eq!(negative.code(), ErrorCode::Internal);
    }
}
