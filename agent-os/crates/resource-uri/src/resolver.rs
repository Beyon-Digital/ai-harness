//! Capability-checked resolution of logical resource URIs.
//!
//! `resolve` always consults the execution context's authority *before*
//! dispatching to a backend: the gate decides from the URI's
//! [`required_capability`](crate::ResourceUri::required_capability). The
//! result is a logical handle — no absolute filesystem paths, and a secret
//! reference that never carries the value or its physical location.
//!
//! [`required_capability`]: crate::ResourceUri::required_capability

use std::collections::BTreeSet;

use domain::generated::contract::ExecutionContext;
use domain::ids::{ArtifactId, RunId, SandboxId, SessionId, TaskId, WorkspaceId};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};

use crate::ResourceUri;

/// Backend-native handle produced by a permitted resolution. These are
/// logical descriptors — binding them to absolute paths is the workspace /
/// secret adapters' job, not this layer's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedResource {
    /// A path relative to a workspace root.
    WorkspacePath {
        /// Owning workspace.
        workspace_id: WorkspaceId,
        /// Normalized path relative to that root.
        relative_path: String,
    },
    /// A content-addressed artifact reference.
    ArtifactRef {
        /// Referenced artifact.
        artifact_id: ArtifactId,
    },
    /// Opaque secret reference: namespace + name, never a path or value.
    SecretRef {
        /// Secret namespace.
        namespace: String,
        /// Secret name.
        name: String,
    },
    /// A live sandbox handle reference.
    SandboxRef {
        /// Referenced sandbox.
        sandbox_id: SandboxId,
    },
    /// A run reference.
    RunRef {
        /// Referenced run.
        run_id: RunId,
    },
    /// A task reference.
    TaskRef {
        /// Referenced task.
        task_id: TaskId,
    },
    /// A session reference.
    SessionRef {
        /// Referenced session.
        session_id: SessionId,
    },
    /// A pinned adapter binding.
    AdapterRef {
        /// Adapter identifier.
        adapter_id: domain::ids::AdapterId,
        /// Pinned version.
        version: String,
        /// Content digest.
        digest: String,
    },
}

/// Authority check run before backend dispatch.
pub trait CapabilityGate: Send + Sync {
    /// Approves `required` under `ctx`, or fails with `FailedPrecondition`.
    fn check(&self, ctx: &ExecutionContext, required: &str) -> errors::Result<()>;
}

/// Gate backed by an explicit set of capability strings carried on the
/// execution context. The MVP carries grants as ids; adapters embed their
/// effective capability snapshot in `options_json`-style freeform fields —
/// for the MVP resolver the gate is the injectable seam: production wires
/// it to the permission engine, tests use [`PermitSetGate`].
pub struct PermitSetGate {
    allowed: BTreeSet<String>,
}

impl PermitSetGate {
    /// Gate that approves exactly `allowed` capabilities.
    pub fn new(allowed: impl IntoIterator<Item = String>) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
        }
    }
}

impl CapabilityGate for PermitSetGate {
    fn check(&self, _ctx: &ExecutionContext, required: &str) -> errors::Result<()> {
        if self.allowed.contains(required) {
            Ok(())
        } else {
            Err(denied(required))
        }
    }
}

/// A gate that denies every resolution.
pub struct DenyAllGate;

impl CapabilityGate for DenyAllGate {
    fn check(&self, _ctx: &ExecutionContext, required: &str) -> errors::Result<()> {
        Err(denied(required))
    }
}

/// Scheme-dispatching resolver: permission first, then logical handle.
pub struct ResourceResolver {
    gate: Box<dyn CapabilityGate>,
}

impl ResourceResolver {
    /// Creates a resolver gated by `gate`.
    pub fn new(gate: impl CapabilityGate + 'static) -> Self {
        Self {
            gate: Box::new(gate),
        }
    }

    /// Resolves `uri` under `ctx`: permission check before dispatch.
    pub fn resolve(
        &self,
        ctx: &ExecutionContext,
        uri: &ResourceUri,
    ) -> errors::Result<ResolvedResource> {
        self.gate.check(ctx, uri.required_capability())?;
        Ok(match uri {
            ResourceUri::Workspace {
                workspace_id,
                relative_path,
            } => ResolvedResource::WorkspacePath {
                workspace_id: *workspace_id,
                relative_path: relative_path.clone(),
            },
            ResourceUri::Artifact(artifact_id) => ResolvedResource::ArtifactRef {
                artifact_id: *artifact_id,
            },
            ResourceUri::Secret { namespace, name } => ResolvedResource::SecretRef {
                namespace: namespace.clone(),
                name: name.clone(),
            },
            ResourceUri::Sandbox(sandbox_id) => ResolvedResource::SandboxRef {
                sandbox_id: *sandbox_id,
            },
            ResourceUri::Run(run_id) => ResolvedResource::RunRef { run_id: *run_id },
            ResourceUri::Task(task_id) => ResolvedResource::TaskRef { task_id: *task_id },
            ResourceUri::Session(session_id) => ResolvedResource::SessionRef {
                session_id: *session_id,
            },
            ResourceUri::Adapter {
                adapter_id,
                version,
                digest,
            } => ResolvedResource::AdapterRef {
                adapter_id: *adapter_id,
                version: version.clone(),
                digest: digest.clone(),
            },
        })
    }
}

fn denied(required: &str) -> KernelError {
    KernelError::new(
        ErrorCode::FailedPrecondition,
        RetryClass::Never,
        format!("missing capability {required:?} for resource resolution"),
    )
}
