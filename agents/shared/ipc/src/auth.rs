use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair, UnparsedPublicKey, ED25519};

use crate::envelope::{payload_hash, verify_timestamp};
use crate::proto;

/// Manages Ed25519 signing and verification for envelope integrity.
pub struct AuthManager {
    keypair: Ed25519KeyPair,
    nonce_cache: Arc<DashMap<String, Instant>>,
}

impl AuthManager {
    pub fn generate() -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
            .expect("failed to generate keypair");
        let keypair =
            Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).expect("invalid pkcs8");

        Self {
            keypair,
            nonce_cache: Arc::new(DashMap::new()),
        }
    }

    pub fn from_pkcs8(pkcs8_bytes: &[u8]) -> anyhow::Result<Self> {
        let keypair = Ed25519KeyPair::from_pkcs8(pkcs8_bytes)
            .map_err(|e| anyhow::anyhow!("invalid pkcs8: {}", e))?;
        Ok(Self {
            keypair,
            nonce_cache: Arc::new(DashMap::new()),
        })
    }

    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.keypair.public_key().as_ref().to_vec()
    }

    pub fn sign_envelope(&self, envelope: &mut proto::IntentEnvelope) {
        let hash = payload_hash(envelope);
        let signature = self.keypair.sign(hash.as_bytes());

        envelope.signature = Some(proto::Signature {
            algorithm: "ed25519".into(),
            public_key: self.public_key_bytes(),
            signature_bytes: signature.as_ref().to_vec(),
        });
    }

    pub fn verify_envelope(envelope: &proto::IntentEnvelope) -> bool {
        let sig = match &envelope.signature {
            Some(sig) => sig,
            None => return false,
        };

        if sig.algorithm != "ed25519" {
            return false;
        }

        let hash = payload_hash(envelope);
        let public_key = UnparsedPublicKey::new(&ED25519, &sig.public_key);
        public_key.verify(hash.as_bytes(), &sig.signature_bytes).is_ok()
    }

    /// Full integrity check: signature + nonce dedup + replay window.
    pub fn validate_integrity(&self, envelope: &proto::IntentEnvelope) -> Result<(), String> {
        if !Self::verify_envelope(envelope) {
            return Err("signature verification failed".into());
        }

        if envelope.nonce.is_empty() || envelope.nonce.len() <= 10 {
            return Err("invalid nonce".into());
        }

        // Nonce deduplication: reject if we've seen this nonce within the replay window
        if self.nonce_cache.contains_key(&envelope.nonce) {
            return Err("duplicate nonce (replay detected)".into());
        }

        if !verify_timestamp(envelope, 30_000) {
            return Err("replay window exceeded".into());
        }

        // Record nonce to prevent replay
        self.nonce_cache.insert(envelope.nonce.clone(), Instant::now());

        // Evict expired nonces older than 60s
        self.nonce_cache.retain(|_, ts| ts.elapsed() < Duration::from_secs(60));

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::build_envelope;

    #[test]
    fn sign_and_verify() {
        let auth = AuthManager::generate();
        let mut env = build_envelope(
            "intent-1".into(),
            "planner".into(),
            "memory".into(),
            "{}".into(),
        );
        auth.sign_envelope(&mut env);
        assert!(AuthManager::verify_envelope(&env));
    }

    #[test]
    fn tampered_payload_fails() {
        let auth = AuthManager::generate();
        let mut env = build_envelope(
            "intent-1".into(),
            "planner".into(),
            "memory".into(),
            "{}".into(),
        );
        auth.sign_envelope(&mut env);
        env.intent_payload_json = r#"{"malicious":true}"#.into();
        assert!(!AuthManager::verify_envelope(&env));
    }

    #[test]
    fn replay_detection_works() {
        let auth = AuthManager::generate();
        let mut env = build_envelope(
            "intent-1".into(),
            "planner".into(),
            "memory".into(),
            "{}".into(),
        );
        auth.sign_envelope(&mut env);

        assert!(auth.validate_integrity(&env).is_ok());
        assert!(auth.validate_integrity(&env).is_err());
    }
}
