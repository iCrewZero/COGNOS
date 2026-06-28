//! Behavioral model — per-agent behavior tracking feeding HAL risk inputs.
//!
//!
//! Produces [`BehavioralMetrics`] consumed by the confidence engine and the
//! risk model's TrustContext / TimeAnomaly components (docs/SPEC.md).
//! History is a bounded sliding window per agent — no unbounded growth.
//!
//! Unknown agents score neutral-conservative (0.5), never trusted-by-default.

use std::collections::{HashMap, VecDeque};

use crate::hal_types::BehavioralMetrics;

/// Maximum observations retained per agent.
const WINDOW: usize = 200;
/// Number of most-recent observations considered "recent".
const RECENT: usize = 20;
/// Fraction of activity in an hour bucket for it to count as an
/// established time pattern (TimeAnomaly = 0.0 per spec).
const ESTABLISHED_HOUR_RATIO: f32 = 0.05;

/// One observed agent action outcome.
#[derive(Debug, Clone)]
pub struct ActionObservation {
    /// Action verb, e.g. "open_files".
    pub action: String,
    pub succeeded: bool,
    /// True when the agent attempted something outside its capability lattice.
    pub was_escalation_attempt: bool,
    /// Hour of day, 0–23, when the action was observed.
    pub hour: u8,
}

/// Per-agent sliding-window behavioral model.
#[derive(Debug, Default)]
pub struct BehavioralModel {
    history: HashMap<String, VecDeque<ActionObservation>>,
}

impl BehavioralModel {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an observation for an agent. Oldest entries are evicted
    /// once the window is full.
    pub fn observe(&mut self, agent_id: &str, observation: ActionObservation) {
        let queue = self
            .history
            .entry(agent_id.to_string())
            .or_insert_with(VecDeque::new);
        if queue.len() >= WINDOW {
            queue.pop_front();
        }
        queue.push_back(observation);
    }

    /// Compute current [`BehavioralMetrics`] for an agent.
    ///
    /// Agents with no history score neutral (0.5) on all ratio metrics —
    /// conservative by design: unknown is not trusted, and not condemned.
    pub fn metrics(&self, agent_id: &str) -> BehavioralMetrics {
        let history = match self.history.get(agent_id) {
            Some(h) if !h.is_empty() => h,
            _ => {
                return BehavioralMetrics {
                    anomaly_score: 0.5,
                    volatility_score: 0.5,
                    escalation_attempts: 0,
                    historical_stability: 0.5,
                    recent_failures: 0,
                }
            }
        };

        let total = history.len();
        let successes = history.iter().filter(|o| o.succeeded).count();
        let historical_stability = successes as f32 / total as f32;

        let escalation_attempts = history
            .iter()
            .filter(|o| o.was_escalation_attempt)
            .count() as u32;

        let recent_start = total.saturating_sub(RECENT);
        let recent: Vec<&ActionObservation> =
            history.iter().skip(recent_start).collect();
        let recent_failures =
            recent.iter().filter(|o| !o.succeeded).count() as u32;

        BehavioralMetrics {
            anomaly_score: self.novelty_score(history, recent_start),
            volatility_score: Self::volatility(history),
            escalation_attempts,
            historical_stability,
            recent_failures,
        }
    }

    /// TimeAnomaly component per docs/SPEC.md:
    /// - 0.0 → action within established time patterns
    /// - 0.5 → outside normal hours but not unprecedented (or unknown agent)
    /// - 1.0 → hour never observed for this agent
    pub fn time_anomaly(&self, agent_id: &str, hour: u8) -> f32 {
        let history = match self.history.get(agent_id) {
            Some(h) if !h.is_empty() => h,
            // No history: neither established nor unprecedented — neutral.
            _ => return 0.5,
        };

        let total = history.len() as f32;
        let at_hour = history.iter().filter(|o| o.hour == hour).count() as f32;

        if at_hour == 0.0 {
            1.0
        } else if at_hour / total >= ESTABLISHED_HOUR_RATIO {
            0.0
        } else {
            0.5
        }
    }

