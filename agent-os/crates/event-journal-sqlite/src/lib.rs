//! SQLite event journal implementing the event journal port.
//!
//! `events.db` is a separate database from `kernel.db`: it bootstraps from the
//! embedded inception schema, is created `0600` inside a `0700` directory, and
//! shares no tables with canonical kernel storage. Appends run in
//! `BEGIN IMMEDIATE` transactions so the head check and the inserts are one
//! writer-serialized unit, with the unique `(stream_key, sequence)` constraint
//! as the final authority under races.

#![forbid(unsafe_code)]

use std::fs;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use event_journal::{AppendResult, EventJournalPort, ReadResult};
use events::{EventEnvelope, StreamKey};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Connection, SqliteConnection, SqlitePool};

/// The inception schema for `events.db`, executed verbatim on first create.
const SCHEMA: &str = include_str!("../../../schema/event_journal.sql");

/// SQLite primary result code for constraint failures.
const SQLITE_CONSTRAINT: i32 = 19;
/// Extended code for a `UNIQUE` constraint failure.
const SQLITE_CONSTRAINT_UNIQUE: i32 = 2067;
/// Extended code for a primary-key constraint failure.
const SQLITE_CONSTRAINT_PRIMARYKEY: i32 = 1555;

/// Configuration for [`SqliteEventJournal::open`].
#[derive(Clone, Debug)]
pub struct JournalConfig {
    /// Path of the `events.db` file; its parent directory is the runtime directory.
    pub path: PathBuf,
    /// SQLite busy timeout applied to every pooled connection, in milliseconds.
    pub busy_timeout_ms: u64,
}

/// SQLite-backed event journal.
#[derive(Debug)]
pub struct SqliteEventJournal {
    pool: SqlitePool,
    path: PathBuf,
}

impl SqliteEventJournal {
    /// Creates `events.db` from the inception schema on first open, otherwise opens it.
    ///
    /// The runtime directory (the parent of `config.path`) is created with mode `0700`
    /// and the database file with mode `0600`; both modes are re-asserted on every open.
    pub async fn open(config: JournalConfig) -> errors::Result<Self> {
        let JournalConfig {
            path,
            busy_timeout_ms,
        } = config;
        let runtime_dir = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .ok_or_else(|| {
                KernelError::new(
                    ErrorCode::FailedPrecondition,
                    RetryClass::Never,
                    "event journal path has no parent directory",
                )
            })?;
        prepare_runtime_dir(runtime_dir)?;
        let created = prepare_database_file(&path)?;
        let options = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(false)
            .busy_timeout(Duration::from_millis(busy_timeout_ms))
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full)
            .foreign_keys(true);
        if created {
            bootstrap_schema(&options).await?;
        }
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(|source| {
                KernelError::new(
                    ErrorCode::Unavailable,
                    RetryClass::Safe,
                    "event journal connection failed",
                )
                .with_source(source)
            })?;
        Ok(Self { pool, path })
    }

    /// Returns the path of the `events.db` file this journal opened.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[async_trait]
impl EventJournalPort for SqliteEventJournal {
    async fn append(
        &self,
        stream_key: &StreamKey,
        expected_sequence: u64,
        batch: &[EventEnvelope],
    ) -> errors::Result<AppendResult> {
        let final_sequence = validate_batch(stream_key, expected_sequence, batch)?;
        let mut txn = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(from_sqlx)?;
        let head = read_head(&mut txn, stream_key).await?;
        if head == expected_sequence {
            for envelope in batch {
                insert_envelope(&mut txn, envelope).await?;
            }
        } else {
            verify_recorded_batch(&mut txn, stream_key, batch).await?;
        }
        txn.commit().await.map_err(from_sqlx)?;
        Ok(AppendResult { final_sequence })
    }

    async fn read_stream(
        &self,
        stream_key: &StreamKey,
        from_sequence: u64,
        limit: u32,
    ) -> errors::Result<ReadResult> {
        let from = encode_sequence(from_sequence)?;
        let rows: Vec<(String, String, i64, Vec<u8>)> = sqlx::query_as(
            "SELECT event_id, stream_key, sequence, envelope FROM events \
             WHERE stream_key = ? AND sequence > ? ORDER BY sequence ASC LIMIT ?",
        )
        .bind(stream_key.as_str())
        .bind(from)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await
        .map_err(from_sqlx)?;

        let mut events = Vec::with_capacity(rows.len());
        for (event_id, stored_stream, stored_sequence, bytes) in rows {
            let decoded = EventEnvelope::from_bytes(&bytes)?;
            let stored_sequence = u64::try_from(stored_sequence)
                .map_err(|_| corruption("event journal row holds a negative sequence"))?;
            if stored_stream != stream_key.as_str()
                || decoded.sequence != stored_sequence
                || decoded.event_id.to_hyphenated() != event_id
            {
                return Err(corruption(
                    "event journal row does not match its recorded envelope",
                ));
            }
            events.push(decoded);
        }
        Ok(ReadResult {
            events,
            retention_gap: false,
        })
    }
}

