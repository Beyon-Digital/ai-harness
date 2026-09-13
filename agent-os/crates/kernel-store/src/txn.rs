//! Store and transaction traits.

use async_trait::async_trait;
use domain::ids::DaemonInstanceId;

use crate::repositories::{
    AdapterRepo, AgentSpecRepo, ArtifactRepo, ConfigRepo, EffectRepo, EnvironmentRepo, GraphRepo,
    IdempotencyRepo, LoopRepo, ResourceRepo, RunRepo, SecurityRepo, SessionRepo, StreamRepo,
    TaskRepo, TimerRepo, WorkspaceRepo,
};
use crate::types::{DaemonFence, TxContext};

/// Opens transactions and owns the daemon fence.
#[async_trait]
pub trait KernelStore: Send + Sync {
    async fn begin_write(&self, ctx: TxContext) -> errors::Result<Box<dyn KernelTxn + '_>>;
    async fn begin_read(&self) -> errors::Result<Box<dyn KernelReadTxn + '_>>;
    async fn acquire_daemon_fence(&self, instance: DaemonInstanceId)
    -> errors::Result<DaemonFence>;
    async fn current_fence(&self) -> errors::Result<Option<DaemonFence>>;
}

/// Write transaction exposing typed repositories and no SQL (R2.1, R2.5).
#[async_trait]
pub trait KernelTxn: Send + Sync {
    fn context(&self) -> &TxContext;
    fn runs(&mut self) -> &mut dyn RunRepo;
    fn tasks(&mut self) -> &mut dyn TaskRepo;
    fn sessions(&mut self) -> &mut dyn SessionRepo;
    fn agent_specs(&mut self) -> &mut dyn AgentSpecRepo;
    fn graph(&mut self) -> &mut dyn GraphRepo;
    fn environments(&mut self) -> &mut dyn EnvironmentRepo;
    fn effects(&mut self) -> &mut dyn EffectRepo;
    fn resources(&mut self) -> &mut dyn ResourceRepo;
    fn timers(&mut self) -> &mut dyn TimerRepo;
    fn security(&mut self) -> &mut dyn SecurityRepo;
    fn config(&mut self) -> &mut dyn ConfigRepo;
    fn workspaces(&mut self) -> &mut dyn WorkspaceRepo;
    fn adapters(&mut self) -> &mut dyn AdapterRepo;
    fn artifacts(&mut self) -> &mut dyn ArtifactRepo;
    fn loop_turns(&mut self) -> &mut dyn LoopRepo;
    fn idempotency(&mut self) -> &mut dyn IdempotencyRepo;
    fn streams(&mut self) -> &mut dyn StreamRepo;
    async fn commit(self: Box<Self>) -> errors::Result<()>;
    async fn rollback(self: Box<Self>) -> errors::Result<()>;
}

/// Read transaction exposing read repositories only (R2.2, compile-time enforced).
#[async_trait]
pub trait KernelReadTxn: Send + Sync {
    fn runs(&mut self) -> &mut dyn crate::repositories::RunRead;
    fn tasks(&mut self) -> &mut dyn crate::repositories::TaskRead;
    fn sessions(&mut self) -> &mut dyn crate::repositories::SessionRead;
    fn agent_specs(&mut self) -> &mut dyn crate::repositories::AgentSpecRead;
    fn graph(&mut self) -> &mut dyn crate::repositories::GraphRead;
    fn environments(&mut self) -> &mut dyn crate::repositories::EnvironmentRead;
    fn effects(&mut self) -> &mut dyn crate::repositories::EffectRead;
    fn resources(&mut self) -> &mut dyn crate::repositories::ResourceRead;
    fn timers(&mut self) -> &mut dyn crate::repositories::TimerRead;
    fn security(&mut self) -> &mut dyn crate::repositories::SecurityRead;
    fn config(&mut self) -> &mut dyn crate::repositories::ConfigRead;
    fn workspaces(&mut self) -> &mut dyn crate::repositories::WorkspaceRead;
    fn adapters(&mut self) -> &mut dyn crate::repositories::AdapterRead;
    fn artifacts(&mut self) -> &mut dyn crate::repositories::ArtifactRead;
    fn loop_turns(&mut self) -> &mut dyn crate::repositories::LoopRead;
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use domain::ids::{CommandId, PrincipalId};

