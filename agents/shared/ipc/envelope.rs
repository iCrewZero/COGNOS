use chrono::Utc;
use ring::digest::{digest, SHA256};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntentEnvelope {
    pub envelope_id: String,

    pub intent_id: String,

    pub source_agent: String,
    pub target_agent: String,

    pub capability_token: String,

    pub action_graph_hash: String,

    pub timestamp_unix_ms: i64,

    pub nonce: String,

    pub session_id: String,
    pub user_id: String,

    pub intent_type: String,
    pub intent_payload_json: String,

    pub risk_estimate: f32,
    pub trust_score: f32,

    pub requires_hal: bool,

    pub requested_capabilities: Vec<String>,

    pub audit: AuditContext,

    pub signature: Option<EnvelopeSignature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditContext {
    pub audit_id: String,

    pub parent_hash: String,

    pub event_hash: String,

    pub originating_host: String,
    pub originating_process: String,

    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvelopeSignature {
    pub algorithm: String,

    pub public_key: Vec<u8>,

    pub signature_bytes: Vec<u8>,
}

impl IntentEnvelope {
    pub fn new(
        intent_id: String,
        source_agent: String,
        target_agent: String,
        payload_json: String,
    ) -> Self {
        let timestamp = Utc::now().timestamp_millis();

        let nonce = Uuid::new_v4().to_string();

        let audit_id = Uuid::new_v4().to_string();

        let event_hash = Self::compute_hash(
            format!(
                "{}:{}:{}:{}",
                &intent_id,
                &source_agent,
                &target_agent,
                &timestamp
            )
            .as_bytes(),
        );

        Self {
            envelope_id: Uuid::new_v4().to_string(),

            intent_id,

            source_agent,

            target_agent,

            capability_token: String::new(),

            action_graph_hash: String::new(),

            timestamp_unix_ms: timestamp,

            nonce,

            session_id: String::new(),

            user_id: String::new(),

            intent_type: "unknown".into(),

            intent_payload_json: payload_json,

            risk_estimate: 0.0,

            trust_score: 0.0,

            requires_hal: false,

            requested_capabilities: vec![],

            audit: AuditContext {
                audit_id,

                parent_hash: String::new(),

                event_hash,

                originating_host: hostname::get()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),

                originating_process: std::process::id().to_string(),

                created_at: timestamp,
            },

            signature: None,
        }
    }

    pub fn compute_hash(data: &[u8]) -> String {
        let hash = digest(&SHA256, data);

        hex::encode(hash.as_ref())
    }

    pub fn payload_hash(&self) -> String {
        let mut hasher = Sha256::new();

        hasher.update(self.intent_id.as_bytes());

        hasher.update(self.source_agent.as_bytes());

        hasher.update(self.target_agent.as_bytes());

        hasher.update(self.intent_payload_json.as_bytes());

        hasher.update(self.timestamp_unix_ms.to_le_bytes());

        hasher.update(self.nonce.as_bytes());

        hex::encode(hasher.finalize())
    }

    pub fn attach_signature(
        &mut self,
        algorithm: String,
        public_key: Vec<u8>,
        signature_bytes: Vec<u8>,
    ) {
        self.signature = Some(
            EnvelopeSignature {
                algorithm,
                public_key,
                signature_bytes,
            }
        );
    }

    pub fn verify_timestamp(
        &self,
        max_skew_ms: i64,
    ) -> bool {
        let now = Utc::now().timestamp_millis();

        let delta = (now - self.timestamp_unix_ms).abs();

        delta <= max_skew_ms
    }

    pub fn requires_capability(
        &self,
        capability: &str,
    ) -> bool {
        self.requested_capabilities
            .iter()
            .any(|c| c == capability)
    }

    pub fn audit_chain_valid(&self) -> bool {
        !self.audit.audit_id.is_empty()
            && !self.audit.event_hash.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_generates_hash() {
        let env = IntentEnvelope::new(
            "intent-1".into(),
            "planner".into(),
            "memory".into(),
            "{\"goal\":\"open_workspace\"}".into(),
        );

        let hash = env.payload_hash();

        assert!(!hash.is_empty());
    }

    #[test]
    fn timestamp_validation_works() {
        let env = IntentEnvelope::new(
            "intent-1".into(),
            "planner".into(),
            "memory".into(),
            "{}".into(),
        );

        assert!(env.verify_timestamp(5000));
    }

    #[test]
    fn capability_detection_works() {
        let mut env = IntentEnvelope::new(
            "intent-1".into(),
            "planner".into(),
            "file".into(),
            "{}".into(),
        );

        env.requested_capabilities
            .push("filesystem.read".into());

        assert!(
            env.requires_capability("filesystem.read")
        );
    }
}