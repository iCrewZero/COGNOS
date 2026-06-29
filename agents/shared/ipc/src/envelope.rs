use chrono::Utc;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::proto;

/// Builds a fully-formed IntentEnvelope proto message ready for signing and dispatch.
pub fn build_envelope(
    intent_id: String,
    source_agent: String,
    target_agent: String,
    payload_json: String,
) -> proto::IntentEnvelope {
    let timestamp = Utc::now().timestamp_millis();
    let nonce = Uuid::new_v4().to_string();
    let audit_id = Uuid::new_v4().to_string();

    let event_hash = compute_hash(
        format!("{}:{}:{}:{}", &intent_id, &source_agent, &target_agent, &timestamp).as_bytes(),
    );

    proto::IntentEnvelope {
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
        audit: Some(proto::AuditContext {
            audit_id,
            parent_hash: String::new(),
            event_hash,
            originating_host: hostname::get()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            originating_process: std::process::id().to_string(),
            created_at: timestamp,
        }),
        signature: None,
    }
}

pub fn compute_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

/// Computes a deterministic hash of the envelope's critical fields for signing.
pub fn payload_hash(envelope: &proto::IntentEnvelope) -> String {
    let mut hasher = Sha256::new();
    hasher.update(envelope.intent_id.as_bytes());
    hasher.update(envelope.source_agent.as_bytes());
    hasher.update(envelope.target_agent.as_bytes());
    hasher.update(envelope.intent_payload_json.as_bytes());
    hasher.update(envelope.timestamp_unix_ms.to_le_bytes());
    hasher.update(envelope.nonce.as_bytes());
    hasher.update(envelope.session_id.as_bytes());
    hex::encode(hasher.finalize())
}

/// Returns true if the envelope timestamp is within `max_skew_ms` of now.
pub fn verify_timestamp(envelope: &proto::IntentEnvelope, max_skew_ms: i64) -> bool {
    let now = Utc::now().timestamp_millis();
    let delta = (now - envelope.timestamp_unix_ms).abs();
    delta <= max_skew_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_generates_hash() {
        let env = build_envelope(
            "intent-1".into(),
            "planner".into(),
            "memory".into(),
            r#"{"goal":"open_workspace"}"#.into(),
        );
        let hash = payload_hash(&env);
        assert!(!hash.is_empty());
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn timestamp_validation_works() {
        let env = build_envelope(
            "intent-1".into(),
            "planner".into(),
            "memory".into(),
            "{}".into(),
        );
        assert!(verify_timestamp(&env, 5000));
    }
}
