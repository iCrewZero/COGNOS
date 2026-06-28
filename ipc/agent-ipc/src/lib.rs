//! DEPRECATED: Use `cognos_ipc_grpc` (ipc/grpc) instead.
//!
//! This crate is a compatibility shim. It re-exports the same proto types
//! as cognos-ipc-grpc so existing code that depends on `cognos_ipc` still
//! compiles. New code should depend on `cognos_ipc_grpc` directly.
//!
//! Both crates now compile from the same canonical proto file
//! (ipc/grpc/proto/cognos.proto) so the wire format is identical.
//!
//! Owner: iCrewZero

pub mod proto {
    tonic::include_proto!("cognos.ipc.v1");

    pub const FILE_DESCRIPTOR_SET: &[u8] =
        tonic::include_file_descriptor_set!("cognos_descriptor");
}

pub mod auth;
pub mod capability;
pub mod client;
pub mod envelope;
pub mod interceptor;
pub mod registry;
pub mod server;
pub mod tls;
