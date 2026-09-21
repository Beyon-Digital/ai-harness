//! Length-delimited `AdapterFrame` codec over any blocking byte stream.
//!
//! Wire format: `u32` big-endian length + prost-encoded `AdapterFrame`,
//! capped at `adapters.max_frame_bytes` (4 MiB, `limits.yaml`). The reader
//! refuses oversized length prefixes before allocating; a clean EOF at a
//! frame boundary yields `None`, mid-frame EOF is `Unavailable`.
#![forbid(unsafe_code)]

use std::io::{Read, Write};

use domain::generated::contract::AdapterFrame;
use errors::KernelError;
use errors::codes::{ErrorCode, RetryClass};
use prost::Message;

/// Maximum adapter frame: `adapters.max_frame_bytes`.
pub const MAX_FRAME_BYTES: usize = 4_194_304;

/// Encodes a frame body to `len ++ bytes`.
pub fn encode_frame(frame: &AdapterFrame) -> Vec<u8> {
    let body = frame.encode_to_vec();
    let mut out = (body.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(&body);
    out
}

/// Writes one frame; the caller owns flush semantics.
pub fn write_frame(stream: &mut impl Write, frame: &AdapterFrame) -> errors::Result<()> {
    stream
        .write_all(&encode_frame(frame))
        .and_then(|()| stream.flush())
        .map_err(|e| io("write frame", e))
}

/// Reads one frame: `None` on clean EOF at a boundary, error on
/// oversized/malformed/mid-frame EOF.
pub fn read_frame(stream: &mut impl Read) -> errors::Result<Option<AdapterFrame>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(io("read frame length", e)),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(KernelError::new(
            ErrorCode::ResourceExhausted,
            RetryClass::Never,
            format!("adapter frame {len} exceeds {MAX_FRAME_BYTES} bytes"),
        ));
    }
    let mut body = vec![0u8; len];
    stream
        .read_exact(&mut body)
        .map_err(|e| io("read frame body", e))?;
    let frame = AdapterFrame::decode(body.as_slice()).map_err(|e| {
        KernelError::new(
            ErrorCode::InvalidArgument,
            RetryClass::Never,
            "adapter frame body is not a valid AdapterFrame",
        )
        .with_source(e)
    })?;
    Ok(Some(frame))
}

fn io(op: &str, source: std::io::Error) -> KernelError {
    KernelError::new(
        ErrorCode::Unavailable,
        RetryClass::Safe,
        format!("{op} failed"),
    )
    .with_source(source)
}
