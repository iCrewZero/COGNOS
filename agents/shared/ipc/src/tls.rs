use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

use rustls::{Certificate, ClientConfig, PrivateKey, RootCertStore, ServerConfig};
use rustls_pemfile::{certs, pkcs8_private_keys};
use tokio::sync::watch;
use tokio_rustls::{TlsAcceptor, TlsConnector};
use tonic::transport::{ClientTlsConfig, Identity, ServerTlsConfig};
use tracing::{info, warn};

/// Tracks file modification times for hot-reload detection.
struct CertPaths {
    cert_path: PathBuf,
    key_path: PathBuf,
    ca_path: PathBuf,
}

impl CertPaths {
    fn last_modified(&self) -> Option<SystemTime> {
        [&self.cert_path, &self.key_path, &self.ca_path]
            .iter()
            .filter_map(|p| fs::metadata(p).ok()?.modified().ok())
            .max()
    }
}

/// Builds mTLS configurations and supports hot-reload of certificates.
pub struct TlsProvider {
    paths: CertPaths,
    server_config: Arc<ServerConfig>,
}

impl TlsProvider {
    /// Loads an initial server TLS configuration with mTLS and returns a TlsProvider
    /// capable of hot-reloading certificates.
    pub fn new_server(
        cert_path: &str,
        key_path: &str,
        ca_path: &str,
    ) -> anyhow::Result<Self> {
        let config = Self::build_server_config(cert_path, key_path, ca_path)?;
        Ok(Self {
            paths: CertPaths {
                cert_path: PathBuf::from(cert_path),
                key_path: PathBuf::from(key_path),
                ca_path: PathBuf::from(ca_path),
            },
            server_config: config,
        })
    }

    /// Attempts to reload certificates from disk. Returns true if updated.
    pub fn try_reload(&mut self) -> bool {
        match Self::build_server_config(
            self.paths.cert_path.to_str().unwrap_or_default(),
            self.paths.key_path.to_str().unwrap_or_default(),
            self.paths.ca_path.to_str().unwrap_or_default(),
        ) {
            Ok(new_config) => {
                self.server_config = new_config;
                info!("TLS certificates reloaded successfully");
                true
            }
            Err(e) => {
                warn!(error = %e, "failed to reload TLS certificates, keeping current config");
                false
            }
        }
    }

    pub fn server_config(&self) -> Arc<ServerConfig> {
        self.server_config.clone()
    }

    pub fn acceptor(&self) -> TlsAcceptor {
        TlsAcceptor::from(self.server_config.clone())
    }

    fn build_server_config(
        cert_path: &str,
        key_path: &str,
        ca_path: &str,
    ) -> anyhow::Result<Arc<ServerConfig>> {
        let cert_file = fs::read(cert_path)?;
        let key_file = fs::read(key_path)?;
        let ca_file = fs::read(ca_path)?;

        let cert_chain = certs(&mut cert_file.as_slice())?
            .into_iter()
            .map(Certificate)
            .collect::<Vec<_>>();

        if cert_chain.is_empty() {
            anyhow::bail!("no certificates found in {}", cert_path);
        }

        let private_key = pkcs8_private_keys(&mut key_file.as_slice())?
            .into_iter()
            .next()
            .map(PrivateKey)
            .ok_or_else(|| anyhow::anyhow!("no private key found in {}", key_path))?;

        let mut roots = RootCertStore::empty();
        let ca_certs = certs(&mut ca_file.as_slice())?;
        if ca_certs.is_empty() {
            anyhow::bail!("no CA certificates found in {}", ca_path);
        }
        for cert in ca_certs {
            roots.add(&Certificate(cert))?;
        }

        let verifier = rustls::server::AllowAnyAuthenticatedClient::new(roots);

        let config = ServerConfig::builder()
            .with_safe_defaults()
            .with_client_cert_verifier(verifier)
            .with_single_cert(cert_chain, private_key)?;

        Ok(Arc::new(config))
    }
}

