//! Principals, actors, capability types, and delegation chains.
//!
//! Authority travels as explicit capability grants: each hop stores the exact
//! grant identifiers it holds, and its capability set is reconstructed from
//! those grants on load, never inferred from ancestry (R1.1, R1.5). A child
//! chain can only narrow authority, so every hop's capability set must be a
//! subset of its parent's (R1.2, R1.3, P1).

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use domain::ids::{ActorId, CapabilityGrantId, DelegationChainId, InvalidId, RunId};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use kernel_store::KernelTxn;
use kernel_store::models::NewDelegationHop;

/// Capability family from `contracts/capabilities/security-capabilities.yaml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CapabilityFamily {
    /// Workspace and filesystem authority.
    Workspace,
    /// Network egress authority.
    Network,
    /// Secret access authority.
    Secret,
    /// Agent spawning authority.
    Agent,
    /// Extension lifecycle authority.
    Extension,
    /// Configuration lifecycle authority.
    Config,
    /// Effect reconciliation authority.
    Effect,
    /// Resource reservation authority.
    Resource,
}

impl CapabilityFamily {
    /// Returns the canonical contract token.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Network => "network",
            Self::Secret => "secret",
            Self::Agent => "agent",
            Self::Extension => "extension",
            Self::Config => "config",
            Self::Effect => "effect",
            Self::Resource => "resource",
        }
    }
}

impl fmt::Display for CapabilityFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CapabilityFamily {
    type Err = InvalidId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "workspace" => Ok(Self::Workspace),
            "network" => Ok(Self::Network),
            "secret" => Ok(Self::Secret),
            "agent" => Ok(Self::Agent),
            "extension" => Ok(Self::Extension),
            "config" => Ok(Self::Config),
            "effect" => Ok(Self::Effect),
            "resource" => Ok(Self::Resource),
            _ => Err(InvalidId),
        }
    }
}

/// Capability action from `contracts/capabilities/security-capabilities.yaml`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CapabilityAction {
    /// Read workspace content.
    Read,
    /// Write workspace content.
    Write,
    /// Fork a workspace.
    Fork,
    /// Merge a workspace.
    Merge,
    /// Transfer the exclusive-write capability.
    TransferExclusive,
    /// Write under shared coordination.
    SharedCoordinated,
    /// Connect to a network target.
    Connect,
    /// Use a secret.
    Use,
    /// Sign or act with a secret.
    SignOrAct,
    /// Spawn an agent.
    Spawn,
    /// Install an extension.
    Install,
    /// Enable an extension.
    Enable,
    /// Disable an extension.
    Disable,
    /// Propose a configuration.
    Propose,
    /// Test a configuration.
    Test,
    /// Activate a configuration.
    Activate,
    /// Roll back a configuration.
    Rollback,
    /// Reconcile an effect.
    Reconcile,
    /// Resolve an unknown effect outcome.
    ResolveUnknown,
    /// Reserve a resource.
    Reserve,
}

impl CapabilityAction {
    /// Returns the canonical contract token.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Fork => "fork",
            Self::Merge => "merge",
            Self::TransferExclusive => "transfer_exclusive_write",
            Self::SharedCoordinated => "shared_coordinated_write",
            Self::Connect => "connect",
            Self::Use => "use",
            Self::SignOrAct => "sign_or_act",
            Self::Spawn => "spawn",
            Self::Install => "install",
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Propose => "propose",
            Self::Test => "test",
            Self::Activate => "activate",
            Self::Rollback => "rollback",
            Self::Reconcile => "reconcile",
            Self::ResolveUnknown => "resolve_unknown",
            Self::Reserve => "reserve",
        }
    }
}

