use std::{
    collections::{
        HashMap,
        VecDeque,
    },
    sync::Arc,
};

use tokio::sync::RwLock;

use chrono::Utc;

use crate::hal_types::{
    AuditEvent,
    EscalationContext,
    HALDecision,
    RuntimeAnomaly,
    TrustState,
};

#[derive(Clone)]
pub struct RuntimeState {
    pub trust_map:
        Arc<
            RwLock<
                HashMap<
                    String,
                    TrustState,
                >,
            >,
        >,

    pub active_decisions:
        Arc<
            RwLock<
                HashMap<
                    String,
                    HALDecision,
                >,
            >,
        >,

    pub anomaly_stream:
        Arc<
            RwLock<
                VecDeque<
                    RuntimeAnomaly,
                >,
            >,
        >,

    pub escalation_state:
        Arc<
            RwLock<
                HashMap<
                    String,
                    EscalationContext,
                >,
            >,
        >,

    pub audit_buffer:
        Arc<
            RwLock<
                VecDeque<
                    AuditEvent,
                >,
            >,
        >,
}

impl RuntimeState {
    pub fn new() -> Self {
        Self {
            trust_map:
                Arc::new(
                    RwLock::new(
                        HashMap::new()
                    )
                ),

            active_decisions:
                Arc::new(
                    RwLock::new(
                        HashMap::new()
                    )
                ),

            anomaly_stream:
                Arc::new(
                    RwLock::new(
                        VecDeque::new()
                    )
                ),

            escalation_state:
                Arc::new(
                    RwLock::new(
                        HashMap::new()
                    )
                ),

            audit_buffer:
                Arc::new(
                    RwLock::new(
                        VecDeque::new()
                    )
                ),
        }
    }

    pub async fn set_trust_state(
        &self,

        agent_id: &str,

        trust:
            TrustState,
    ) {
        let mut map =
            self
                .trust_map
                .write()
                .await;

        map.insert(
            agent_id.to_string(),
            trust,
        );
    }

    pub async fn get_trust_state(
        &self,

        agent_id: &str,
    ) -> Option<TrustState> {
        let map =
            self
                .trust_map
                .read()
                .await;

        map.get(agent_id)
            .cloned()
    }

    pub async fn update_decision(
        &self,

        intent_id: &str,

        decision:
            HALDecision,
    ) {
        let mut decisions =
            self
                .active_decisions
                .write()
                .await;

        decisions.insert(
            intent_id.to_string(),
            decision,
        );
    }

    pub async fn get_decision(
        &self,

        intent_id: &str,
    ) -> Option<HALDecision> {
        let decisions =
            self
                .active_decisions
                .read()
                .await;

        decisions
            .get(intent_id)
            .cloned()
    }

    pub async fn push_anomaly(
        &self,

        anomaly:
            RuntimeAnomaly,
    ) {
        let mut stream =
            self
                .anomaly_stream
                .write()
                .await;

        stream.push_back(
            anomaly
        );

        if stream.len() > 5000 {
            stream.pop_front();
        }
    }

    pub async fn recent_anomalies(
        &self,
    ) -> Vec<
        RuntimeAnomaly
    > {
        let stream =
            self
                .anomaly_stream
                .read()
                .await;

        stream
            .iter()
            .cloned()
            .collect()
    }

    pub async fn escalation_active(
        &self,

        agent_id: &str,
    ) -> bool {
        let state =
            self
                .escalation_state
                .read()
                .await;

        state.contains_key(
            agent_id
        )
    }

    pub async fn set_escalation(
        &self,

        agent_id: &str,

        escalation:
            EscalationContext,
    ) {
        let mut state =
            self
                .escalation_state
                .write()
                .await;

        state.insert(
            agent_id.to_string(),
            escalation,
        );
    }

    pub async fn clear_escalation(
        &self,

        agent_id: &str,
    ) {
        let mut state =
            self
                .escalation_state
                .write()
                .await;

        state.remove(
            agent_id
        );
    }

    pub async fn append_audit(
        &self,

        event:
            AuditEvent,
    ) {
        let mut buffer =
            self
                .audit_buffer
                .write()
                .await;

        buffer.push_back(
            event
        );

        if buffer.len() > 10_000 {
            buffer.pop_front();
        }
    }

    pub async fn audit_snapshot(
        &self,
    ) -> Vec<AuditEvent> {
        let buffer =
            self
                .audit_buffer
                .read()
                .await;

        buffer
            .iter()
            .cloned()
            .collect()
    }

    pub async fn decay_trust(
        &self,

        agent_id: &str,

        amount: f32,
    ) {
        let mut map =
            self
                .trust_map
                .write()
                .await;

        if let Some(state) =
            map.get_mut(agent_id)
        {
            state.current_score =
                (
                    state.current_score
                        - amount
                )
                    .clamp(
                        0.0,
                        1.0,
                    );

            if state.current_score
                < 0.20
            {
                state
                    .compromise_suspected =
                    true;
            }
        }
    }

    pub async fn recover_trust(
        &self,

        agent_id: &str,

        amount: f32,
    ) {
        let mut map =
            self
                .trust_map
                .write()
                .await;

        if let Some(state) =
            map.get_mut(agent_id)
        {
            state.current_score =
                (
                    state.current_score
                        + amount
                )
                    .clamp(
                        0.0,
                        1.0,
                    );
        }
    }

    pub async fn runtime_health(
        &self,
    ) -> f32 {
        let map =
            self
                .trust_map
                .read()
                .await;

        if map.is_empty() {
            return 1.0;
        }

        let total: f32 =
            map.values()
                .map(
                    |s| {
                        s.current_score
                    }
                )
                .sum();

        total
            / map.len()
                as f32
    }

    pub async fn compromised_agents(
        &self,
    ) -> Vec<String> {
        let map =
            self
                .trust_map
                .read()
                .await;

        map.iter()
            .filter(
                |(_, trust)| {
                    trust
                        .compromise_suspected
                },
            )
            .map(
                |(id, _)| {
                    id.clone()
                },
            )
            .collect()
    }

    pub fn monotonic_timestamp()
        -> i64
    {
        Utc::now()
            .timestamp_millis()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::hal_types::{
        EscalationLevel,
    };

    #[tokio::test]
    async fn trust_decay() {
        let runtime =
            RuntimeState::new();

        runtime
            .set_trust_state(
                "planner",

                TrustState {
                    current_score:
                        0.90,

                    historical_average:
                        0.92,

                    decay_rate:
                        0.02,

                    recovery_rate:
                        0.01,

                    compromise_suspected:
                        false,
                },
            )
            .await;

        runtime
            .decay_trust(
                "planner",
                0.50,
            )
            .await;

        let trust =
            runtime
                .get_trust_state(
                    "planner"
                )
                .await
                .unwrap();

        assert!(
            trust.current_score
                < 0.90
        );
    }

    #[tokio::test]
    async fn escalation_tracking() {
        let runtime =
            RuntimeState::new();

        runtime
            .set_escalation(
                "planner",

                EscalationContext {
                    level:
                        EscalationLevel::High,

                    reason:
                        "suspicious"
                            .into(),

                    requires_isolation:
                        true,

                    requires_forensics:
                        true,
                },
            )
            .await;

        assert!(
            runtime
                .escalation_active(
                    "planner"
                )
                .await
        );
    }
}