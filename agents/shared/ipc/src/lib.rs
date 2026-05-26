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
