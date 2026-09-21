//! Request/response dispatch with deadlines and cancellation.
//!
//! `dispatch_call` writes a `PortCallRequest`, then reads inbound frames
//! until the matching `PortCallResponse` arrives. Frame reads honor the
//! request's deadline via stream read timeout; on expiry a `CancelCall`
//! frame is sent and the call fails `DeadlineExceeded`. Unrelated frames
//! (ping, pong, stray responses) are consumed and ignored.
#![forbid(unsafe_code)]

use std::io;
use std::os::unix::net::UnixStream;
use std::time::Instant;

use domain::generated::contract::{
    AdapterFrame, AdapterPing, AdapterPong, CancelCall, PortCallRequest, PortCallResponse,
    adapter_frame::Body,
};
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};

use crate::framing::{read_frame, write_frame};
pub use crate::handshake::SessionPhase;
use crate::handshake::accept_inbound;

/// Failure modes of `dispatch_call` that are distinct from `KernelError`:
/// a call that was cancelled delivers a `Cancelled` marker so the caller
/// can distinguish adapter-initiated cancellation from expiry.
#[derive(Debug)]
pub enum CallError {
    /// Kernel-level failure.
    Kernel(KernelError),
    /// The adapter reported `CancelledCall` for this call id.
    Cancelled(String),
}

impl From<KernelError> for CallError {
    fn from(error: KernelError) -> Self {
        CallError::Kernel(error)
    }
}

/// Sends `request` over `stream` and waits for the matching
/// `PortCallResponse` or the deadline. `deadline` is an absolute
/// `Instant`; `phase` is threaded through `accept_inbound` so order
/// violations fail the call.
pub fn dispatch_call(
    stream: &mut UnixStream,
    phase: &mut SessionPhase,
    request: PortCallRequest,
    deadline: Instant,
) -> Result<PortCallResponse, CallError> {
    let call_id = request.call_id.clone();
    write_frame(
        stream,
        &AdapterFrame {
            body: Some(Body::Request(request)),
        },
    )
    .map_err(CallError::Kernel)?;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| CallError::Kernel(deadline_exceeded(&call_id)))?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|e| CallError::Kernel(io_err("set read timeout", e)))?;
        let frame = match read_frame(stream) {
            Ok(Some(frame)) => frame,
            Ok(None) => {
                return Err(CallError::Kernel(KernelError::new(
                    ErrorCode::Unavailable,
                    RetryClass::Never,
                    "adapter stream closed mid-call",
                )));
            }
            Err(e) => {
                if let Some(src) = std::error::Error::source(&e)
                    && let Some(io_e) = src.downcast_ref::<io::Error>()
                    && io_e.kind() == io::ErrorKind::WouldBlock
                {
                    send_cancel(stream, &call_id);
                    return Err(CallError::Kernel(deadline_exceeded(&call_id)));
                }
                return Err(CallError::Kernel(e));
            }
        };
        *phase = accept_inbound(*phase, &frame).map_err(CallError::Kernel)?;
        match frame.body {
            Some(Body::Response(resp)) if resp.call_id == call_id => return Ok(resp),
            Some(Body::Ping(AdapterPing { nonce })) => {
                let _ = write_frame(
                    stream,
                    &AdapterFrame {
                        body: Some(Body::Pong(AdapterPong { nonce })),
                    },
                );
            }
            Some(Body::Cancel(CancelCall { call_id: cid })) if cid == call_id => {
                return Err(CallError::Cancelled(cid));
            }
            _ => {}
        }
    }
}

/// Sends a `CancelCall` frame; delivery failure is ignored — the caller
/// is already erroring.
pub fn send_cancel(stream: &mut UnixStream, call_id: &str) {
    let _ = write_frame(
        stream,
        &AdapterFrame {
            body: Some(Body::Cancel(CancelCall {
                call_id: call_id.to_owned(),
            })),
        },
    );
}

/// Sends `AdapterPing` and awaits the matching `AdapterPong` by `deadline`.
pub fn ping(stream: &mut UnixStream, nonce: &str, deadline: Instant) -> errors::Result<()> {
    write_frame(
        stream,
        &AdapterFrame {
            body: Some(Body::Ping(AdapterPing {
                nonce: nonce.to_owned(),
            })),
        },
    )?;
    let phase = SessionPhase::Ready;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| deadline_exceeded(nonce))?;
        stream
            .set_read_timeout(Some(remaining))
            .map_err(|e| io_err("set read timeout", e))?;
        match read_frame(stream)? {
            Some(AdapterFrame {
                body: Some(Body::Pong(AdapterPong { nonce: got })),
            }) if got == nonce => return Ok(()),
            Some(frame) => {
                accept_inbound(phase, &frame)?;
            }
            None => {
                return Err(KernelError::new(
                    ErrorCode::Unavailable,
                    RetryClass::Never,
                    "adapter stream closed during ping",
                ));
            }
        }
    }
}

fn deadline_exceeded(call_id: &str) -> KernelError {
    KernelError::new(
        ErrorCode::Unavailable,
        RetryClass::Never,
        format!("adapter call {call_id} exceeded its deadline"),
    )
}

fn io_err(op: &str, source: io::Error) -> KernelError {
    KernelError::new(
        ErrorCode::Unavailable,
        RetryClass::Safe,
        format!("{op} failed"),
    )
    .with_source(source)
}
