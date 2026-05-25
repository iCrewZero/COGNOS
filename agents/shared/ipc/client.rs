use std::time::Duration;

use tonic::{
    transport::{
        Certificate,
        Channel,
        ClientTlsConfig,
        Endpoint,
        Identity,
    },
    Request,
};

use tokio::time::timeout;

use tracing::{
    error,
    info,
};

use crate::{
    auth::AuthManager,
    envelope::IntentEnvelope,
};

pub mod proto {
    tonic::include_proto!(
        "cognos.ipc.v1"
    );
}

use proto::{
    cognos_agent_client::CognosAgentClient,

    AuditContext,
    CapabilityRequest,
    HeartbeatRequest,
    IntentEnvelope as ProtoEnvelope,
    Signature,
};

pub struct IPCClient {
    client:
        CognosAgentClient<Channel>,

    auth: AuthManager,
}

impl IPCClient {
    pub async fn connect(
        address: &str,

        ca_cert_path: &str,

        client_cert_path: &str,

        client_key_path: &str,
    ) -> anyhow::Result<Self> {
        let ca_cert =
            tokio::fs::read(
                ca_cert_path
            )
            .await?;

        let client_cert =
            tokio::fs::read(
                client_cert_path
            )
            .await?;

        let client_key =
            tokio::fs::read(
                client_key_path
            )
            .await?;

        let tls =
            ClientTlsConfig::new()
                .ca_certificate(
                    Certificate::from_pem(
                        ca_cert
                    )
                )
                .identity(
                    Identity::from_pem(
                        client_cert,
                        client_key,
                    )
                )
                .domain_name(
                    "cognos.local"
                );

        let endpoint =
            Endpoint::from_shared(
                address.to_string()
            )?
            .tls_config(tls)?
            .tcp_keepalive(
                Some(
                    Duration::from_secs(
                        30
                    )
                )
            )
            .connect_timeout(
                Duration::from_secs(
                    5
                )
            );

        let channel =
            endpoint
                .connect()
                .await?;

        info!(
            "connected to {}",
            address
        );

        Ok(Self {
            client:
                CognosAgentClient::new(
                    channel
                ),

            auth:
                AuthManager::generate(),
        })
    }

    pub async fn dispatch(
        &mut self,

        mut envelope:
            IntentEnvelope,
    ) -> anyhow::Result<String> {
        self.auth
            .sign_envelope(
                &mut envelope
            );

        let proto =
            self
                .to_proto(
                    envelope
                );

        let request =
            Request::new(proto);

        let response =
            timeout(
                Duration::from_secs(
                    10
                ),

                self.client
                    .dispatch(
                        request
                    ),
            )
            .await??;

        let inner =
            response.into_inner();

        if !inner.success {
            if let Some(err) =
                inner.error
            {
                anyhow::bail!(
                    "IPC error {}: {}",
                    err.code,
                    err.message
                );
            }

            anyhow::bail!(
                "unknown IPC failure"
            );
        }

        Ok(inner.result_json)
    }

    pub async fn heartbeat(
        &mut self,

        agent_id: &str,

        version: &str,

        cpu_usage: f32,

        memory_usage: f32,
    ) -> anyhow::Result<()> {
        let req =
            HeartbeatRequest {
                agent_id:
                    agent_id.into(),

                timestamp_unix_ms:
                    chrono::Utc::now()
                        .timestamp_millis(),

                version:
                    version.into(),

                cpu_usage,

                memory_usage,

                active_capabilities:
                    vec![],
            };

        self.client
            .heartbeat(
                Request::new(req)
            )
            .await?;

        Ok(())
    }

    pub async fn request_capabilities(
        &mut self,

        requesting_agent: &str,

        capabilities:
            Vec<String>,

        justification: &str,

        intent_id: &str,
    ) -> anyhow::Result<bool> {
        let req =
            CapabilityRequest {
                requesting_agent:
                    requesting_agent
                        .into(),

                requested_capabilities:
                    capabilities,

                justification:
                    justification
                        .into(),

                intent_id:
                    intent_id.into(),
            };

        let response =
            self.client
                .capability_check(
                    Request::new(req)
                )
                .await?;

        Ok(
            response
                .into_inner()
                .approved
        )
    }

    fn to_proto(
        &self,

        envelope:
            IntentEnvelope,
    ) -> ProtoEnvelope {
        ProtoEnvelope {
            envelope_id:
                envelope
                    .envelope_id,

            intent_id:
                envelope.intent_id,

            source_agent:
                envelope
                    .source_agent,

            target_agent:
                envelope
                    .target_agent,

            capability_token:
                envelope
                    .capability_token,

            action_graph_hash:
                envelope
                    .action_graph_hash,

            timestamp_unix_ms:
                envelope
                    .timestamp_unix_ms,

            nonce:
                envelope.nonce,

            session_id:
                envelope
                    .session_id,

            user_id:
                envelope.user_id,

            intent_type:
                envelope
                    .intent_type,

            intent_payload_json:
                envelope
                    .intent_payload_json,

            risk_estimate:
                envelope
                    .risk_estimate,

            trust_score:
                envelope
                    .trust_score,

            requires_hal:
                envelope
                    .requires_hal,

            requested_capabilities:
                envelope
                    .requested_capabilities,

            audit:
                Some(
                    AuditContext {
                        audit_id:
                            envelope
                                .audit
                                .audit_id,

                        parent_hash:
                            envelope
                                .audit
                                .parent_hash,

                        event_hash:
                            envelope
                                .audit
                                .event_hash,

                        originating_host:
                            envelope
                                .audit
                                .originating_host,

                        originating_process:
                            envelope
                                .audit
                                .originating_process,

                        created_at:
                            envelope
                                .audit
                                .created_at,
                    }
                ),

            signature:
                envelope
                    .signature
                    .map(
                        |sig| {
                            Signature {
                                algorithm:
                                    sig.algorithm,

                                public_key:
                                    sig.public_key,

                                signature_bytes:
                                    sig.signature_bytes,
                            }
                        }
                    ),
        }
    }
}