/// Validates the batch shape and returns the final position of the batch.
fn validate_batch(
    stream_key: &StreamKey,
    expected_sequence: u64,
    batch: &[EventEnvelope],
) -> errors::Result<u64> {
    if batch.is_empty() {
        return Err(invalid("event journal append batch is empty"));
    }
    let mut next = expected_sequence
        .checked_add(1)
        .ok_or_else(|| invalid("event journal expected sequence overflows"))?;
    for envelope in batch {
        if envelope.stream_key != *stream_key {
            return Err(invalid(
                "event journal batch envelope belongs to a different stream",
            ));
        }
        if envelope.sequence != next {
            return Err(invalid(
                "event journal batch sequences are not contiguous from expected_sequence + 1",
            ));
        }
        next = next
            .checked_add(1)
            .ok_or_else(|| invalid("event journal batch sequence overflows"))?;
    }
    Ok(next - 1)
}

/// Reads the stream head inside an open write transaction.
async fn read_head(conn: &mut SqliteConnection, stream_key: &StreamKey) -> errors::Result<u64> {
    let head: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM events WHERE stream_key = ?")
            .bind(stream_key.as_str())
            .fetch_one(conn)
            .await
            .map_err(from_sqlx)?;
    u64::try_from(head).map_err(|_| corruption("event journal stream head is negative"))
}

