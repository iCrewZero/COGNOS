//! HAL Provenance — tracks where code came from and verifies signatures.
//!

use sha2::{
    Digest,
    Sha256,
};

use chrono::Utc;

use crate::hal_types::{
    ProvenanceConfidence,
    ProvenanceData,
};

pub struct ProvenanceEngine;

impl ProvenanceEngine {
    pub fn verify_signature(
        signature_verified: bool,
    ) -> ProvenanceConfidence {
        if signature_verified {
            ProvenanceConfidence::Verified
        } else {
            ProvenanceConfidence::Forged
        }
    }

    pub fn compute_chain_hash(
        parent_hash: &str,

        payload_hash: &str,

        timestamp: i64,
    ) -> String {
        let mut hasher =
            Sha256::new();

        hasher.update(
            parent_hash.as_bytes()
        );

        hasher.update(
            payload_hash.as_bytes()
        );

        hasher.update(
            timestamp
                .to_le_bytes()
        );

        hex::encode(
            hasher.finalize()
        )
    }

    pub fn validate_chain(
        expected_hash: &str,

        parent_hash: &str,

        payload_hash: &str,

        timestamp: i64,
    ) -> bool {
        let computed =
            Self::compute_chain_hash(
                parent_hash,
                payload_hash,
                timestamp,
            );

        computed == expected_hash
    }

    pub fn replay_window_valid(
        timestamp: i64,

        max_skew_ms: i64,
    ) -> bool {
        let now =
            Utc::now()
                .timestamp_millis();

        let delta =
            (
                now - timestamp
            )
                .abs();

        delta <= max_skew_ms
    }

    pub fn provenance_score(
        provenance:
            &ProvenanceData,
    ) -> f32 {
        let mut score: f32 = 0.0;

        if provenance
            .signature_verified
        {
            score += 0.40;
        }

        if provenance
            .replay_checked
        {
            score += 0.25;
        }

        if !provenance
            .certificate_fingerprint
            .is_empty()
        {
            score += 0.20;
        }

        if !provenance
            .trust_chain_hash
            .is_empty()
        {
            score += 0.15;
        }

        score.clamp(0.0, 1.0)
    }

    pub fn detect_forgery(
        provenance:
            &ProvenanceData,
    ) -> bool {
        !provenance
            .signature_verified
            || provenance
                .confidence
                == ProvenanceConfidence::Forged
    }

    pub fn trust_lineage_depth(
        chain: &[String],
    ) -> usize {
        chain.len()
    }

    pub fn provenance_decay(
        age_ms: i64,
    ) -> f32 {
        let hours =
            age_ms as f32
                / 3_600_000.0;

        (
            1.0
                - (
                    hours
                        * 0.015
                )
        )
            .clamp(0.0, 1.0)
    }

    pub fn verify_authority_continuity(
        previous_agent: &str,

        current_agent: &str,

        capability_chain:
            &[String],
    ) -> bool {
        if previous_agent
            == current_agent
        {
            return true;
        }

        capability_chain
            .iter()
            .any(
                |cap| {
                    cap.contains(
                        "delegate"
                    )
                },
            )
    }

    pub fn detect_chain_break(
        parent_hash: &str,

        current_parent:
            &str,
    ) -> bool {
        parent_hash
            != current_parent
    }

    pub fn confidence_from_lineage(
        lineage_depth: usize,
    ) -> f32 {
        let depth =
            lineage_depth
                as f32;

        (
            1.0
                - (
                    depth
                        * 0.03
                )
        )
            .clamp(0.0, 1.0)
    }

    pub fn signed_origin_valid(
        provenance:
            &ProvenanceData,
    ) -> bool {
        provenance
            .signature_verified
            && !provenance
                .certificate_fingerprint
                .is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_hash_consistent() {
        let h1 =
            ProvenanceEngine::
                compute_chain_hash(
                    "parent",
                    "payload",
                    100,
                );

        let h2 =
            ProvenanceEngine::
                compute_chain_hash(
                    "parent",
                    "payload",
                    100,
                );

        assert_eq!(
            h1,
            h2
        );
    }

    #[test]
    fn replay_window_valid() {
        let now =
            Utc::now()
                .timestamp_millis();

        assert!(
            ProvenanceEngine::
                replay_window_valid(
                    now,
                    30_000,
                )
        );
    }

    #[test]
    fn detect_forged() {
        let provenance =
            ProvenanceData {
                source_agent:
                    "planner"
                        .into(),

                certificate_fingerprint:
                    "".into(),

                trust_chain_hash:
                    "".into(),

                signature_verified:
                    false,

                replay_checked:
                    false,

                confidence:
                    ProvenanceConfidence::Forged,
            };

        assert!(
            ProvenanceEngine::
                detect_forgery(
                    &provenance
                )
        );
    }

    #[test]
    fn provenance_score_high() {
        let provenance =
            ProvenanceData {
                source_agent:
                    "planner"
                        .into(),

                certificate_fingerprint:
                    "abc".into(),

                trust_chain_hash:
                    "def".into(),

                signature_verified:
                    true,

                replay_checked:
                    true,

                confidence:
                    ProvenanceConfidence::Verified,
            };

        let score =
            ProvenanceEngine::
                provenance_score(
                    &provenance
                );

        assert!(
            score > 0.8
        );
    }
}