    /// Fraction of recent actions whose verb never appeared earlier in the
    /// window. New-behavior spikes raise this toward 1.0.
    fn novelty_score(
        &self,
        history: &VecDeque<ActionObservation>,
        recent_start: usize,
    ) -> f32 {
        if recent_start == 0 {
            // No prior baseline to compare against — neutral.
            return 0.5;
        }
        let baseline: std::collections::HashSet<&str> = history
            .iter()
            .take(recent_start)
            .map(|o| o.action.as_str())
            .collect();
        let recent: Vec<&ActionObservation> =
            history.iter().skip(recent_start).collect();
        if recent.is_empty() {
            return 0.0;
        }
        let novel = recent
            .iter()
            .filter(|o| !baseline.contains(o.action.as_str()))
            .count();
        novel as f32 / recent.len() as f32
    }

    /// Rate of success↔failure flips between consecutive observations.
    /// Stable agents (all success or all failure) score 0.0.
    fn volatility(history: &VecDeque<ActionObservation>) -> f32 {
        if history.len() < 2 {
            return 0.0;
        }
        let outcomes: Vec<bool> = history.iter().map(|o| o.succeeded).collect();
        let flips = outcomes
            .windows(2)
            .filter(|w| w[0] != w[1])
            .count();
        flips as f32 / (outcomes.len() - 1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(action: &str, succeeded: bool, hour: u8) -> ActionObservation {
        ActionObservation {
            action: action.into(),
            succeeded,
            was_escalation_attempt: false,
            hour,
        }
    }

    #[test]
    fn unknown_agent_scores_neutral() {
        let model = BehavioralModel::new();
        let m = model.metrics("ghost");
        assert!((m.anomaly_score - 0.5).abs() < f32::EPSILON);
        assert!((m.historical_stability - 0.5).abs() < f32::EPSILON);
        assert_eq!(m.recent_failures, 0);
        assert!((model.time_anomaly("ghost", 3) - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn consistent_success_is_stable_and_calm() {
        let mut model = BehavioralModel::new();
        for _ in 0..50 {
            model.observe("file_agent", obs("open_files", true, 14));
        }
        let m = model.metrics("file_agent");
        assert!((m.historical_stability - 1.0).abs() < f32::EPSILON);
        assert!((m.volatility_score - 0.0).abs() < f32::EPSILON);
        assert_eq!(m.recent_failures, 0);
    }

    #[test]
    fn escalation_attempts_are_counted() {
        let mut model = BehavioralModel::new();
        model.observe(
            "coding_agent",
            ActionObservation {
                action: "write_source".into(),
                succeeded: false,
                was_escalation_attempt: true,
                hour: 14,
            },
        );
        assert_eq!(model.metrics("coding_agent").escalation_attempts, 1);
    }

    #[test]
    fn novel_recent_actions_raise_anomaly() {
        let mut model = BehavioralModel::new();
        // Baseline: 30 routine actions.
        for _ in 0..30 {
            model.observe("agent", obs("open_files", true, 14));
        }
        // Recent burst of never-seen behavior.
        for _ in 0..20 {
            model.observe("agent", obs("modify_config", true, 14));
        }
        let m = model.metrics("agent");
        assert!(
            m.anomaly_score > 0.9,
            "expected high novelty, got {}",
            m.anomaly_score
        );
    }

    #[test]
    fn time_anomaly_tiers_match_spec() {
        let mut model = BehavioralModel::new();
        for _ in 0..99 {
            model.observe("agent", obs("open_files", true, 14));
        }
        model.observe("agent", obs("open_files", true, 22));

        // Established pattern hour.
        assert!((model.time_anomaly("agent", 14) - 0.0).abs() < f32::EPSILON);
        // Seen, but rare (1% < 5% threshold).
        assert!((model.time_anomaly("agent", 22) - 0.5).abs() < f32::EPSILON);
        // Never observed.
        assert!((model.time_anomaly("agent", 3) - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn window_is_bounded() {
        let mut model = BehavioralModel::new();
        for i in 0..(WINDOW + 100) {
            model.observe("agent", obs("a", i % 2 == 0, 14));
        }
        let len = model
            .history
            .get("agent")
            .map(|h| h.len())
            .unwrap_or(0);
        assert_eq!(len, WINDOW);
    }
}