impl fmt::Display for CapabilityAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for CapabilityAction {
    type Err = InvalidId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "read" => Ok(Self::Read),
            "write" => Ok(Self::Write),
            "fork" => Ok(Self::Fork),
            "merge" => Ok(Self::Merge),
            "transfer_exclusive_write" => Ok(Self::TransferExclusive),
            "shared_coordinated_write" => Ok(Self::SharedCoordinated),
            "connect" => Ok(Self::Connect),
            "use" => Ok(Self::Use),
            "sign_or_act" => Ok(Self::SignOrAct),
            "spawn" => Ok(Self::Spawn),
            "install" => Ok(Self::Install),
            "enable" => Ok(Self::Enable),
            "disable" => Ok(Self::Disable),
            "propose" => Ok(Self::Propose),
            "test" => Ok(Self::Test),
            "activate" => Ok(Self::Activate),
            "rollback" => Ok(Self::Rollback),
            "reconcile" => Ok(Self::Reconcile),
            "resolve_unknown" => Ok(Self::ResolveUnknown),
            "reserve" => Ok(Self::Reserve),
            _ => Err(InvalidId),
        }
    }
}

/// A typed capability: one family/action pair from the capability contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Capability {
    /// Capability family.
    pub family: CapabilityFamily,
    /// Action within the family.
    pub action: CapabilityAction,
}

impl Capability {
    /// Builds a capability from its family and action.
    pub const fn new(family: CapabilityFamily, action: CapabilityAction) -> Self {
        Self { family, action }
    }

    /// Returns whether the pair is defined by the capability contract.
    pub const fn is_supported(&self) -> bool {
        match self.family {
            CapabilityFamily::Workspace => matches!(
                self.action,
                CapabilityAction::Read
                    | CapabilityAction::Write
                    | CapabilityAction::Fork
                    | CapabilityAction::Merge
                    | CapabilityAction::TransferExclusive
                    | CapabilityAction::SharedCoordinated
            ),
            CapabilityFamily::Network => matches!(self.action, CapabilityAction::Connect),
            CapabilityFamily::Secret => {
                matches!(
                    self.action,
                    CapabilityAction::Use | CapabilityAction::SignOrAct
                )
            }
            CapabilityFamily::Agent => matches!(self.action, CapabilityAction::Spawn),
            CapabilityFamily::Extension => matches!(
                self.action,
                CapabilityAction::Install | CapabilityAction::Enable | CapabilityAction::Disable
            ),
            CapabilityFamily::Config => matches!(
                self.action,
                CapabilityAction::Propose
                    | CapabilityAction::Test
                    | CapabilityAction::Activate
                    | CapabilityAction::Rollback
            ),
            CapabilityFamily::Effect => matches!(
                self.action,
                CapabilityAction::Reconcile | CapabilityAction::ResolveUnknown
            ),
            CapabilityFamily::Resource => matches!(self.action, CapabilityAction::Reserve),
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.family, self.action)
    }
}

impl FromStr for Capability {
    type Err = InvalidId;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (family, action) = text.split_once('.').ok_or(InvalidId)?;
        let capability = Self {
            family: family.parse()?,
            action: action.parse()?,
        };
        if capability.is_supported() {
            Ok(capability)
        } else {
            Err(InvalidId)
        }
    }
}

/// One delegation hop: an actor/run context holding explicit grants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hop {
    /// Zero-based position of the hop in its chain.
    pub hop_index: u32,
    /// Actor that holds this hop's authority.
    pub actor_id: ActorId,
    /// Run context when the hop is scoped to a run.
    pub run_id: Option<RunId>,
    /// Exact grant identifiers backing this hop.
    pub grant_ids: Vec<CapabilityGrantId>,
    /// Capabilities derived from the referenced grants.
    pub capabilities: Vec<Capability>,
}

/// A linear authority lineage from a root holder to its delegates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DelegationChain {
    /// Identifier shared by every hop of the chain.
    pub chain_id: DelegationChainId,
    /// Hops ordered from root (`0`) to tip.
    pub hops: Vec<Hop>,
}

/// Width of one canonical hyphenated UUIDv7 in the persisted grant-id blob.
const GRANT_ID_WIDTH: usize = 36;

