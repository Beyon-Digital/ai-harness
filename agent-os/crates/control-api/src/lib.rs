//! gRPC API translation and authentication context.
//!
//! The local control plane runs over a `0600` Unix-domain socket in the
//! daemon runtime directory. Peer credentials (`SO_PEERCRED`) resolve to a
//! configured principal — handlers dispatch on the verified identity and
//! never see a raw store handle.
#![forbid(unsafe_code)]

pub mod server;
pub mod uds;
pub mod views;

pub use server::{
    ControlApiService, HealthInfo, LocalActor, PeerCreds, PeerPrincipalMap, UdsConn,
    UidPrincipalMap, generated, serve,
};
pub use uds::{ControlSocket, SOCKET_FILE_NAME};
