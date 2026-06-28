// lstm_predictor.h — LSTM workload predictor used by the COGNOS scheduler.
//
// Purpose:
//   Given a recent window of TelemetrySample records, predict which
//   "scenario" the user is currently in (CodingActive, VideoRendering,
//   BatteryCritical, IdleOvernight, Gaming, VibeCoding, GeneralUse) and
//   return a confidence score plus a per-scenario alternative distribution.
//   The scheduler uses this to preload models, pin CPU affinities, and
//   adjust cgroup weights proactively.
//
// Usage:
//   cognos::LstmPredictor predictor;
//   if (predictor.load_model("/var/lib/cognos/lstm.bin")) {
//       auto pred = predictor.predict(telemetry_history);
//       if (pred.scenario == cognos::Scenario::VideoRendering) {
//           // bump GPU cgroup weight, preload render LLM, etc.
//       }
//   }
//
// The class is PImpl-backed so the header has no STL-heavy internals
// beyond <vector>, <cstdint>, <string>.
//
// v0: stub

#pragma once

#include <vector>
#include <cstdint>
#include <string>

namespace cognos {

/// A single telemetry snapshot fed into the predictor.
///
/// All `*_usage` / `*_rate` fields are normalized to `[0.0, 1.0]` by the
/// scheduler's telemetry sampler before being handed to the predictor.
struct TelemetrySample {
    /// CPU utilization in `[0, 1]` (averaged over the sample interval).
    float cpu_usage;
    /// GPU utilization in `[0, 1]`.
    float gpu_usage;
    /// RAM utilization in `[0, 1]` (committed / total).
    float ram_usage;
    /// Block-IO rate in `[0, 1]` (fraction of the configured baseline BW).
    float io_rate;
    /// Network rate in `[0, 1]` (fraction of the configured baseline BW).
    float net_rate;
    /// Numeric id of the app that currently has keyboard focus, or `-1`
    /// if none / unknown.
    int foreground_app_id;
    /// Nanosecond timestamp (CLOCK_MONOTONIC) at which the sample was
    /// collected.
    uint64_t timestamp_ns;
};

/// The discrete set of user scenarios the predictor can emit.
///
/// `Count` is a sentinel used to size the `alternatives` array in
/// [`Prediction`]; it must remain the last enumerator.
enum class Scenario : int {
    CodingActive     = 0,  ///< Active editing / compilation loop.
    VideoRendering   = 1,  ///< GPU-bound encode / render.
    BatteryCritical  = 2,  ///< < 15% battery — enter power-saver.
    IdleOvernight    = 3,  ///< Long idle window — run maintenance.
    Gaming           = 4,  ///< Latency-sensitive interactive workload.
    VibeCoding       = 5,  ///< AI-assisted rapid prototyping.
    GeneralUse       = 6,  ///< Default / unknown.
    Count            = 7,  ///< Sentinel — must stay last.
};

/// Output of [`LstmPredictor::predict`].
struct Prediction {
    /// The most-likely scenario.
    Scenario scenario;
    /// Confidence in `[0, 1]` for the chosen `scenario`.
    float confidence;
    /// Horizon (seconds) over which the prediction is expected to hold.
    /// The scheduler should re-predict at least this often.
    float horizon_seconds;
    /// Per-scenario probability distribution. `alternatives[i]` is the
    /// raw (pre-argmax) score for `Scenario(i)`. The sum is not
    /// guaranteed to be 1.0 — the scheduler should softmax if it needs
    /// a proper distribution.
    float alternatives[static_cast<int>(Scenario::Count)];
};

/// LSTM-based workload predictor.
///
/// PImpl-backed: the public surface is intentionally minimal so that the
/// header can be included from translation units that do not link against
/// a full BLAS / Eigen stack.
class LstmPredictor {
public:
    LstmPredictor();
    ~LstmPredictor();

    LstmPredictor(const LstmPredictor&) = delete;
    LstmPredictor& operator=(const LstmPredictor&) = delete;

    /// Load a model blob from `path`.
    ///
    /// The file format is private to the implementation; see
    /// `lstm_predictor.cpp` for the (v0) header layout. Returns `false`
    /// if the file does not exist or fails basic validation.
    bool load_model(const std::string& path);

    /// Returns `true` once a model has been successfully loaded.
    bool is_loaded() const;

    /// Predict the current scenario from a recent telemetry window.
    ///
    /// If `history` is shorter than [`input_window_size`], the
    /// implementation zero-pads at the front. If the model is not
    /// loaded, a heuristic fallback is used (see the .cpp for details).
    Prediction predict(const std::vector<TelemetrySample>& history);

    /// Number of telemetry samples the model consumes in one forward
    /// pass. Always returns a positive value.
    int input_window_size() const;

    /// LSTM hidden-state width. Used by the scheduler to estimate the
    /// memory cost of running the predictor continuously.
    int hidden_size() const;

private:
    struct Impl;
    Impl* impl_;
};

} // namespace cognos

// v0: stub