/// Builds an mTLS client configuration.
pub fn build_client_config(
    cert_path: &str,
    key_path: &str,
    ca_path: &str,
) -> anyhow::Result<Arc<ClientConfig>> {
    let cert_file = fs::read(cert_path)?;
    let key_file = fs::read(key_path)?;
    let ca_file = fs::read(ca_path)?;

    let cert_chain = certs(&mut cert_file.as_slice())?
        .into_iter()
        .map(Certificate)
        .collect::<Vec<_>>();

    if cert_chain.is_empty() {
        anyhow::bail!("no client certificates found in {}", cert_path);
    }

    let private_key = pkcs8_private_keys(&mut key_file.as_slice())?
        .into_iter()
        .next()
        .map(PrivateKey)
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", key_path))?;

    let mut roots = RootCertStore::empty();
    let ca_certs = certs(&mut ca_file.as_slice())?;
    if ca_certs.is_empty() {
        anyhow::bail!("no CA certificates found in {}", ca_path);
    }
    for cert in ca_certs {
        roots.add(&Certificate(cert))?;
    }

    let config = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(roots)
        .with_single_cert(cert_chain, private_key)?;

    Ok(Arc::new(config))
}

pub fn build_connector(config: Arc<ClientConfig>) -> TlsConnector {
    TlsConnector::from(config)
}

/// Builds a Tonic-native server TLS config from PEM files.
pub async fn tonic_server_tls(
    cert_path: &str,
    key_path: &str,
    ca_path: &str,
) -> anyhow::Result<ServerTlsConfig> {
    let cert = tokio::fs::read(cert_path).await?;
    let key = tokio::fs::read(key_path).await?;
    let ca = tokio::fs::read(ca_path).await?;

    let tls = ServerTlsConfig::new()
        .identity(Identity::from_pem(cert, key))
        .client_ca_root(tonic::transport::Certificate::from_pem(ca));

    Ok(tls)
}

/// Builds a Tonic-native client TLS config from PEM files.
pub async fn tonic_client_tls(
    cert_path: &str,
    key_path: &str,
    ca_path: &str,
) -> anyhow::Result<ClientTlsConfig> {
    let ca = tokio::fs::read(ca_path).await?;
    let cert = tokio::fs::read(cert_path).await?;
    let key = tokio::fs::read(key_path).await?;

    let tls = ClientTlsConfig::new()
        .ca_certificate(tonic::transport::Certificate::from_pem(ca))
        .identity(Identity::from_pem(cert, key))
        .domain_name("cognos.local");

    Ok(tls)
}

/// Spawns a background task that periodically checks for certificate changes
/// and triggers a reload callback.
pub async fn spawn_cert_watcher(
    cert_path: String,
    key_path: String,
    ca_path: String,
    reload_tx: watch::Sender<()>,
    check_interval: std::time::Duration,
) {
    tokio::spawn(async move {
        let paths = CertPaths {
            cert_path: PathBuf::from(&cert_path),
            key_path: PathBuf::from(&key_path),
            ca_path: PathBuf::from(&ca_path),
        };

        let mut last_modified = paths.last_modified();

        loop {
            tokio::time::sleep(check_interval).await;

            let current = paths.last_modified();
            if current != last_modified {
                info!("certificate file change detected, signaling reload");
                last_modified = current;
                let _ = reload_tx.send(());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_provider_type_exists() {
        let _ = std::mem::size_of::<TlsProvider>();
    }

    #[test]
    fn cert_paths_handles_missing_files() {
        let paths = CertPaths {
            cert_path: PathBuf::from("/nonexistent/cert.pem"),
            key_path: PathBuf::from("/nonexistent/key.pem"),
            ca_path: PathBuf::from("/nonexistent/ca.pem"),
        };
        assert!(paths.last_modified().is_none());
    }
}
