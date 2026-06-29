//! COGNOS IPC authentication and capability enforcement.
//!
//! Every inbound RPC carries a signed Envelope whose capability field
//! declares the privilege it needs. This module verifies tokens and
//! checks capabilities.

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use base64::Engine;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, warn};

type HmacSha256 = Hmac<Sha256>;

// ─── Errors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
pub enum AuthError {
    #[error("invalid token")]
    InvalidToken,
    #[error("token expired")]
    Expired,
    #[error("unknown agent: {0}")]
    UnknownAgent(String),
    #[error("signature mismatch")]
    SignatureMismatch,
}

// ─── Capability violations ───────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityViolation {
    pub required: String,
    pub held: String,
    pub reason: String,
    pub message: String,
    pub agent_id: String,
    pub trace_id: String,
}

impl std::fmt::Display for CapabilityViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "capability violation: required={}, held={}, reason={}, agent={}",
            self.required, self.held, self.reason, self.agent_id
        )
    }
}

impl std::error::Error for CapabilityViolation {}

// ─── AuthContext ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthContext {
    pub agent_id: String,
    pub capabilities: HashSet<String>,
    pub session_token: String,
    pub expiry: u64,
}

impl AuthContext {
    /// Check whether this context holds a given capability.
    /// Currently uses exact string match. A future version will walk
    /// the capability lattice so that e.g. `fs.hal` implies `fs.read`.
    pub fn has_capability(&self, cap: &str) -> bool {
        self.capabilities.contains(cap)
    }
}

// ─── Token creation ──────────────────────────────────────────────────────────

/// Create an HMAC-SHA256 session token.
///
/// Format: `base64(agent_id).base64(expiry).base64(hmac_signature)`
/// The HMAC is computed over `agent_id|expiry` using `secret`.
pub fn create_token(
    agent_id: &str,
    expiry: u64,
    secret: &[u8],
) -> String {
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let agent_b64 = engine.encode(agent_id.as_bytes());
    let expiry_b64 = engine.encode(expiry.to_string().as_bytes());

    let mut mac = HmacSha256::new_from_slice(secret)
        .expect("HMAC key length is always valid for SHA-256");
    mac.update(agent_id.as_bytes());
    mac.update(b"|");
    mac.update(expiry.to_string().as_bytes());
    let sig = mac.finalize().into_bytes();

    let sig_b64 = engine.encode(&sig);
    format!("{agent_b64}.{expiry_b64}.{sig_b64}")
}

// ─── Token verification ──────────────────────────────────────────────────────

/// Verify an HMAC-SHA256 session token and return the authenticated context.
///
/// Token format: `base64(agent_id).base64(expiry).base64(hmac)`
/// The HMAC is recomputed over `agent_id|expiry` using the server secret
/// and compared in constant time.
pub fn verify_token(
    token: &str,
    expected_agent: &str,
    secret: &[u8],
) -> Result<AuthContext, AuthError> {
    debug!(agent = expected_agent, "verifying session token");

    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        warn!("token did not have 3 dot-separated parts");
        return Err(AuthError::InvalidToken);
    }

    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;

    let agent_bytes = engine.decode(parts[0]).map_err(|_| AuthError::InvalidToken)?;
    let agent_id = String::from_utf8(agent_bytes).map_err(|_| AuthError::InvalidToken)?;

    let expiry_bytes = engine.decode(parts[1]).map_err(|_| AuthError::InvalidToken)?;
    let expiry_str = String::from_utf8(expiry_bytes).map_err(|_| AuthError::InvalidToken)?;

    let supplied_sig = engine.decode(parts[2]).map_err(|_| AuthError::InvalidToken)?;

    if agent_id != expected_agent {
        // Always check the signature even on agent mismatch to avoid
        // leaking which agents exist. Use a dummy agent for the check.
        let dummy_expiry: u64 = expiry_str.parse().unwrap_or(0);
        let mut mac = HmacSha256::new_from_slice(secret).unwrap_or_else(|_| {
            HmacSha256::new_from_slice(b"fallback-key").unwrap()
        });
        mac.update(b"dummy|");
        mac.update(dummy_expiry.to_string().as_bytes());
        let _ = mac.verify(supplied_sig.as_slice());

        return Err(AuthError::UnknownAgent(agent_id));
    }

    let expiry: u64 = expiry_str.parse().map_err(|_| AuthError::InvalidToken)?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if expiry <= now {
        warn!(expiry, now, "token expired");
        return Err(AuthError::Expired);
    }

    // Real HMAC-SHA256 verification using the `hmac` crate.
    let mut mac = HmacSha256::new_from_slice(secret)
        .expect("HMAC key length is always valid for SHA-256");
    mac.update(agent_id.as_bytes());
    mac.update(b"|");
    mac.update(expiry.to_string().as_bytes());

    mac.verify_slice(&supplied_sig)
        .map_err(|_| {
            warn!("HMAC signature mismatch");
            AuthError::SignatureMismatch
        })?;

    // In production, look up the agent's capability set from the registry.
    // For now, start with an empty set — the caller should populate it
    // after successful verification.
    let capabilities: HashSet<String> = HashSet::new();

    Ok(AuthContext {
        agent_id,
        capabilities,
        session_token: token.to_string(),
        expiry,
    })
}

// ─── Capability enforcement ──────────────────────────────────────────────────

/// Check that `ctx` holds `required_cap`. Returns a CapabilityViolation
/// on failure so the caller can wrap it into the wire response.
pub fn enforce_capability(
    ctx: &AuthContext,
    required_cap: &str,
) -> Result<(), CapabilityViolation> {
    if ctx.has_capability(required_cap) {
        debug!(agent = %ctx.agent_id, cap = required_cap, "capability granted");
        return Ok(());
    }

    let held = ctx
        .capabilities
        .iter()
        .max_by_key(|c| c.len())
        .cloned()
        .unwrap_or_default();

    warn!(
        agent = %ctx.agent_id,
        required = required_cap,
        held = %held,
        "capability denied"
    );

    Err(CapabilityViolation {
        required: required_cap.to_string(),
        held,
        reason: "missing".to_string(),
        message: format!(
            "agent `{}` lacks required capability `{}`",
            ctx.agent_id, required_cap
        ),
        agent_id: ctx.agent_id.clone(),
        trace_id: String::new(),
    })
}