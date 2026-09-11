//! Repository implementations over a shared SQLite connection.
//!
//! The groups implemented by the SQLite store are [`runs`], [`tasks`],
//! [`sessions`], [`graph`], and [`environments`]; each repository view shares
//! the connection owned by its transaction through [`SharedConn`].
//!
//! [`UnavailableRepo`] stands in for repository groups whose SQL
//! implementations arrive with later persistence tasks. Every operation on it
//! fails closed with `FailedPrecondition`/`Never`, so a caller can never
//! observe fabricated rows.

pub(crate) mod environments;
pub(crate) mod graph;
pub(crate) mod runs;
pub(crate) mod sessions;
pub(crate) mod tasks;

use std::sync::Arc;

use domain::effect::EffectState;
use domain::ids::*;
use domain::resource::{ReservationState, TimerState};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use sqlx::Sqlite;
use sqlx::pool::PoolConnection;
use sqlx::sqlite::SqliteConnection;

/// Shared handle to the write connection owned by a write transaction.
pub(crate) type WriteConn = Arc<tokio::sync::Mutex<Option<sqlx::Transaction<'static, Sqlite>>>>;

/// Shared handle to the read connection owned by a read transaction.
pub(crate) type ReadConn = Arc<tokio::sync::Mutex<Option<PoolConnection<Sqlite>>>>;

/// Connection shared by every repository view of one transaction.
#[derive(Clone)]
pub(crate) enum SharedConn {
    /// Connection inside an immediate write transaction.
    Write(WriteConn),
    /// Plain read connection.
    Read(ReadConn),
}

impl SharedConn {
    /// Locks the shared connection for the duration of one repository call.
    pub(crate) async fn lock(&self) -> ConnGuard<'_> {
        match self {
            Self::Write(conn) => ConnGuard::Write(conn.lock().await),
            Self::Read(conn) => ConnGuard::Read(conn.lock().await),
        }
    }
}

/// Held connection guard that yields the underlying connection.
pub(crate) enum ConnGuard<'a> {
    /// Guard over an immediate write transaction.
    Write(tokio::sync::MutexGuard<'a, Option<sqlx::Transaction<'static, Sqlite>>>),
    /// Guard over a plain read connection.
    Read(tokio::sync::MutexGuard<'a, Option<PoolConnection<Sqlite>>>),
}

impl ConnGuard<'_> {
    /// Borrows the live connection, or fails closed when the transaction has
    /// already been committed or rolled back.
    pub(crate) fn connection(&mut self) -> errors::Result<&mut SqliteConnection> {
        match self {
            Self::Write(guard) => guard.as_mut().map(|txn| &mut **txn),
            Self::Read(guard) => guard.as_mut().map(|conn| &mut **conn),
        }
        .ok_or_else(|| {
            KernelError::new(
                ErrorCode::FailedPrecondition,
                RetryClass::Never,
                "transaction connection is already finished",
            )
        })
    }
}

/// Repository view for groups without a SQL implementation in this store.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct UnavailableRepo;

use kernel_store::models::*;
use kernel_store::repositories::*;

use crate::mapping;

macro_rules! unavailable_repo {
    ($($trait_name:ident { $(async fn $method:ident(&mut self $(, $arg:ident: $ty:ty)* $(,)?) -> errors::Result<$out:ty>;)* })*) => {
        $(
            #[async_trait::async_trait]
            impl $trait_name for UnavailableRepo {
                $(
                    async fn $method(&mut self $(, $arg: $ty)*) -> errors::Result<$out> {
                        $(let _ = $arg;)*
                        Err(mapping::unavailable(stringify!($trait_name)))
                    }
                )*
            }
        )*
    };
}