/// Persists one hop of `chain`, failing closed on any integrity violation.
///
/// The hop must extend the chain contiguously, every referenced grant must
/// exist, its declared capabilities must exactly match those grants, and for
/// non-root hops the capability set must be a subset of the parent's.
pub async fn persist_hop(
    txn: &mut dyn KernelTxn,
    chain: DelegationChainId,
    hop: Hop,
) -> errors::Result<()> {
    let capabilities = resolve_hop(&mut *txn, &hop).await?;
    let existing = load_chain(&mut *txn, chain).await?;
    let expected_index = u32::try_from(existing.hops.len())
        .map_err(|_| integrity("delegation chain exceeds the hop index range"))?;
    if hop.hop_index != expected_index {
        return Err(integrity(
            "delegation hop does not extend the chain in order",
        ));
    }
    if let Some(parent) = existing.hops.last() {
        let parent_capabilities: BTreeSet<Capability> =
            parent.capabilities.iter().copied().collect();
        if !capabilities.is_subset(&parent_capabilities) {
            return Err(integrity(
                "delegation hop capabilities are not a subset of its parent",
            ));
        }
    }
    txn.security()
        .insert_delegation_hop(NewDelegationHop {
            chain_id: chain,
            hop_index: hop.hop_index,
            principal_or_actor_id: hop.actor_id.to_string(),
            run_id: hop.run_id,
            capability_grant_ids: encode_grant_ids(&hop.grant_ids),
        })
        .await?;
    Ok(())
}

/// Loads and validates a chain, rejecting it as an integrity error when any
/// referenced grant is missing or the hop linkage is broken (R1.3, R1.4).
pub async fn load_chain(
    txn: &mut dyn KernelTxn,
    chain: DelegationChainId,
) -> errors::Result<DelegationChain> {
    let rows = txn.security().list_delegation_hops(chain).await?;
    let mut hops = Vec::with_capacity(rows.len());
    for row in rows {
        let grant_ids = decode_grant_ids(&row.capability_grant_ids)?;
        let mut capabilities = BTreeSet::new();
        for grant_id in &grant_ids {
            let grant = txn
                .security()
                .get_grant(*grant_id)
                .await?
                .ok_or_else(|| integrity("delegation hop references a missing grant"))?;
            capabilities.insert(parse_capability(&grant.capability_id)?);
        }
        let actor_id = ActorId::from_str(&row.principal_or_actor_id)
            .map_err(|_| integrity("delegation hop actor is not a valid identifier"))?;
        hops.push(Hop {
            hop_index: row.hop_index,
            actor_id,
            run_id: row.run_id,
            grant_ids,
            capabilities: capabilities.into_iter().collect(),
        });
    }
    let chain = DelegationChain {
        chain_id: chain,
        hops,
    };
    validate_chain(&chain)?;
    Ok(chain)
}

/// Returns the capability subset for a child hop, sorted and duplicate-free.
///
/// Fails when the requested set is not a subset of the parent's grant-backed
/// capabilities or when the parent chain itself is not valid (R1.2).
pub fn derive_child_chain(
    parent: &DelegationChain,
    requested: &[Capability],
) -> errors::Result<Vec<Capability>> {
    validate_chain(parent)?;
    let parent_capabilities: BTreeSet<Capability> = parent
        .hops
        .last()
        .map(|hop| hop.capabilities.iter().copied().collect())
        .unwrap_or_default();
    let mut derived = BTreeSet::new();
    for capability in requested {
        if !parent_capabilities.contains(capability) {
            return Err(invalid(
                "requested capability is not held by the parent chain",
            ));
        }
        derived.insert(*capability);
    }
    Ok(derived.into_iter().collect())
}

