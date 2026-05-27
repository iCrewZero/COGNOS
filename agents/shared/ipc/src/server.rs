use std::net::SocketAddr;
use std::pin::Pin;

use tokio::sync::broadcast;
use tokio_stream::{wrappers::BroadcastStream, Stream, StreamExt};
use tonic::transport::Server;
use tonic::{Request, Response, Status, Streaming};
use tracing::{info, warn};
use uuid::Uuid;

use crate::auth::AuthManager;
use crate::capability::CapabilityRuntime;
use crate::interceptor::{AuthInterceptor, RateLimiter, SESSION_TOKEN_KEY};
use crate::proto;
use crate::proto::cognos_agent_server::{CognosAgent, CognosAgentServer};
use crate::registry::AgentRegistry;

/// The gRPC service implementation for the COGNOS Agent IPC layer.
pub struct CognosAgentService {
    registry: AgentRegistry,
    capability_runtime: CapabilityRuntime,
    auth_interceptor: AuthInterceptor,
    rate_limiter: RateLimiter,
    auth_manager: AuthManager,
    event_tx: broadcast::Sender<proto::AgentEvent>,
}

impl CognosAgentService {
    pub fn new(
        registry: AgentRegistry,
        capability_runtime: CapabilityRuntime,
        auth_interceptor: AuthInterceptor,
        auth_manager: AuthManager,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(1024);
        Self {
            registry,
            capability_runtime,
            auth_interceptor,
            rate_limiter: RateLimiter::new(100),
            auth_manager,
            event_tx,
        }
    }

    /// Publishes an event to all subscribers of the event stream.
    pub fn publish_event(&self, event: proto::AgentEvent) {
        let _ = self.event_tx.send(event);
    }

    fn extract_agent_id<T>(&self, req: &Request<T>) -> Result<String, Status> {
        req.metadata()
            .get("x-cognos-agent-id")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| Status::unauthenticated("missing agent identity"))
    }

    fn validate_session<T>(&self, req: &Request<T>) -> Result<String, Status> {
        let token = req
            .metadata()
            .get(SESSION_TOKEN_KEY)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing session token"))?;

        let interceptor_req = Request::new(());
        // Lightweight token presence check
        let _ = token;
        Ok(token.to_string())
    }
}

