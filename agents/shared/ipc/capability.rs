use std::{
    collections::{
        HashMap,
        HashSet,
    },
    sync::Arc,
};

use tokio::sync::RwLock;

use tracing::{
    info,
    warn,
};

use crate::{
    envelope::IntentEnvelope,
    registry::AgentRegistry,
};

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
)]
pub enum Capability {
    MemoryRead,
    MemoryWrite,

    FilesystemRead,
    FilesystemWrite,

    NetworkOutbound,
    NetworkInbound,

    ProcessSpawn,

    ShellExecute,

    ModelInference,

    SchedulerControl,

    UIOverlay,

    HALOverride,
}

impl Capability {
    pub fn as_str(
        &self
    ) -> &'static str {
        match self {
            Self::MemoryRead =>
                "memory.read",

            Self::MemoryWrite =>
                "memory.write",

            Self::FilesystemRead =>
                "filesystem.read",

            Self::FilesystemWrite =>
                "filesystem.write",

            Self::NetworkOutbound =>
                "network.outbound",

            Self::NetworkInbound =>
                "network.inbound",

            Self::ProcessSpawn =>
                "process.spawn",

            Self::ShellExecute =>
                "shell.execute",

            Self::ModelInference =>
                "model.inference",

            Self::SchedulerControl =>
                "scheduler.control",

            Self::UIOverlay =>
                "ui.overlay",

            Self::HALOverride =>
                "hal.override",
        }
    }

    pub fn from_str(
        value: &str
    ) -> Option<Self> {
        match value {
            "memory.read" =>
                Some(
                    Self::MemoryRead
                ),

            "memory.write" =>
                Some(
                    Self::MemoryWrite
                ),

            "filesystem.read" =>
                Some(
                    Self::FilesystemRead
                ),

            "filesystem.write" =>
                Some(
                    Self::FilesystemWrite
                ),

            "network.outbound" =>
                Some(
                    Self::NetworkOutbound
                ),

            "network.inbound" =>
                Some(
                    Self::NetworkInbound
                ),

            "process.spawn" =>
                Some(
                    Self::ProcessSpawn
                ),

            "shell.execute" =>
                Some(
                    Self::ShellExecute
                ),

            "model.inference" =>
                Some(
                    Self::ModelInference
                ),

            "scheduler.control" =>
                Some(
                    Self::SchedulerControl
                ),

            "ui.overlay" =>
                Some(
                    Self::UIOverlay
                ),

            "hal.override" =>
                Some(
                    Self::HALOverride
                ),

            _ => None,
        }
    }
}

#[derive(Clone)]
pub struct CapabilityRuntime {
    registry:
        AgentRegistry,

    active_tokens:
        Arc<
            RwLock<
                HashMap<
                    String,
                    HashSet<
                        Capability
                    >,
                >,
            >,
        >,
}

impl CapabilityRuntime {
    pub fn new(
        registry:
            AgentRegistry,
    ) -> Self {
        Self {
            registry,

            active_tokens:
                Arc::new(
                    RwLock::new(
                        HashMap::new()
                    )
                ),
        }
    }

    pub async fn grant(
        &self,

        agent_id: &str,

        capability:
            Capability,
    ) {
        let mut tokens =
            self
                .active_tokens
                .write()
                .await;

        let entry =
            tokens
                .entry(
                    agent_id
                        .to_string()
                )
                .or_insert_with(
                    HashSet::new
                );

        info!(
            "granting {} to {}",
            capability.as_str(),
            agent_id
        );

        entry.insert(
            capability
        );
    }

    pub async fn revoke(
        &self,

        agent_id: &str,

        capability:
            &Capability,
    ) {
        let mut tokens =
            self
                .active_tokens
                .write()
                .await;

        if let Some(set) =
            tokens.get_mut(
                agent_id
            )
        {
            set.remove(
                capability
            );

            warn!(
                "revoked {} from {}",
                capability.as_str(),
                agent_id
            );
        }
    }

    pub async fn has_capability(
        &self,

        agent_id: &str,

        capability:
            &Capability,
    ) -> bool {
        let tokens =
            self
                .active_tokens
                .read()
                .await;

        tokens
            .get(agent_id)
            .map(|set| {
                set.contains(
                    capability
                )
            })
            .unwrap_or(false)
    }

    pub async fn validate_envelope(
        &self,

        envelope:
            &IntentEnvelope,
    ) -> Result<(), String> {
        let agent =
            self
                .registry
                .get(
                    &envelope
                        .source_agent
                )
                .await
                .ok_or_else(
                    || {
                        "unknown agent"
                            .to_string()
                    }
                )?;

        if !agent.healthy {
            return Err(
                "agent unhealthy"
                    .into()
            );
        }

        if agent.trust_score
            < 0.20
        {
            return Err(
                "agent trust score too low"
                    .into()
            );
        }

        for requested in
            &envelope
                .requested_capabilities
        {
            let capability =
                Capability::from_str(
                    requested
                )
                .ok_or_else(
                    || {
                        format!(
                            "unknown capability {}",
                            requested
                        )
                    }
                )?;

            let allowed =
                self
                    .has_capability(
                        &envelope
                            .source_agent,

                        &capability,
                    )
                    .await;

            if !allowed {
                return Err(
                    format!(
                        "missing capability {}",
                        requested
                    )
                );
            }
        }

        Ok(())
    }

    pub async fn isolate_agent(
        &self,

        agent_id: &str,
    ) {
        let mut tokens =
            self
                .active_tokens
                .write()
                .await;

        tokens.remove(
            agent_id
        );

        warn!(
            "isolated agent {}",
            agent_id
        );
    }

    pub async fn snapshot(
        &self,
    ) -> HashMap<
        String,
        Vec<String>
    > {
        let tokens =
            self
                .active_tokens
                .read()
                .await;

        tokens
            .iter()
            .map(
                |(
                    agent,
                    caps,
                )| {
                    (
                        agent.clone(),

                        caps.iter()
                            .map(
                                |c| {
                                    c.as_str()
                                        .to_string()
                                }
                            )
                            .collect(),
                    )
                },
            )
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::Utc;

    use crate::registry::{
        AgentMetadata,
    };

    #[tokio::test]
    async fn capability_grant() {
        let registry =
            AgentRegistry::new();

        registry
            .register(
                AgentMetadata {
                    agent_id:
                        "planner"
                            .into(),

                    service_name:
                        "planner"
                            .into(),

                    address:
                        "127.0.0.1"
                            .into(),

                    public_key:
                        vec![
                            1,
                            2,
                            3
                        ],

                    capabilities:
                        vec![],

                    trust_score:
                        0.91,

                    last_heartbeat:
                        Utc::now()
                            .timestamp_millis(),

                    healthy: true,
                }
            )
            .await;

        let runtime =
            CapabilityRuntime::new(
                registry
            );

        runtime
            .grant(
                "planner",

                Capability::MemoryRead,
            )
            .await;

        assert!(
            runtime
                .has_capability(
                    "planner",

                    &Capability::MemoryRead,
                )
                .await
        );
    }
}