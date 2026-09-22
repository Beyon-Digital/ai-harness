//! Framed protobuf protocol for external adapter processes.
#![forbid(unsafe_code)]

pub mod framing;
pub mod handshake;
pub mod session;

pub use framing::{MAX_FRAME_BYTES, encode_frame, read_frame, write_frame};
pub use handshake::{ExpectedIdentity, SessionPhase, accept_inbound};
pub use session::{CallError, dispatch_call};