    use crate::{KernelTxn, TxContext};

    struct StubTxn {
        ctx: TxContext,
    }

    #[async_trait::async_trait]
    impl KernelTxn for StubTxn {
        fn context(&self) -> &TxContext {
            &self.ctx
        }

        fn runs(&mut self) -> &mut dyn crate::RunRepo {
            panic!("object-safety stub")
        }

        fn tasks(&mut self) -> &mut dyn crate::TaskRepo {
            panic!("object-safety stub")
        }

        fn sessions(&mut self) -> &mut dyn crate::SessionRepo {
            panic!("object-safety stub")
        }

        fn agent_specs(&mut self) -> &mut dyn crate::AgentSpecRepo {
            panic!("object-safety stub")
        }

        fn graph(&mut self) -> &mut dyn crate::GraphRepo {
            panic!("object-safety stub")
        }

        fn environments(&mut self) -> &mut dyn crate::EnvironmentRepo {
            panic!("object-safety stub")
        }

        fn effects(&mut self) -> &mut dyn crate::EffectRepo {
            panic!("object-safety stub")
        }

        fn resources(&mut self) -> &mut dyn crate::ResourceRepo {
            panic!("object-safety stub")
        }

        fn timers(&mut self) -> &mut dyn crate::TimerRepo {
            panic!("object-safety stub")
        }

        fn security(&mut self) -> &mut dyn crate::SecurityRepo {
            panic!("object-safety stub")
        }

        fn config(&mut self) -> &mut dyn crate::ConfigRepo {
            panic!("object-safety stub")
        }

        fn workspaces(&mut self) -> &mut dyn crate::WorkspaceRepo {
            panic!("object-safety stub")
        }

        fn adapters(&mut self) -> &mut dyn crate::AdapterRepo {
            panic!("object-safety stub")
        }

        fn artifacts(&mut self) -> &mut dyn crate::ArtifactRepo {
            panic!("object-safety stub")
        }

        fn loop_turns(&mut self) -> &mut dyn crate::LoopRepo {
            panic!("object-safety stub")
        }

        fn idempotency(&mut self) -> &mut dyn crate::IdempotencyRepo {
            panic!("object-safety stub")
        }

        fn streams(&mut self) -> &mut dyn crate::StreamRepo {
            panic!("object-safety stub")
        }

        async fn commit(self: Box<Self>) -> errors::Result<()> {
            Ok(())
        }

        async fn rollback(self: Box<Self>) -> errors::Result<()> {
            Ok(())
        }
    }

    const PRINCIPAL: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e70";
    const COMMAND: &str = "018f2b9c-4a1e-7c3d-9f00-2b7a1c5d6e71";

    #[test]
    fn boxed_kernel_txn_performs_a_no_op_call() {
        let principal = match PrincipalId::from_str(PRINCIPAL) {
            Ok(value) => value,
            Err(error) => panic!("sample principal rejected: {error}"),
        };
        let command = match CommandId::from_str(COMMAND) {
            Ok(value) => value,
            Err(error) => panic!("sample command rejected: {error}"),
        };
        let txn: Box<dyn KernelTxn> = Box::new(StubTxn {
            ctx: TxContext {
                daemon_epoch: 7,
                principal_id: principal,
                command_id: command,
                correlation_id: None,
            },
        });
        assert_eq!(txn.context().daemon_epoch, 7);
        assert_eq!(txn.context().principal_id, principal);
        assert_eq!(txn.context().command_id, command);
    }
}
