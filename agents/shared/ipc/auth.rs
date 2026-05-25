use ring::{
    rand::SystemRandom,
    signature::{
        Ed25519KeyPair,
        KeyPair,
        UnparsedPublicKey,
        ED25519,
    },
};

use base64::{
    engine::general_purpose,
    Engine as _,
};

use crate::envelope::{
    EnvelopeSignature,
    IntentEnvelope,
};

pub struct AuthManager {
    keypair: Ed25519KeyPair,
}

impl AuthManager {
    pub fn generate() -> Self {
        let rng = SystemRandom::new();

        let pkcs8 =
            Ed25519KeyPair::generate_pkcs8(&rng)
                .expect("failed to generate keypair");

        let keypair =
            Ed25519KeyPair::from_pkcs8(
                pkcs8.as_ref()
            )
            .expect("invalid pkcs8");

        Self { keypair }
    }

    pub fn public_key_bytes(&self) -> Vec<u8> {
        self.keypair
            .public_key()
            .as_ref()
            .to_vec()
    }

    pub fn public_key_base64(&self) -> String {
        general_purpose::STANDARD.encode(
            self.public_key_bytes()
        )
    }

    pub fn sign_envelope(
        &self,
        envelope: &mut IntentEnvelope,
    ) {
        let payload_hash =
            envelope.payload_hash();

        let signature =
            self.keypair.sign(
                payload_hash.as_bytes()
            );

        envelope.attach_signature(
            "ed25519".into(),

            self.public_key_bytes(),

            signature.as_ref().to_vec(),
        );
    }

    pub fn verify_envelope(
        envelope: &IntentEnvelope,
    ) -> bool {
        let sig =
            match &envelope.signature {
                Some(sig) => sig,
                None => return false,
            };

        if sig.algorithm != "ed25519" {
            return false;
        }

        let payload_hash =
            envelope.payload_hash();

        let public_key =
            UnparsedPublicKey::new(
                &ED25519,
                &sig.public_key,
            );

        public_key
            .verify(
                payload_hash.as_bytes(),
                &sig.signature_bytes,
            )
            .is_ok()
    }

    pub fn export_signature(
        envelope: &IntentEnvelope,
    ) -> Option<String> {
        envelope.signature.as_ref().map(
            |sig| {
                general_purpose::STANDARD.encode(
                    &sig.signature_bytes
                )
            },
        )
    }

    pub fn validate_nonce(
        envelope: &IntentEnvelope,
    ) -> bool {
        !envelope.nonce.is_empty()
            && envelope.nonce.len() > 10
    }

    pub fn validate_replay_window(
        envelope: &IntentEnvelope,
        max_skew_ms: i64,
    ) -> bool {
        envelope.verify_timestamp(max_skew_ms)
    }

    pub fn validate_integrity(
        envelope: &IntentEnvelope,
    ) -> Result<(), String> {
        if !Self::verify_envelope(envelope) {
            return Err(
                "signature verification failed"
                    .into(),
            );
        }

        if !Self::validate_nonce(envelope) {
            return Err(
                "invalid nonce".into()
            );
        }

        if !Self::validate_replay_window(
            envelope,
            30_000,
        ) {
            return Err(
                "replay window exceeded"
                    .into(),
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_and_verify() {
        let auth =
            AuthManager::generate();

        let mut env =
            IntentEnvelope::new(
                "intent-1".into(),
                "planner".into(),
                "memory".into(),
                "{}".into(),
            );

        auth.sign_envelope(&mut env);

        assert!(
            AuthManager::verify_envelope(
                &env
            )
        );
    }

    #[test]
    fn tampered_payload_fails() {
        let auth =
            AuthManager::generate();

        let mut env =
            IntentEnvelope::new(
                "intent-1".into(),
                "planner".into(),
                "memory".into(),
                "{}".into(),
            );

        auth.sign_envelope(&mut env);

        env.intent_payload_json =
            "{\"malicious\":true}".into();

        assert!(
            !AuthManager::verify_envelope(
                &env
            )
        );
    }

    #[test]
    fn replay_validation_works() {
        let auth =
            AuthManager::generate();

        let mut env =
            IntentEnvelope::new(
                "intent-1".into(),
                "planner".into(),
                "memory".into(),
                "{}".into(),
            );

        auth.sign_envelope(&mut env);

        assert!(
            AuthManager::validate_replay_window(
                &env,
                30_000,
            )
        );
    }
}