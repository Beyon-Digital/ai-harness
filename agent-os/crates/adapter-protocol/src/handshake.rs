//! Bootstrap/hello identity validation.
//!
//! Order is enforced: `AdapterBootstrap` first (kernel -> child), then
//! `AdapterHello` (child -> kernel) pinned to the supervisor's expected
//! adapter identity — adapter self-assertion in `Hello` can never widen the
//! registered `(adapter_id, version, bundle_digest)` identity. Any other
//! frame arriving before a valid Hello is an unexpected-order error.
#![forbid(unsafe_code)]

use domain::generated::contract::{
    AdapterBootstrap, AdapterFrame, AdapterHello, adapter_frame::Body,
};
use domain::ids::DaemonInstanceId;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};

/// The identity the supervisor expects the child to echo.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpectedIdentity {
    /// Owning daemon instance.
    pub daemon_instance_id: DaemonInstanceId,
    /// Daemon fencing epoch for this spawn.
    pub daemon_fencing_epoch: u64,
    /// Fresh adapter-instance id minted for this spawn.
    pub adapter_instance_id: String,
    /// Registered adapter id.
    pub adapter_id: String,
    /// Registered adapter version.
    pub adapter_version: String,
    /// Verified bundle digest (registry-attested).
    pub expected_bundle_digest: String,
    /// Protocol version the kernel offers.
    pub protocol_version: u32,
}

impl ExpectedIdentity {
    /// Builds the `AdapterBootstrap` frame for `bootstrap_nonce`.
    pub fn bootstrap_frame(&self, bootstrap_nonce: &str) -> AdapterFrame {
        AdapterFrame {
            body: Some(Body::Bootstrap(AdapterBootstrap {
                daemon_instance_id: self.daemon_instance_id.to_string(),
                daemon_fencing_epoch: self.daemon_fencing_epoch,
                adapter_instance_id: self.adapter_instance_id.clone(),
                expected_bundle_digest: self.expected_bundle_digest.clone(),
                bootstrap_nonce: bootstrap_nonce.to_owned(),
                protocol_version: self.protocol_version,
            })),
        }
    }

    /// Verifies a received `AdapterHello` against the expected identity.
    /// The `bootstrap_nonce` is supplied for the handshake transcript; the
    /// contract has no echo field on `Hello`, so freshness is proven by the
    /// private channel carrying the nonce (only the child that read the
    /// bootstrap can respond on it).
    pub fn verify_hello(&self, hello: &AdapterHello, _bootstrap_nonce: &str) -> errors::Result<()> {
        let check = |field: &str, got: &str, want: &str| {
            if got == want {
                Ok(())
            } else {
                Err(failed(format!("adapter hello {field} mismatch")))
            }
        };
        check(
            "adapter_instance_id",
            &hello.adapter_instance_id,
            &self.adapter_instance_id,
        )?;
        check("adapter_id", &hello.adapter_id, &self.adapter_id)?;
        check(
            "adapter_version",
            &hello.adapter_version,
            &self.adapter_version,
        )?;
        check(
            "bundle_digest",
            &hello.bundle_digest,
            &self.expected_bundle_digest,
        )?;
        if hello.protocol_version != self.protocol_version {
            return Err(failed(format!(
                "protocol version mismatch: adapter offers {}, kernel expects {}",
                hello.protocol_version, self.protocol_version
            )));
        }
        Ok(())
    }
}

/// Frame-ordering phase of an adapter session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPhase {
    /// Waiting for the child's `AdapterHello`.
    AwaitingHello,
    /// Handshake verified; request/response/cancel/ping frames flow.
    Ready,
    /// `AdapterShutdown` sent; session is draining.
    Draining,
}

/// Which side a frame arrived from — kernel frames are never legal inbound.
#[derive(Clone, Copy, Debug)]
pub enum Direction {
    /// Frame read from the adapter child.
    FromChild,
    /// Frame read from the kernel side (only used in fixture tests).
    FromKernel,
}

/// Validates an inbound frame against the session phase; returns the new
/// phase. Kernel->child frames (`Bootstrap`, `Shutdown`) arriving
/// inbound are unexpected-order errors.
pub fn accept_inbound(phase: SessionPhase, frame: &AdapterFrame) -> errors::Result<SessionPhase> {
    let body = frame.body.as_ref().ok_or_else(|| {
        KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            "adapter frame has no body",
        )
    })?;
    match (phase, body) {
        (SessionPhase::AwaitingHello, Body::Hello(_)) => Ok(SessionPhase::Ready),
        (SessionPhase::AwaitingHello, _) => Err(failed(
            "expected AdapterHello before any other frame".to_owned(),
        )),
        (SessionPhase::Ready, Body::Response(_))
        | (SessionPhase::Ready, Body::Pong(_))
        | (SessionPhase::Ready, Body::Hello(_))
        | (SessionPhase::Ready, Body::Cancel(_)) => Ok(SessionPhase::Ready),
        (SessionPhase::Ready, Body::Bootstrap(_) | Body::Shutdown(_)) => {
            Err(failed("kernel frame received inbound".to_owned()))
        }
        (SessionPhase::Ready, Body::Ping(_)) => Ok(SessionPhase::Ready),
        (SessionPhase::Ready, Body::Request(_)) => Err(failed(
            "PortCallRequest is a kernel->adapter frame".to_owned(),
        )),
        (SessionPhase::Draining, _) => {
            Err(failed("frame received after AdapterShutdown".to_owned()))
        }
    }
}

fn failed(message: String) -> KernelError {
    KernelError::new(ErrorCode::FailedPrecondition, RetryClass::Never, message)
}
