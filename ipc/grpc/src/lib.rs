//! COGNOS gRPC IPC — the communication backbone between all agents,
//! the HAL, the intent-engine, the scheduler, and the memory service.
//!
//! Every RPC is wrapped in a signed [`Envelope`] so the server can
//! authenticate and authorize before any side effect.

pub mod auth;
pub mod client;
pub mod runtime;
pub mod server;

// Re-export the proto-generated types for convenience.
pub mod proto {
    pub mod v1 {
        tonic::include_proto!("cognos.ipc.v1");
    }
}

// Re-export commonly used proto types at the crate root.
pub use proto::v1::*;