unavailable_repo! {
    EffectRead {
        async fn get(&mut self, id: EffectId) -> errors::Result<Option<EffectRow>>;
        async fn list_by_run(&mut self, run: RunId) -> errors::Result<Vec<EffectRow>>;
    }
    EffectRepo {
        async fn insert(&mut self, effect: NewEffect) -> errors::Result<()>;
        async fn cas_transition(
            &mut self,
            id: EffectId,
            expect_state: EffectState,
            expect_token: Option<u64>,
            patch: EffectPatch,
        ) -> errors::Result<bool>;
    }
    ResourceRead {
        async fn get(&mut self, id: ReservationId) -> errors::Result<Option<ReservationRow>>;
        async fn list_by_run(&mut self, run: RunId) -> errors::Result<Vec<ReservationRow>>;
    }
    ResourceRepo {
        async fn insert(&mut self, reservation: NewReservation) -> errors::Result<()>;
        async fn cas_transition(
            &mut self,
            id: ReservationId,
            expect_state: ReservationState,
            patch: ReservationPatch,
        ) -> errors::Result<bool>;
    }
    TimerRead {
        async fn get(&mut self, id: TimerId) -> errors::Result<Option<TimerRow>>;
        async fn list_due(&mut self, due_before_ms: i64) -> errors::Result<Vec<TimerRow>>;
    }
    TimerRepo {
        async fn insert(&mut self, timer: NewTimer) -> errors::Result<()>;
        async fn cas_transition(
            &mut self,
            id: TimerId,
            expect_state: TimerState,
            expect_version: u64,
            patch: TimerPatch,
        ) -> errors::Result<bool>;
    }
    SecurityRead {
        async fn get_grant(
            &mut self,
            id: CapabilityGrantId,
        ) -> errors::Result<Option<CapabilityGrantRow>>;
        async fn list_delegation_hops(
            &mut self,
            chain: DelegationChainId,
        ) -> errors::Result<Vec<DelegationHopRow>>;
        async fn get_approval_request(
            &mut self,
            id: ApprovalRequestId,
        ) -> errors::Result<Option<ApprovalRequestRow>>;
        async fn list_approval_responses(
            &mut self,
            request: ApprovalRequestId,
        ) -> errors::Result<Vec<ApprovalResponseRow>>;
    }
    SecurityRepo {
        async fn insert_grant(&mut self, grant: NewCapabilityGrant) -> errors::Result<()>;
        async fn insert_delegation_hop(&mut self, hop: NewDelegationHop) -> errors::Result<()>;
        async fn insert_approval_request(
            &mut self,
            request: NewApprovalRequest,
        ) -> errors::Result<()>;
        async fn insert_approval_response(
            &mut self,
            response: NewApprovalResponse,
        ) -> errors::Result<()>;
    }
    ConfigRead {
        async fn get_generation(
            &mut self,
            id: ConfigGenerationId,
        ) -> errors::Result<Option<ConfigGenerationRow>>;
        async fn get_active(&mut self) -> errors::Result<Option<ActiveConfigGenerationRow>>;
    }
    ConfigRepo {
        async fn insert_generation(
            &mut self,
            generation: NewConfigGeneration,
        ) -> errors::Result<()>;
        async fn cas_active(
            &mut self,
            expected_revision: u64,
            generation: ConfigGenerationId,
            activated_at_ms: i64,
        ) -> errors::Result<bool>;
    }
    WorkspaceRead {
        async fn get_workspace(&mut self, id: WorkspaceId)
            -> errors::Result<Option<WorkspaceRow>>;
        async fn get_lease(&mut self, id: LeaseId) -> errors::Result<Option<WorkspaceLeaseRow>>;
    }
    WorkspaceRepo {
        async fn insert_workspace(&mut self, workspace: NewWorkspace) -> errors::Result<()>;
        async fn insert_lease(&mut self, lease: NewWorkspaceLease) -> errors::Result<()>;
        async fn cas_lease(
            &mut self,
            id: LeaseId,
            expect_epoch: u64,
            patch: LeasePatch,
        ) -> errors::Result<bool>;
    }
    AdapterRead {
        async fn get_registration(
            &mut self,
            adapter_id: AdapterId,
            version: &str,
            bundle_digest: &str,
        ) -> errors::Result<Option<AdapterRegistrationRow>>;
        async fn get_conformance_report(
            &mut self,
            adapter_id: AdapterId,
            version: &str,
            bundle_digest: &str,
        ) -> errors::Result<Option<ConformanceReportRow>>;
    }
    AdapterRepo {
        async fn insert_registration(
            &mut self,
            registration: NewAdapterRegistration,
        ) -> errors::Result<()>;
        async fn insert_instance(&mut self, instance: NewAdapterInstance) -> errors::Result<()>;
        async fn cas_instance_state(
            &mut self,
            id: AdapterInstanceId,
            expect_state: &str,
            patch: AdapterInstanceStatePatch,
        ) -> errors::Result<bool>;
        async fn insert_conformance_report(
            &mut self,
            report: NewConformanceReport,
        ) -> errors::Result<()>;
    }
    ArtifactRead {
        async fn get_by_id(&mut self, id: ArtifactId) -> errors::Result<Option<ArtifactRow>>;
        async fn get_by_uri(&mut self, uri: &str) -> errors::Result<Option<ArtifactRow>>;
    }
    ArtifactRepo {
        async fn insert(&mut self, artifact: NewArtifact) -> errors::Result<()>;
    }
    LoopRead {
        async fn get_turn(&mut self, id: TurnId) -> errors::Result<Option<LoopTurnRow>>;
        async fn get_decision(
            &mut self,
            run: RunId,
            decision: DecisionId,
        ) -> errors::Result<Option<DecisionRow>>;
    }
    LoopRepo {
        async fn insert_turn(&mut self, turn: NewLoopTurn) -> errors::Result<()>;
        async fn cas_turn(
            &mut self,
            id: TurnId,
            expect_state: &str,
            patch: LoopTurnPatch,
        ) -> errors::Result<bool>;
        async fn insert_decision(&mut self, decision: NewDecision) -> errors::Result<()>;
    }
    IdempotencyRepo {
        async fn lookup(
            &mut self,
            principal: PrincipalId,
            key: &IdempotencyKey,
        ) -> errors::Result<Option<IdempotencyRecordRow>>;
        async fn insert(&mut self, record: NewIdempotencyRecord) -> errors::Result<()>;
    }
    StreamRepo {
        async fn allocate(&mut self, stream_key: EventStreamKey) -> errors::Result<u64>;
        async fn insert_outbox(&mut self, event: NewOutboxEvent) -> errors::Result<()>;
        async fn scan_unpublished(&mut self, limit: u32)
            -> errors::Result<Vec<OutboxEventRow>>;
        async fn mark_published(
            &mut self,
            event_id: EventId,
            kind: PublishKind,
        ) -> errors::Result<()>;
    }
}
