use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tonic::{Request, Status};

/// Metadata key for the authenticated agent identity (set by the auth interceptor).
pub const AGENT_ID_KEY: &str = "x-cognos-agent-id";
pub const SESSION_TOKEN_KEY: &str = "x-cognos-session-token";
pub const TRACE_ID_KEY: &str = "x-cognos-trace-id";

/// Server-side interceptor that validates the session token in gRPC metadata.
#[derive(Clone)]
pub struct AuthInterceptor {
    valid_tokens: Arc<DashMap<String, TokenEntry>>,
}

#[derive(Clone)]
struct TokenEntry {
    agent_id: String,
    expires_at: Instant,
}

impl AuthInterceptor {
    pub fn new() -> Self {
        Self {
            valid_tokens: Arc::new(DashMap::new()),
        }
    }

    /// Registers a session token for an agent (called after successful TLS auth).
    pub fn register_token(&self, token: &str, agent_id: &str, ttl: Duration) {
        self.valid_tokens.insert(
            token.to_string(),
            TokenEntry {
                agent_id: agent_id.to_string(),
                expires_at: Instant::now() + ttl,
            },
        );
    }

    /// Revokes a session token.
    pub fn revoke_token(&self, token: &str) {
        self.valid_tokens.remove(token);
    }

    /// Tonic interceptor function: validates session token from metadata.
    pub fn intercept(&self, req: Request<()>) -> Result<Request<()>, Status> {
        let token = req
            .metadata()
            .get(SESSION_TOKEN_KEY)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| Status::unauthenticated("missing session token"))?;

        let entry = self
            .valid_tokens
            .get(token)
            .ok_or_else(|| Status::unauthenticated("invalid session token"))?;

        if Instant::now() > entry.expires_at {
            self.valid_tokens.remove(token);
            return Err(Status::unauthenticated("session token expired"));
        }

        Ok(req)
    }

    /// Extracts the agent ID from a validated request's metadata.
    pub fn agent_id_from_request<T>(req: &Request<T>) -> Option<String> {
        req.metadata()
            .get(AGENT_ID_KEY)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    }
}

impl Default for AuthInterceptor {
    fn default() -> Self {
        Self::new()
    }
}

/// Rate limiter per-agent: rejects requests exceeding the configured RPS.
#[derive(Clone)]
pub struct RateLimiter {
    limits: Arc<DashMap<String, RateWindow>>,
    max_per_second: u32,
}

struct RateWindow {
    count: u32,
    window_start: Instant,
}

impl RateLimiter {
    pub fn new(max_per_second: u32) -> Self {
        Self {
            limits: Arc::new(DashMap::new()),
            max_per_second,
        }
    }

    /// Returns Ok(()) if the request is allowed, Err(Status) if rate-limited.
    pub fn check(&self, agent_id: &str) -> Result<(), Status> {
        let now = Instant::now();

        let mut entry = self.limits.entry(agent_id.to_string()).or_insert(RateWindow {
            count: 0,
            window_start: now,
        });

        if now.duration_since(entry.window_start) >= Duration::from_secs(1) {
            entry.count = 0;
            entry.window_start = now;
        }

        entry.count += 1;

        if entry.count > self.max_per_second {
            return Err(Status::resource_exhausted(format!(
                "rate limit exceeded for agent {}",
                agent_id
            )));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_interceptor_rejects_missing_token() {
        let interceptor = AuthInterceptor::new();
        let req = Request::new(());
        let result = interceptor.intercept(req);
        assert!(result.is_err());
    }

    #[test]
    fn rate_limiter_enforces_limit() {
        let limiter = RateLimiter::new(5);
        for _ in 0..5 {
            assert!(limiter.check("test-agent").is_ok());
        }
        assert!(limiter.check("test-agent").is_err());
    }
}