/// Validates chain integrity: contiguous hop linkage, grant-backed
/// capabilities, and the subset rule between each hop and its parent.
pub fn validate_chain(chain: &DelegationChain) -> errors::Result<()> {
    let mut previous: Option<BTreeSet<Capability>> = None;
    for (position, hop) in chain.hops.iter().enumerate() {
        if u32::try_from(position).ok() != Some(hop.hop_index) {
            return Err(integrity(
                "delegation hop index is not the next contiguous position",
            ));
        }
        let capabilities = hop_capabilities(hop)?;
        if let Some(parent) = &previous
            && !capabilities.is_subset(parent)
        {
            return Err(integrity(
                "delegation hop capabilities are not a subset of its parent",
            ));
        }
        previous = Some(capabilities);
    }
    Ok(())
}

/// Resolves a hop's grants, verifying existence and declared capabilities.
async fn resolve_hop(txn: &mut dyn KernelTxn, hop: &Hop) -> errors::Result<BTreeSet<Capability>> {
    let declared = hop_capabilities(hop)?;
    let mut derived = BTreeSet::new();
    for grant_id in &hop.grant_ids {
        let grant = txn
            .security()
            .get_grant(*grant_id)
            .await?
            .ok_or_else(|| integrity("delegation hop references a missing grant"))?;
        derived.insert(parse_capability(&grant.capability_id)?);
    }
    if derived != declared {
        return Err(integrity(
            "delegation hop capabilities do not match its grants",
        ));
    }
    Ok(derived)
}

/// Validates grant/capability shape within one hop and returns its capability
/// set, rejecting duplicates and claims that no stored grant can back.
fn hop_capabilities(hop: &Hop) -> errors::Result<BTreeSet<Capability>> {
    let grants: BTreeSet<CapabilityGrantId> = hop.grant_ids.iter().copied().collect();
    if grants.len() != hop.grant_ids.len() {
        return Err(integrity("delegation hop repeats a grant identifier"));
    }
    let capabilities: BTreeSet<Capability> = hop.capabilities.iter().copied().collect();
    if capabilities.len() != hop.capabilities.len() {
        return Err(integrity("delegation hop repeats a capability"));
    }
    if !capabilities.is_empty() && grants.is_empty() {
        return Err(integrity(
            "delegation hop claims capabilities without grant identifiers",
        ));
    }
    if capabilities.len() > grants.len() {
        return Err(integrity(
            "delegation hop claims more capabilities than its grants",
        ));
    }
    Ok(capabilities)
}

/// Parses one persisted grant's capability id.
fn parse_capability(capability_id: &str) -> errors::Result<Capability> {
    Capability::from_str(capability_id)
        .map_err(|_| integrity("delegation hop grant carries an unrecognized capability"))
}

/// Encodes grant identifiers as their concatenated canonical 36-byte forms.
fn encode_grant_ids(grant_ids: &[CapabilityGrantId]) -> Vec<u8> {
    let mut blob = Vec::with_capacity(grant_ids.len() * GRANT_ID_WIDTH);
    for grant_id in grant_ids {
        blob.extend_from_slice(grant_id.to_string().as_bytes());
    }
    blob
}

/// Decodes the concatenated canonical grant identifiers.
fn decode_grant_ids(blob: &[u8]) -> errors::Result<Vec<CapabilityGrantId>> {
    if !blob.len().is_multiple_of(GRANT_ID_WIDTH) {
        return Err(integrity(
            "delegation hop grant list is not a sequence of identifiers",
        ));
    }
    let mut grant_ids = Vec::with_capacity(blob.len() / GRANT_ID_WIDTH);
    for chunk in blob.chunks_exact(GRANT_ID_WIDTH) {
        let text = std::str::from_utf8(chunk)
            .map_err(|_| integrity("delegation hop grant list is not canonical text"))?;
        grant_ids.push(
            CapabilityGrantId::from_str(text).map_err(|_| {
                integrity("delegation hop grant list contains an invalid identifier")
            })?,
        );
    }
    Ok(grant_ids)
}

/// Builds an integrity failure (R1.4: rejected and never used for authorization).
fn integrity(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::FailedPrecondition, RetryClass::Never, message)
}

/// Builds a caller-input failure.
fn invalid(message: &'static str) -> KernelError {
    KernelError::new(ErrorCode::InvalidArgument, RetryClass::Never, message)
}