#[tonic::async_trait]
impl CognosAgent for CognosAgentService {
    async fn dispatch(
        &self,
        request: Request<proto::IntentEnvelope>,
    ) -> Result<Response<proto::AgentResponse>, Status> {
        let envelope = request.into_inner();

        // Rate limit check
        self.rate_limiter.check(&envelope.source_agent)?;

        // Verify envelope signature
        if !AuthManager::verify_envelope(&envelope) {
            return Err(Status::unauthenticated("invalid envelope signature"));
        }

        // Validate integrity (nonce dedup + replay window)
        self.auth_manager
            .validate_integrity(&envelope)
            .map_err(|e| Status::permission_denied(e))?;

        // Capability lattice enforcement
        self.capability_runtime
            .validate_envelope(&envelope)
            .await
            .map_err(|e| Status::permission_denied(e))?;

        // Verify target agent exists
        if !self.registry.exists(&envelope.target_agent).await {
            return Err(Status::not_found(format!(
                "target agent '{}' not registered",
                envelope.target_agent
            )));
        }

        info!(
            source = %envelope.source_agent,
            target = %envelope.target_agent,
            intent = %envelope.intent_type,
            "dispatching intent"
        );

        // Emit audit event
        self.publish_event(proto::AgentEvent {
            event_id: Uuid::new_v4().to_string(),
            source_agent: "coordinator".into(),
            event_type: "intent_dispatched".into(),
            payload_json: serde_json::json!({
                "envelope_id": envelope.envelope_id,
                "source": envelope.source_agent,
                "target": envelope.target_agent,
                "intent_type": envelope.intent_type,
            })
            .to_string(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            severity: "info".into(),
        });

        let response = proto::AgentResponse {
            success: true,
            response_id: Uuid::new_v4().to_string(),
            request_envelope_id: envelope.envelope_id,
            responding_agent: "coordinator".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            result_json: "{}".into(),
            warnings: vec![],
            audit_events: vec![],
            error: None,
        };

        Ok(Response::new(response))
    }

    async fn heartbeat(
        &self,
        request: Request<proto::HeartbeatRequest>,
    ) -> Result<Response<proto::HeartbeatResponse>, Status> {
        let req = request.into_inner();

        if req.agent_id.is_empty() {
            return Err(Status::invalid_argument("agent_id is required"));
        }

        self.registry.update_heartbeat(&req.agent_id).await;

        info!(
            agent = %req.agent_id,
            cpu = req.cpu_usage,
            mem = req.memory_usage,
            "heartbeat received"
        );

        let response = proto::HeartbeatResponse {
            accepted: true,
            orchestrator_time: chrono::Utc::now().to_rfc3339(),
            revoked_capabilities: vec![],
        };

        Ok(Response::new(response))
    }

    async fn capability_check(
        &self,
        request: Request<proto::CapabilityRequest>,
    ) -> Result<Response<proto::CapabilityResponse>, Status> {
        let req = request.into_inner();

        if req.requesting_agent.is_empty() {
            return Err(Status::invalid_argument("requesting_agent is required"));
        }

        let mut granted = vec![];
        let mut denied = vec![];

        for cap_str in &req.requested_capabilities {
            let cap = crate::capability::Capability::from_str(cap_str);
            match cap {
                Some(c) => {
                    if self
                        .capability_runtime
                        .has_capability(&req.requesting_agent, &c)
                        .await
                    {
                        granted.push(cap_str.clone());
                    } else {
                        denied.push(cap_str.clone());
                    }
                }
                None => {
                    denied.push(cap_str.clone());
                }
            }
        }

        let approved = denied.is_empty() && !granted.is_empty();

        info!(
            agent = %req.requesting_agent,
            approved = approved,
            granted = ?granted,
            denied = ?denied,
            "capability check"
        );

        let response = proto::CapabilityResponse {
            approved,
            granted_capabilities: granted,
            denied_capabilities: denied,
            hal_decision_id: Uuid::new_v4().to_string(),
            computed_risk: 0.0,
        };

        Ok(Response::new(response))
    }

    type HeartbeatStreamStream =
        Pin<Box<dyn Stream<Item = Result<proto::HeartbeatResponse, Status>> + Send>>;

    async fn heartbeat_stream(
        &self,
        request: Request<Streaming<proto::HeartbeatRequest>>,
    ) -> Result<Response<Self::HeartbeatStreamStream>, Status> {
        let registry = self.registry.clone();
        let mut stream = request.into_inner();

        let output = async_stream::try_stream! {
            while let Some(req) = stream.next().await {
                let req = req?;

                if req.agent_id.is_empty() {
                    Err(Status::invalid_argument("agent_id is required"))?;
                }

                registry.update_heartbeat(&req.agent_id).await;

                yield proto::HeartbeatResponse {
                    accepted: true,
                    orchestrator_time: chrono::Utc::now().to_rfc3339(),
                    revoked_capabilities: vec![],
                };
            }
        };

        Ok(Response::new(Box::pin(output)))
    }

    type EventStreamStream =
        Pin<Box<dyn Stream<Item = Result<proto::AgentEvent, Status>> + Send>>;

    async fn event_stream(
        &self,
        request: Request<proto::EventSubscription>,
    ) -> Result<Response<Self::EventStreamStream>, Status> {
        let subscription = request.into_inner();
        let event_types: Vec<String> = subscription.event_types;

        let rx = self.event_tx.subscribe();
        let stream = BroadcastStream::new(rx).filter_map(move |result| {
            match result {
                Ok(event) => {
                    if event_types.is_empty()
                        || event_types.contains(&event.event_type)
                    {
                        Some(Ok(event))
                    } else {
                        None
                    }
                }
                Err(_) => None,
            }
        });

        Ok(Response::new(Box::pin(stream)))
    }
}

/// Configuration for launching the gRPC server.
pub struct ServerConfig {
    pub listen_addr: SocketAddr,
    pub cert_path: Option<String>,
    pub key_path: Option<String>,
    pub ca_path: Option<String>,
}

/// Starts the full COGNOS gRPC server with health check, reflection, and the agent service.
pub async fn start_server(
    config: ServerConfig,
    service: CognosAgentService,
) -> anyhow::Result<()> {
    let (mut health_reporter, health_service) = tonic_health::server::health_reporter();
    health_reporter
        .set_serving::<CognosAgentServer<CognosAgentService>>()
        .await;

    let reflection_service = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(proto::FILE_DESCRIPTOR_SET)
        .build()?;

    let grpc_service = CognosAgentServer::new(service);

    let mut builder = Server::builder();

    if let (Some(cert), Some(key), Some(ca)) =
        (&config.cert_path, &config.key_path, &config.ca_path)
    {
        let tls = crate::tls::tonic_server_tls(cert, key, ca).await?;
        builder = builder.tls_config(tls)?;
    }

    info!(addr = %config.listen_addr, "starting COGNOS gRPC server");

    builder
        .add_service(health_service)
        .add_service(reflection_service)
        .add_service(grpc_service)
        .serve(config.listen_addr)
        .await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::AgentMetadata;

    fn make_service() -> CognosAgentService {
        let registry = AgentRegistry::new();
        let capability_runtime = CapabilityRuntime::new(registry.clone());
        let auth_interceptor = AuthInterceptor::new();
        let auth_manager = AuthManager::generate();
        CognosAgentService::new(registry, capability_runtime, auth_interceptor, auth_manager)
    }

    #[tokio::test]
    async fn heartbeat_updates_registry() {
        let service = make_service();
        service
            .registry
            .register(AgentMetadata {
                agent_id: "planner".into(),
                service_name: "planner".into(),
                address: "127.0.0.1:50051".into(),
                public_key: vec![],
                capabilities: vec![],
                trust_score: 0.9,
                last_heartbeat: 0,
                healthy: false,
            })
            .await;

        let req = Request::new(proto::HeartbeatRequest {
            agent_id: "planner".into(),
            timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
            version: "0.1.0".into(),
            cpu_usage: 0.5,
            memory_usage: 0.3,
            active_capabilities: vec![],
        });

        let resp = service.heartbeat(req).await.unwrap();
        assert!(resp.into_inner().accepted);

        let agent = service.registry.get("planner").await.unwrap();
        assert!(agent.healthy);
    }

    #[tokio::test]
    async fn dispatch_rejects_unsigned_envelope() {
        let service = make_service();
        let envelope = proto::IntentEnvelope {
            envelope_id: "test".into(),
            source_agent: "planner".into(),
            target_agent: "memory".into(),
            ..Default::default()
        };

        let req = Request::new(envelope);
        let result = service.dispatch(req).await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code(), tonic::Code::Unauthenticated);
    }
}
