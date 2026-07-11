//! Process-local pipeline metrics — HAL outcomes, parser cache/fallback, latency.
//!
//! Each daemon (HAL, intent-engine, orchestrator) updates the counters it owns;
//! `cognos status` merges snapshots from each endpoint.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use crate::proto::v1::PipelineMetrics;

/// Global metrics registry for the current process.
pub static METRICS: MetricsRegistry = MetricsRegistry::new();

/// Atomic counters and last-latency sample for pipeline observability.
pub struct MetricsRegistry {
    hal_granted: AtomicU64,
    hal_denied: AtomicU64,
    hal_approval_required: AtomicU64,
    parser_cache_hits: AtomicU64,
    parser_cache_misses: AtomicU64,
    parser_fallback_uses: AtomicU64,
    intent_requests: AtomicU64,
    last_total_ms: AtomicU64,
    last_parse_ms: AtomicU64,
    last_orchestrate_ms: AtomicU64,
    last_execute_ms: AtomicU64,
    last_trace_id: Mutex<String>,
}

impl MetricsRegistry {
    pub const fn new() -> Self {
        Self {
            hal_granted: AtomicU64::new(0),
            hal_denied: AtomicU64::new(0),
            hal_approval_required: AtomicU64::new(0),
            parser_cache_hits: AtomicU64::new(0),
            parser_cache_misses: AtomicU64::new(0),
            parser_fallback_uses: AtomicU64::new(0),
            intent_requests: AtomicU64::new(0),
            last_total_ms: AtomicU64::new(0),
            last_parse_ms: AtomicU64::new(0),
            last_orchestrate_ms: AtomicU64::new(0),
            last_execute_ms: AtomicU64::new(0),
            last_trace_id: Mutex::new(String::new()),
        }
    }

    pub fn record_hal_granted(&self) {
        self.hal_granted.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_hal_denied(&self) {
        self.hal_denied.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_hal_approval_required(&self) {
        self.hal_approval_required.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_hal_status(&self, status: &str) {
        match status {
            "granted" => self.record_hal_granted(),
            "denied" | "failed" => self.record_hal_denied(),
            "approval_required" => self.record_hal_approval_required(),
            _ => {}
        }
    }

    pub fn record_parser_cache_hit(&self) {
        self.parser_cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_parser_cache_miss(&self) {
        self.parser_cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_parser_fallback(&self) {
        self.parser_fallback_uses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_intent_request(&self) {
        self.intent_requests.fetch_add(1, Ordering::Relaxed);
    }

    /// Record the last end-to-end latency sample for a trace.
    pub fn record_latency(
        &self,
        trace_id: &str,
        parse_ms: u64,
        orchestrate_ms: u64,
        execute_ms: u64,
    ) {
        let total = parse_ms.saturating_add(orchestrate_ms).saturating_add(execute_ms);
        self.last_parse_ms.store(parse_ms, Ordering::Relaxed);
        self.last_orchestrate_ms
            .store(orchestrate_ms, Ordering::Relaxed);
        self.last_execute_ms.store(execute_ms, Ordering::Relaxed);
        self.last_total_ms.store(total, Ordering::Relaxed);
        if let Ok(mut id) = self.last_trace_id.lock() {
            *id = trace_id.to_string();
        }
    }

    /// Snapshot counters for `GetPipelineMetrics`.
    pub fn snapshot(&self) -> PipelineMetrics {
        let last_trace_id = self
            .last_trace_id
            .lock()
            .map(|s| s.clone())
            .unwrap_or_default();
        PipelineMetrics {
            hal_granted: self.hal_granted.load(Ordering::Relaxed),
            hal_denied: self.hal_denied.load(Ordering::Relaxed),
            hal_approval_required: self.hal_approval_required.load(Ordering::Relaxed),
            parser_cache_hits: self.parser_cache_hits.load(Ordering::Relaxed),
            parser_cache_misses: self.parser_cache_misses.load(Ordering::Relaxed),
            parser_fallback_uses: self.parser_fallback_uses.load(Ordering::Relaxed),
            intent_requests: self.intent_requests.load(Ordering::Relaxed),
            last_total_latency_ms: self.last_total_ms.load(Ordering::Relaxed) as f64,
            last_parse_latency_ms: self.last_parse_ms.load(Ordering::Relaxed) as f64,
            last_orchestrate_latency_ms: self.last_orchestrate_ms.load(Ordering::Relaxed) as f64,
            last_execute_latency_ms: self.last_execute_ms.load(Ordering::Relaxed) as f64,
            last_trace_id,
        }
    }
}

/// Emit a structured tracing event for one pipeline stage (grep by trace_id).
pub fn log_stage(trace_id: &str, stage: &str, latency_ms: u64) {
    tracing::info!(
        trace_id = %trace_id,
        stage = %stage,
        latency_ms = latency_ms,
        "pipeline stage"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hal_status_routing() {
        let m = MetricsRegistry::new();
        m.record_hal_status("granted");
        m.record_hal_status("approval_required");
        m.record_hal_status("denied");
        let s = m.snapshot();
        assert_eq!(s.hal_granted, 1);
        assert_eq!(s.hal_approval_required, 1);
        assert_eq!(s.hal_denied, 1);
    }
}
