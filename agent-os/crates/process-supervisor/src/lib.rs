//! Supervised external process lifecycle with private socketpair IPC.
//!
//! The supervisor spawns adapter/loop processes with a private
//! `AF_UNIX SOCK_STREAM` socketpair (child end on a fixed inherited fd —
//! never a discoverable listener), runs the digest-bound bootstrap
//! handshake, tracks pid/start-identity/instance, and escalates shutdown to
//! kill. Every process is durable as an `adapter_instances` row tied to the
//! daemon's fencing epoch; a drain or restart invalidates old instances.
#![forbid(unsafe_code)]

pub mod child;
pub mod spawn;

pub use child::{heartbeat, instance_state, mark_exited, mark_failed, mark_ready, record_spawn};
pub use spawn::{Child, ExitReason, SpawnSpec, handshake, spawn, terminate, wait};