/// Inserts one envelope through the embedded schema's column set.
async fn insert_envelope(
    conn: &mut SqliteConnection,
    envelope: &EventEnvelope,
) -> errors::Result<()> {
    sqlx::query(
        "INSERT INTO events \
         (event_id, stream_key, sequence, event_type, event_version, occurred_at_ms, envelope) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(envelope.event_id.to_hyphenated())
    .bind(envelope.stream_key.as_str())
    .bind(encode_sequence(envelope.sequence)?)
    .bind(&envelope.event_type)
    .bind(i64::from(envelope.event_version))
    .bind(envelope.occurred_unix_ms)
    .bind(envelope.to_bytes())
    .execute(conn)
    .await
    .map_err(from_sqlx)?;
    Ok(())
}

/// Accepts a batch that is already recorded exactly, rejecting any divergence.
///
/// A missing position means the head is behind the batch expectation
/// (`FailedPrecondition`); a recorded position with another event id or other
/// bytes is a `Conflict`. Reading and writing nothing is the idempotent path.
async fn verify_recorded_batch(
    conn: &mut SqliteConnection,
    stream_key: &StreamKey,
    batch: &[EventEnvelope],
) -> errors::Result<()> {
    for envelope in batch {
        let existing: Option<(String, Vec<u8>)> = sqlx::query_as(
            "SELECT event_id, envelope FROM events WHERE stream_key = ? AND sequence = ?",
        )
        .bind(stream_key.as_str())
        .bind(encode_sequence(envelope.sequence)?)
        .fetch_optional(&mut *conn)
        .await
        .map_err(from_sqlx)?;
        match existing {
            None => {
                return Err(KernelError::new(
                    ErrorCode::FailedPrecondition,
                    RetryClass::Never,
                    "event journal append expected sequence does not match the stream head",
                ));
            }
            Some((event_id, bytes)) => {
                if event_id != envelope.event_id.to_hyphenated() {
                    return Err(KernelError::new(
                        ErrorCode::Conflict,
                        RetryClass::Never,
                        "event journal position already holds a different event",
                    ));
                }
                if bytes != envelope.to_bytes() {
                    return Err(KernelError::new(
                        ErrorCode::Conflict,
                        RetryClass::Never,
                        "event journal position already holds different event bytes",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn prepare_runtime_dir(dir: &Path) -> errors::Result<()> {
    fs::create_dir_all(dir).map_err(|source| {
        KernelError::new(
            ErrorCode::Unavailable,
            RetryClass::Safe,
            "event journal runtime directory could not be created",
        )
        .with_source(source)
    })?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|source| {
        KernelError::new(
            ErrorCode::Unavailable,
            RetryClass::Safe,
            "event journal runtime directory permissions could not be set",
        )
        .with_source(source)
    })
}

fn prepare_database_file(path: &Path) -> errors::Result<bool> {
    let created = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(_) => true,
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(source) => {
            return Err(KernelError::new(
                ErrorCode::Unavailable,
                RetryClass::Safe,
                "event journal file could not be created",
            )
            .with_source(source));
        }
    };
    if !created && !path.is_file() {
        return Err(KernelError::new(
            ErrorCode::FailedPrecondition,
            RetryClass::Never,
            "event journal path is not a regular file",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| {
        KernelError::new(
            ErrorCode::Unavailable,
            RetryClass::Safe,
            "event journal file permissions could not be set",
        )
        .with_source(source)
    })?;
    Ok(created)
}

async fn bootstrap_schema(options: &SqliteConnectOptions) -> errors::Result<()> {
    let mut connection = SqliteConnection::connect_with(options)
        .await
        .map_err(|source| {
            KernelError::new(
                ErrorCode::Unavailable,
                RetryClass::Safe,
                "event journal schema bootstrap connection failed",
            )
            .with_source(source)
        })?;
    sqlx::raw_sql(SCHEMA)
        .execute(&mut connection)
        .await
        .map_err(|source| {
            KernelError::new(
                ErrorCode::Unavailable,
                RetryClass::Safe,
                "event journal schema bootstrap failed",
            )
            .with_source(source)
        })?;
    connection.close().await.map_err(|source| {
        KernelError::new(
            ErrorCode::Unavailable,
            RetryClass::Safe,
            "event journal schema bootstrap connection failed to close",
        )
        .with_source(source)
    })
}

fn encode_sequence(sequence: u64) -> errors::Result<i64> {
    i64::try_from(sequence)
        .map_err(|_| invalid("event journal sequence exceeds the SQLite integer range"))
}

/// Converts a `sqlx` failure into the stable kernel taxonomy (persistence
/// `mapping` conventions: unique and primary-key violations are conflicts,
/// other constraints are failed preconditions, and busy or I/O failures are
/// retry-safe unavailability).
fn from_sqlx(source: sqlx::Error) -> KernelError {
    let (code, retry) = classify(&source);
    KernelError::new(code, retry, message(code)).with_source(source)
}

fn classify(source: &sqlx::Error) -> (ErrorCode, RetryClass) {
    if let Some(database) = source.as_database_error() {
        return match database.code().and_then(|code| code.parse::<i32>().ok()) {
            Some(code) => classify_sqlite(code),
            None => (ErrorCode::Internal, RetryClass::Never),
        };
    }
    match source {
        sqlx::Error::PoolClosed | sqlx::Error::PoolTimedOut | sqlx::Error::Io(_) => {
            (ErrorCode::Unavailable, RetryClass::Safe)
        }
        sqlx::Error::RowNotFound => (ErrorCode::NotFound, RetryClass::Never),
        _ => (ErrorCode::Internal, RetryClass::Never),
    }
}

fn classify_sqlite(code: i32) -> (ErrorCode, RetryClass) {
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
        // SQLITE_BUSY (5) and SQLITE_LOCKED (6), including their extended codes.
        5 | 6 => (ErrorCode::Unavailable, RetryClass::Safe),
        // SQLITE_IOERR (10) and SQLITE_CANTOPEN (14) are retry-safe availability failures.
        10 | 14 => (ErrorCode::Unavailable, RetryClass::Safe),
        _ => (ErrorCode::Internal, RetryClass::Never),
    }
}

fn message(code: ErrorCode) -> &'static str {
    match code {
        ErrorCode::Conflict => "event journal constraint conflict",
        ErrorCode::FailedPrecondition => "event journal constraint rejected the write",
        ErrorCode::Unavailable => "event journal storage is unavailable",
        ErrorCode::NotFound => "event journal row is missing",
        _ => "event journal storage operation failed",
    }
}

fn invalid(detail: &'static str) -> KernelError {
    KernelError::new(ErrorCode::InvalidArgument, RetryClass::Never, detail)
}

fn corruption(detail: &'static str) -> KernelError {
    KernelError::new(ErrorCode::Internal, RetryClass::Never, detail)
}
