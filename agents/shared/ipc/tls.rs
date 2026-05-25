use std::{
    fs,
    sync::Arc,
};

use rustls::{
    Certificate,
    ClientConfig,
    PrivateKey,
    RootCertStore,
    ServerConfig,
};

use rustls_pemfile::{
    certs,
    pkcs8_private_keys,
};

use tokio_rustls::{
    TlsAcceptor,
    TlsConnector,
};

pub struct TLSConfig;

impl TLSConfig {
    pub fn load_server_config(
        cert_path: &str,
        key_path: &str,
        ca_path: &str,
    ) -> anyhow::Result<Arc<ServerConfig>> {
        let cert_file =
            fs::read(cert_path)?;

        let key_file =
            fs::read(key_path)?;

        let ca_file =
            fs::read(ca_path)?;

        let cert_chain =
            certs(
                &mut cert_file.as_slice()
            )?
            .into_iter()
            .map(Certificate)
            .collect::<Vec<_>>();

        let private_key =
            pkcs8_private_keys(
                &mut key_file.as_slice()
            )?
            .into_iter()
            .next()
            .map(PrivateKey)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "missing private key"
                )
            })?;

        let mut roots =
            RootCertStore::empty();

        for cert in certs(
            &mut ca_file.as_slice()
        )? {
            roots.add(
                &Certificate(cert)
            )?;
        }

        let verifier =
            rustls::server::
                AllowAnyAuthenticatedClient::new(
                    roots,
                );

        let config =
            ServerConfig::builder()
                .with_safe_defaults()
                .with_client_cert_verifier(
                    verifier,
                )
                .with_single_cert(
                    cert_chain,
                    private_key,
                )?;

        Ok(Arc::new(config))
    }

    pub fn load_client_config(
        cert_path: &str,
        key_path: &str,
        ca_path: &str,
    ) -> anyhow::Result<Arc<ClientConfig>> {
        let cert_file =
            fs::read(cert_path)?;

        let key_file =
            fs::read(key_path)?;

        let ca_file =
            fs::read(ca_path)?;

        let cert_chain =
            certs(
                &mut cert_file.as_slice()
            )?
            .into_iter()
            .map(Certificate)
            .collect::<Vec<_>>();

        let private_key =
            pkcs8_private_keys(
                &mut key_file.as_slice()
            )?
            .into_iter()
            .next()
            .map(PrivateKey)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "missing private key"
                )
            })?;

        let mut roots =
            RootCertStore::empty();

        for cert in certs(
            &mut ca_file.as_slice()
        )? {
            roots.add(
                &Certificate(cert)
            )?;
        }

        let config =
            ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(
                    roots,
                )
                .with_single_cert(
                    cert_chain,
                    private_key,
                )?;

        Ok(Arc::new(config))
    }

    pub fn build_acceptor(
        config: Arc<ServerConfig>,
    ) -> TlsAcceptor {
        TlsAcceptor::from(config)
    }

    pub fn build_connector(
        config: Arc<ClientConfig>,
    ) -> TlsConnector {
        TlsConnector::from(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_type_exists() {
        let _ =
            std::mem::size_of::<TLSConfig>();

        assert!(true);
    }
}