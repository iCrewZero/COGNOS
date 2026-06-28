//! Workload predictor — reads eBPF telemetry and predicts the next usage
//! scenario (CodingActive, VideoRendering, etc.) using an LSTM model.
//! Predictions feed the policy engine.
//!
//! In v0 the predictor is a stub: it accepts telemetry samples and returns a
//! `GeneralUse` forecast with zero confidence. v1 wires the ONNX LSTM backend
//! that lives under `scheduler/src/predictor/` (see `lstm_predictor.cpp`,
//! `runtime.cpp`, `export_onnx.py`).

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// Scenario is the canonical enum defined in daemon.rs.
// predictor.rs needs it for the prediction output type.
use crate::daemon::Scenario;

// ─── Telemetry Sample ─────────────────────────────────────────────────────────

/// A single point-in-time observation of system-wide metrics, as produced by
/// the eBPF `scheduler_telemetry` map (see `kernel/ebpf/scheduler_telemetry.bpf.c`).
/// The predictor consumes a rolling window of these to forecast the upcoming
/// scenario.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TelemetrySample {
    /// Aggregate CPU usage in `[0.0, 1.0]` across all cores.
    pub cpu: f32,
    /// GPU usage in `[0.0, 1.0]`; `0.0` when no GPU is present.
    pub gpu: f32,
    /// RAM used, in GiB.
    pub ram: f32,
    /// I/O operations per second across all block devices.
    pub io: u64,
    /// Network packets per second across all interfaces.
    pub net: u64,
    /// Foreground application identifier (e.g. `"vscode"`, `"steam"`); empty
    /// when the compositor has not reported one.
    pub foreground_app: String,
    /// When the sample was taken.
    pub timestamp: DateTime<Utc>,
}

// ─── Prediction Model ─────────────────────────────────────────────────────────

/// Opaque handle to a loaded LSTM model. In v0 the field is informational
/// only; v1 wraps an ONNX runtime session loaded from
/// `/usr/lib/cognos/models/scheduler_lstm.onnx`.
#[derive(Debug, Clone, Default)]
pub struct LstmHandle {
    /// Filesystem path the model was loaded from, if any.
    pub model_path: Option<String>,
    /// Number of input timesteps the model expects per inference call.
    pub context_window: usize,
}

/// Which predictor backend to use.
#[derive(Debug, Clone, Default)]
pub enum PredictionModel {
    /// LSTM-based forecaster. Requires a loaded model; if
    /// [`LstmHandle::model_path`] is `None`, [`Predictor::predict`] returns
    /// [`PredictorError::ModelNotLoaded`].
    Lstm(LstmHandle),
    /// Hand-rolled rule-based fallback. Always available; mirrors the
    /// heuristic detection in `daemon::SchedulerDaemon::detect_scenario`.
    Heuristic,
    /// Run both LSTM and heuristic; reconcile disagreements. The default
    /// backend in v0 so the stub degrades cleanly to `GeneralUse`.
    #[default]
    Hybrid,
}

/// Internal wrapper carrying the chosen backend plus tunable knobs.
#[derive(Debug, Clone, Default)]
pub struct PredictorModel {
    /// Active prediction backend.
    pub kind: PredictionModel,
    /// Minimum samples required before [`Predictor::predict`] will emit a
    /// forecast. Below this the predictor returns
    /// [`PredictorError::InsufficientHistory`].
    pub min_history: usize,
}

// ─── Prediction ───────────────────────────────────────────────────────────────

/// A scenario forecast produced by [`Predictor::predict`]. Confidence below
/// `0.5` causes the runtime to fall back to [`Scenario::GeneralUse`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Prediction {
    /// Predicted scenario for the upcoming window.
    pub scenario: Scenario,
    /// Confidence in `[0.0, 1.0]`.
    pub confidence: f32,
    /// Forecast horizon this prediction covers.
    pub horizon: Duration,
    /// Ranked alternative scenarios with their (lower) confidence scores.
    pub alternatives: Vec<(Scenario, f32)>,
}

// ─── Errors ───────────────────────────────────────────────────────────────────

/// Failures that can occur during prediction.
#[derive(Debug, Error)]
pub enum PredictorError {
    /// No model has been loaded for the requested LSTM backend.
    #[error("predictor model not loaded")]
    ModelNotLoaded,
    /// Not enough telemetry history has been accumulated yet.
    #[error("insufficient history: have {have}, need {need}")]
    InsufficientHistory {
        /// Number of samples currently in the history buffer.
        have: usize,
        /// Minimum samples required by the active model.
        need: usize,
    },
    /// The inference backend returned an error.
    #[error("inference failed: {0}")]
    InferenceFailed(String),
}

// ─── Predictor ────────────────────────────────────────────────────────────────

/// Workload predictor — accumulates eBPF telemetry and forecasts the next
/// scenario over a fixed [`horizon`](Self::horizon).
///
/// Typical lifecycle:
///
/// ```text
/// predictor.push_sample(sample);          // each tick (1Hz)
/// let prediction = predictor.predict().await?;
/// // → policy engine consumes prediction.scenario
/// ```
pub struct Predictor {
    /// Active prediction backend and its tunables.
    pub model: PredictorModel,
    /// Rolling window of recent telemetry samples.
    pub history: Vec<TelemetrySample>,
    /// How far into the future the predictor forecasts.
    pub horizon: Duration,
}

impl Predictor {
    /// Build a predictor with the given model and forecast horizon.
    pub fn new(model: PredictorModel, horizon: Duration) -> Self {
        Self {
            model,
            history: Vec::new(),
            horizon,
        }
    }

    /// Append a new telemetry sample to the rolling history window.
    pub fn push_sample(&mut self, sample: TelemetrySample) {
        self.history.push(sample);
        // v0: keep an unbounded buffer. TODO(v1): bound to the LSTM
        // context window and drop samples older than `horizon * N`.
    }

    /// Forecast the next scenario over `horizon`. Confidence below `0.5`
    /// falls back to [`Scenario::GeneralUse`] so the runtime degrades
    /// safely when the model is uncertain.
    pub async fn predict(&self) -> Result<Prediction, PredictorError> {
        // v0: stub implementation.
        let min_history = self.model.min_history.max(1);
        if self.history.len() < min_history {
            return Err(PredictorError::InsufficientHistory {
                have: self.history.len(),
                need: min_history,
            });
        }

        let mut prediction = match &self.model.kind {
            PredictionModel::Lstm(handle) if handle.model_path.is_none() => {
                return Err(PredictorError::ModelNotLoaded);
            }
            PredictionModel::Lstm(_) => {
                // TODO(v1): invoke the ONNX runtime on the history window
                // via the C++ shim under `scheduler/src/predictor/`.
                return Err(PredictorError::InferenceFailed(
                    "LSTM backend not yet implemented".into(),
                ));
            }
            PredictionModel::Heuristic | PredictionModel::Hybrid => {
                // TODO(v1): rule-based forecaster over the history window,
                // mirroring `daemon::SchedulerDaemon::detect_scenario`.
                Prediction {
                    scenario: Scenario::GeneralUse,
                    confidence: 0.0,
                    horizon: self.horizon,
                    alternatives: Vec::new(),
                }
            }
        };

        // Confidence < 0.5 → fall back to GeneralUse so the runtime never
        // acts on a low-confidence forecast.
        if prediction.confidence < 0.5 {
            prediction.scenario = Scenario::GeneralUse;
        }

        Ok(prediction)
    }
}

// v0: stub implementation
