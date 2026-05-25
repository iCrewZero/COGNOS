use std::{
    collections::HashMap,
    sync::Arc,
};

use tokio::sync::RwLock;

use chrono::Utc;

#[derive(Debug, Clone)]
pub struct AgentMetadata {
    pub agent_id: String,

    pub service_name: String,

    pub address: String,

    pub public_key: Vec<u8>,

    pub capabilities: Vec<String>,

    pub trust_score: f32,

    pub last_heartbeat: i64,

    pub healthy: bool,
}

#[derive(Clone)]
pub struct AgentRegistry {
    inner: Arc<
        RwLock<
            HashMap<
                String,
                AgentMetadata,
            >,
        >,
    >,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(
                RwLock::new(
                    HashMap::new()
                )
            ),
        }
    }

    pub async fn register(
        &self,
        metadata: AgentMetadata,
    ) {
        let mut registry =
            self.inner.write().await;

        registry.insert(
            metadata.agent_id.clone(),
            metadata,
        );
    }

    pub async fn unregister(
        &self,
        agent_id: &str,
    ) {
        let mut registry =
            self.inner.write().await;

        registry.remove(agent_id);
    }

    pub async fn update_heartbeat(
        &self,
        agent_id: &str,
    ) {
        let mut registry =
            self.inner.write().await;

        if let Some(agent) =
            registry.get_mut(agent_id)
        {
            agent.last_heartbeat =
                Utc::now()
                    .timestamp_millis();

            agent.healthy = true;
        }
    }

    pub async fn update_trust_score(
        &self,
        agent_id: &str,
        trust_score: f32,
    ) {
        let mut registry =
            self.inner.write().await;

        if let Some(agent) =
            registry.get_mut(agent_id)
        {
            agent.trust_score =
                trust_score;
        }
    }

    pub async fn mark_unhealthy(
        &self,
        agent_id: &str,
    ) {
        let mut registry =
            self.inner.write().await;

        if let Some(agent) =
            registry.get_mut(agent_id)
        {
            agent.healthy = false;
        }
    }

    pub async fn get(
        &self,
        agent_id: &str,
    ) -> Option<AgentMetadata> {
        let registry =
            self.inner.read().await;

        registry
            .get(agent_id)
            .cloned()
    }

    pub async fn exists(
        &self,
        agent_id: &str,
    ) -> bool {
        let registry =
            self.inner.read().await;

        registry.contains_key(agent_id)
    }

    pub async fn list_agents(
        &self,
    ) -> Vec<AgentMetadata> {
        let registry =
            self.inner.read().await;

        registry
            .values()
            .cloned()
            .collect()
    }

    pub async fn healthy_agents(
        &self,
    ) -> Vec<AgentMetadata> {
        let registry =
            self.inner.read().await;

        registry
            .values()
            .filter(|a| a.healthy)
            .cloned()
            .collect()
    }

    pub async fn agents_with_capability(
        &self,
        capability: &str,
    ) -> Vec<AgentMetadata> {
        let registry =
            self.inner.read().await;

        registry
            .values()
            .filter(|agent| {
                agent
                    .capabilities
                    .iter()
                    .any(|c| c == capability)
            })
            .cloned()
            .collect()
    }

    pub async fn verify_agent_key(
        &self,
        agent_id: &str,
        public_key: &[u8],
    ) -> bool {
        let registry =
            self.inner.read().await;

        if let Some(agent) =
            registry.get(agent_id)
        {
            return agent.public_key
                == public_key;
        }

        false
    }

    pub async fn stale_agents(
        &self,
        timeout_ms: i64,
    ) -> Vec<String> {
        let registry =
            self.inner.read().await;

        let now =
            Utc::now()
                .timestamp_millis();

        registry
            .values()
            .filter(|agent| {
                now - agent.last_heartbeat
                    > timeout_ms
            })
            .map(|agent| {
                agent.agent_id.clone()
            })
            .collect()
    }

    pub async fn cleanup_stale(
        &self,
        timeout_ms: i64,
    ) {
        let stale =
            self
                .stale_agents(
                    timeout_ms
                )
                .await;

        let mut registry =
            self.inner.write().await;

        for id in stale {
            registry.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_agent() {
        let registry =
            AgentRegistry::new();

        let metadata =
            AgentMetadata {
                agent_id:
                    "planner".into(),

                service_name:
                    "planner-service"
                        .into(),

                address:
                    "127.0.0.1:50051"
                        .into(),

                public_key:
                    vec![1, 2, 3],

                capabilities:
                    vec![
                        "memory.read"
                            .into(),
                    ],

                trust_score: 0.91,

                last_heartbeat:
                    Utc::now()
                        .timestamp_millis(),

                healthy: true,
            };

        registry
            .register(metadata)
            .await;

        assert!(
            registry
                .exists("planner")
                .await
        );
    }

    #[tokio::test]
    async fn capability_lookup() {
        let registry =
            AgentRegistry::new();

        registry
            .register(
                AgentMetadata {
                    agent_id:
                        "memory".into(),

                    service_name:
                        "memory-service"
                            .into(),

                    address:
                        "127.0.0.1:50052"
                            .into(),

                    public_key:
                        vec![9, 9, 9],

                    capabilities:
                        vec![
                            "memory.read"
                                .into(),
                        ],

                    trust_score: 0.95,

                    last_heartbeat:
                        Utc::now()
                            .timestamp_millis(),

                    healthy: true,
                }
            )
            .await;

        let result =
            registry
                .agents_with_capability(
                    "memory.read"
                )
                .await;

        assert_eq!(
            result.len(),
            1
        );
    }
}