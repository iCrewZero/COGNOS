//!
//!

use std::collections::{
    HashMap,
    VecDeque,
};

use chrono::Utc;

use crate::hal_types::{
    BehavioralMetrics,
    RuntimeAnomaly,
};

pub struct AnomalyDetector {
    history:
        HashMap<
            String,
            VecDeque<f32>,
        >,
}

impl AnomalyDetector {
    pub fn new() -> Self {
        Self {
            history:
                HashMap::new(),
        }
    }

    pub fn record_metric(
        &mut self,

        agent_id: &str,

        value: f32,
    ) {
        let queue =
            self
                .history
                .entry(
                    agent_id
                        .to_string()
                )
                .or_insert_with(
                    VecDeque::new
                );

        queue.push_back(
            value
        );

        if queue.len() > 256 {
            queue.pop_front();
        }
    }

    pub fn anomaly_score(
        &self,

        agent_id: &str,

        current: f32,
    ) -> f32 {
        let history =
            match self
                .history
                .get(agent_id)
            {
                Some(h) => h,
                None => {
                    return 0.0;
                }
            };

        if history.is_empty() {
            return 0.0;
        }

        let mean =
            history.iter()
                .sum::<f32>()
                / history.len()
                    as f32;

        let variance =
            history.iter()
                .map(
                    |v| {
                        (
                            v - mean
                        )
                            .powi(2)
                    },
                )
                .sum::<f32>()
                / history.len()
                    as f32;

        let std_dev =
            variance.sqrt();

        if std_dev == 0.0 {
            return 0.0;
        }

        let z_score =
            (
                current - mean
            )
                .abs()
                / std_dev;

        (
            z_score / 10.0
        )
            .clamp(0.0, 1.0)
    }

    pub fn behavioral_drift(
        &self,

        baseline: f32,

        current: f32,
    ) -> f32 {
        (
            current - baseline
        )
            .abs()
            .clamp(0.0, 1.0)
    }

    pub fn detect_spike(
        &self,

        previous: f32,

        current: f32,
    ) -> bool {
        (
            current - previous
        )
            .abs()
            > 0.45
    }

    pub fn syscall_frequency_anomaly(
        &self,

        expected: u64,

        observed: u64,
    ) -> f32 {
        if expected == 0 {
            return 1.0;
        }

        let ratio =
            observed
                as f32
                / expected
                    as f32;

        (
            ratio - 1.0
        )
            .abs()
            .clamp(0.0, 1.0)
    }

    pub fn capability_abuse_score(
        &self,

        granted: usize,

        used: usize,
    ) -> f32 {
        if granted == 0 {
            return 1.0;
        }

        let utilization =
            used as f32
                / granted
                    as f32;

        if utilization > 1.0 {
            return 1.0;
        }

        (
            utilization
                - 0.70
        )
            .max(0.0)
            .clamp(0.0, 1.0)
    }

    pub fn repeated_failures(
        &self,

        failures: u32,
    ) -> f32 {
        (
            failures
                as f32
                / 10.0
        )
            .clamp(0.0, 1.0)
    }

    pub fn detect_privilege_drift(
        &self,

        baseline_caps:
            &[String],

        current_caps:
            &[String],
    ) -> bool {
        current_caps.len()
            > baseline_caps.len()
    }

    pub fn entropy_score(
        &self,

        values: &[f32],
    ) -> f32 {
        if values.is_empty() {
            return 0.0;
        }

        let sum: f32 =
            values.iter()
                .sum();

        if sum == 0.0 {
            return 0.0;
        }

        let entropy =
            values.iter()
                .map(
                    |v| {
                        let p =
                            v / sum;

                        if p == 0.0 {
                            0.0
                        } else {
                            -p
                                * p
                                    .log2()
                        }
                    },
                )
                .sum::<f32>();

        (
            entropy / 8.0
        )
            .clamp(0.0, 1.0)
    }

    pub fn generate_anomaly(
        &self,

        anomaly_type: &str,

        severity: f32,

        description: &str,
    ) -> RuntimeAnomaly {
        RuntimeAnomaly {
            anomaly_type:
                anomaly_type
                    .into(),

            severity,

            description:
                description
                    .into(),

            detected_at:
                Utc::now()
                    .timestamp_millis(),
        }
    }

    pub fn aggregate_behavioral_metrics(
        &self,

        anomaly_score: f32,

        volatility_score: f32,

        escalation_attempts: u32,

        historical_stability: f32,

        recent_failures: u32,
    ) -> BehavioralMetrics {
        BehavioralMetrics {
            anomaly_score,

            volatility_score,

            escalation_attempts,

            historical_stability,

            recent_failures,
        }
    }

    pub fn compromise_probability(
        &self,

        anomaly_score: f32,

        escalation_attempts: u32,

        volatility: f32,
    ) -> f32 {
        let mut probability =
            anomaly_score
                * 0.50;

        probability +=
            (
                escalation_attempts
                    as f32
                    / 10.0
            )
                * 0.30;

        probability +=
            volatility
                * 0.20;

        probability
            .clamp(0.0, 1.0)
    }

    pub fn isolate_threshold(
        compromise_probability:
            f32,
    ) -> bool {
        compromise_probability
            > 0.80
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anomaly_score_detected() {
        let mut detector =
            AnomalyDetector::new();

        for _ in 0..50 {
            detector.record_metric(
                "planner",
                0.10,
            );
        }

        let score =
            detector
                .anomaly_score(
                    "planner",
                    1.0,
                );

        assert!(
            score > 0.5
        );
    }

    #[test]
    fn spike_detected() {
        let detector =
            AnomalyDetector::new();

        assert!(
            detector
                .detect_spike(
                    0.1,
                    0.9,
                )
        );
    }

    #[test]
    fn privilege_drift_detected() {
        let detector =
            AnomalyDetector::new();

        let baseline =
            vec![
                "memory.read"
                    .into()
            ];

        let current =
            vec![
                "memory.read"
                    .into(),

                "filesystem.write"
                    .into(),
            ];

        assert!(
            detector
                .detect_privilege_drift(
                    &baseline,
                    &current,
                )
        );
    }
}