use std::sync::Arc;
use std::time::Duration;

use backoff::ExponentialBackoffBuilder;
use tonic::metadata::MetadataValue;
use tonic::transport::{Channel, Endpoint};
use tonic::Request;
use tracing::{info, warn};

use crate::auth::AuthManager;
use crate::interceptor::{AGENT_ID_KEY, SESSION_TOKEN_KEY, TRACE_ID_KEY};
use crate::proto;
use crate::proto::cognos_agent_client::CognosAgentClient;
use crate::tls;

/// Configuration for the IPC client connection.
pub struct ClientConfig {
    pub address: String,
    pub agent_id: String,
    pub session_token: String,
    pub ca_cert_path: Option<String>,
    pub client_cert_path: Option<String>,
    pub client_key_path: Option<String>,
}

/// High-level gRPC client for COGNOS inter-agent IPC.
///
/// This client is cheaply cloneable (Channel is Arc-backed) and safe to share
/// across tasks. All methods take `&self` to allow concurrent RPC calls over
/// the same HTTP/2 connection.
#[derive(Clone)]
pub struct IPCClient {
    client: CognosAgentClient<Channel>,
    auth: Arc<AuthManager>,
    agent_id: String,
    session_token: String,
}

impl IPCClient {
    /// Connects to the COGNOS gRPC server with mTLS and exponential backoff.
    pub async fn connect(config: ClientConfig) -> anyhow::Result<Self> {
        let mut endpoint = Endpoint::from_shared(config.address.clone())?
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .keep_alive_timeout(Duration::from_secs(20))
            .http2_keep_alive_interval(Duration::from_secs(15));

        if let (Some(ca), Some(cert), Some(key)) = (
            &config.ca_cert_path,
            &config.client_cert_path,
            &config.client_key_path,
        ) {
            let tls_config = tls::tonic_client_tls(cert, key, ca).await?;
            endpoint = endpoint.tls_config(tls_config)?;
        }

        let channel = Self::connect_with_backoff(endpoint).await?;

        info!(address = %config.address, agent = %config.agent_id, "connected to COGNOS gRPC server");

        let auth = if let (Some(_cert), Some(key)) =
            (&config.client_cert_path, &config.client_key_path)
        {
            let key_bytes = tokio::fs::read(key).await?;
            AuthManager::from_pkcs8(&key_bytes).unwrap_or_else(|_| AuthManager::generate())
        } else {
            AuthManager::generate()
        };

        Ok(Self {
            client: CognosAgentClient::new(channel),
            auth: Arc::new(auth),
            agent_id: config.agent_id,
            session_token: config.session_token,
        })
    }

    async fn connect_with_backoff(endpoint: Endpoint) -> anyhow::Result<Channel> {
        let backoff = ExponentialBackoffBuilder::new()
            .with_initial_interval(Duration::from_millis(100))
            .with_max_interval(Duration::from_secs(5))
            .with_max_elapsed_time(Some(Duration::from_secs(30)))
            .build();

        let channel = backoff::future::retry(backoff, || async {
            endpoint.connect().await.map_err(|e| {
                warn!(error = %e, "connection attempt failed, retrying...");
                backoff::Error::transient(e)
            })
        })
        .await?;

        Ok(channel)
    }

    /// Injects standard COGNOS metadata into a gRPC request.
    fn inject_metadata<T>(&self, req: &mut Request<T>) {
        let md = req.metadata_mut();

        if let Ok(val) = self.agent_id.parse::<MetadataValue<_>>() {
            md.insert(AGENT_ID_KEY, val);
        }
        if let Ok(val) = self.session_token.parse::<MetadataValue<_>>() {
            md.insert(SESSION_TOKEN_KEY, val);
        }
        let trace_id = uuid::Uuid::new_v4().to_string();
        if let Ok(val) = trace_id.parse::<MetadataValue<_>>() {
            md.insert(TRACE_ID_KEY, val);
        }
    }

    /// Dispatches an intent envelope to the target agent through the coordinator.
    pub async fn dispatch(
        &self,
        mut envelope: proto::IntentEnvelope,
    ) -> anyhow::Result<proto::AgentResponse> {
        self.auth.sign_envelope(&mut envelope);

        let mut request = Request::new(envelope);
        self.inject_metadata(&mut request);

        let response = self
            .client
            .clone()
            .dispatch(request)
            .await
            .map_err(|s| anyhow::anyhow!("dispatch failed: {}", s.message()))?;

        let inner = response.into_inner();

        if !inner.success {
            if let Some(err) = &inner.error {
                anyhow::bail!(
                    "IPC error {} (retryable={}): {}",
                    err.code,
                    err.retryable,
                    err.message
                );
            }
            anyhow::bail!("unknown IPC failure");
        }

        Ok(inner)
    }

    /// Sends a heartbeat to the coordinator.
    pub async fn heartbeat(
        &self,
        version: &str,
        cpu_usage: f32,
        memory_usage: f32,
        active_capabilities: Vec<String>,
    ) -> anyhow::Result<proto::HeartbeatResponse> {
        let req = proto::HeartbeatRequest {
            agent_id: self.agent_id.clone(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            version: version.into(),
            cpu_usage,
            memory_usage,
            active_capabilities,
        };

        let mut request = Request::new(req);
        self.inject_metadata(&mut request);

        let response = self
            .client
            .clone()
            .heartbeat(request)
            .await
            .map_err(|s| anyhow::anyhow!("heartbeat failed: {}", s.message()))?;

        Ok(response.into_inner())
    }

    /// Requests capabilities from the coordinator/HAL.
    pub async fn request_capabilities(
        &self,
        capabilities: Vec<String>,
        justification: &str,
        intent_id: &str,
    ) -> anyhow::Result<proto::CapabilityResponse> {
        let req = proto::CapabilityRequest {
            requesting_agent: self.agent_id.clone(),
            requested_capabilities: capabilities,
            justification: justification.into(),
            intent_id: intent_id.into(),
        };

        let mut request = Request::new(req);
        self.inject_metadata(&mut request);

        let response = self
            .client
            .clone()
            .capability_check(request)
            .await
            .map_err(|s| anyhow::anyhow!("capability check failed: {}", s.message()))?;

        Ok(response.into_inner())
    }

    /// Subscribes to the server-streamed event bus.
    pub async fn subscribe_events(
        &self,
        event_types: Vec<String>,
    ) -> anyhow::Result<tonic::Streaming<proto::AgentEvent>> {
        let sub = proto::EventSubscription {
            subscriber_agent: self.agent_id.clone(),
            event_types,
            filter_expression: String::new(),
        };

        let mut request = Request::new(sub);
        self.inject_metadata(&mut request);

        let response = self
            .client
            .clone()
            .event_stream(request)
            .await
            .map_err(|s| anyhow::anyhow!("event subscription failed: {}", s.message()))?;

        Ok(response.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_config_fields() {
        let config = ClientConfig {
            address: "http://127.0.0.1:50051".into(),
            agent_id: "planner".into(),
            session_token: "test-token".into(),
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
        };
        assert_eq!(config.agent_id, "planner");
    }
}
