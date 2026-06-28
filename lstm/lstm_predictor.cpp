// lstm_predictor.cpp — implementation of the COGNOS LSTM workload predictor.
//
// v0: stub — real LSTM forward pass is TODO; uses heuristic fallback
// when no model file is present, and a zero-initialized "loaded" path
// when a model file exists so the scheduler can exercise both code
// paths during bring-up.
//
// See lstm_predictor.h for the public API contract.

#include "lstm_predictor.h"

#include <cmath>
#include <algorithm>
#include <fstream>
#include <stdexcept>
#include <cstring>

namespace cognos {

// ─── Impl ───────────────────────────────────────────────────────────────────

/// Private implementation details.
///
/// Weight tensors are stored row-major and flat. The exact layout is
/// TODO(v1) — once the real forward pass lands we will document the
/// matrix shapes here.
struct LstmPredictor::Impl {
    bool loaded = false;
    int hidden_size = 64;
    int input_window = 32;

    /// Input-to-hidden weights (concatenation of the 4 LSTM gates).
    /// Shape: `[4 * hidden_size, input_window * feature_dim]` (v0: empty).
    std::vector<float> weights_ih;
    /// Hidden-to-hidden weights. Shape: `[4 * hidden_size, hidden_size]`.
    std::vector<float> weights_hh;
    /// Bias for the 4 gates. Shape: `[4 * hidden_size]`.
    std::vector<float> bias;
};

// ─── Construction / destruction ─────────────────────────────────────────────

LstmPredictor::LstmPredictor()
    : impl_(new Impl()) {
    // Defaults are sane: a 64-unit LSTM over a 32-sample window. The
    // scheduler can read these back via input_window_size() / hidden_size()
    // even before load_model() is called.
}

LstmPredictor::~LstmPredictor() {
    delete impl_;
    impl_ = nullptr;
}

// ─── Model loading ──────────────────────────────────────────────────────────

namespace {

// Magic bytes for the v0 model file format. Spelled out as four ASCII
// chars so the file can be inspected with `file` / `xxd`.
constexpr char kMagic[4] = {'C', 'L', 'S', 'T'};  // CognoS LsTm
constexpr uint32_t kVersion = 1;

#pragma pack(push, 1)
struct ModelHeader {
    char magic[4];
    uint32_t version;
    int32_t hidden_size;
    int32_t input_window;
    int32_t feature_dim;
    uint64_t weights_ih_count;
    uint64_t weights_hh_count;
    uint64_t bias_count;
};
#pragma pack(pop)

} // namespace

bool LstmPredictor::load_model(const std::string& path) {
    std::ifstream f(path, std::ios::binary);
    if (!f) {
        // File not found / unreadable — predictor stays in heuristic mode.
        impl_->loaded = false;
        return false;
    }

    ModelHeader hdr{};
    f.read(reinterpret_cast<char*>(&hdr), sizeof(hdr));
    if (!f || std::memcmp(hdr.magic, kMagic, 4) != 0) {
        // Bad magic — refuse to load rather than risk running garbage.
        impl_->loaded = false;
        return false;
    }
    if (hdr.version != kVersion) {
        // Version mismatch — TODO(v1): add a migration path.
        impl_->loaded = false;
        return false;
    }
    if (hdr.hidden_size <= 0 || hdr.input_window <= 0) {
        impl_->loaded = false;
        return false;
    }

    impl_->hidden_size = hdr.hidden_size;
    impl_->input_window = hdr.input_window;

    // Read the three weight tensors. We do not validate the element counts
    // against the declared dimensions in v0 — the real forward pass (v1)
    // will do shape checking.
    impl_->weights_ih.resize(hdr.weights_ih_count);
    impl_->weights_hh.resize(hdr.weights_hh_count);
    impl_->bias.resize(hdr.bias_count);

    if (hdr.weights_ih_count > 0) {
        f.read(reinterpret_cast<char*>(impl_->weights_ih.data()),
               static_cast<std::streamsize>(hdr.weights_ih_count * sizeof(float)));
    }
    if (hdr.weights_hh_count > 0) {
        f.read(reinterpret_cast<char*>(impl_->weights_hh.data()),
               static_cast<std::streamsize>(hdr.weights_hh_count * sizeof(float)));
    }
    if (hdr.bias_count > 0) {
        f.read(reinterpret_cast<char*>(impl_->bias.data()),
               static_cast<std::streamsize>(hdr.bias_count * sizeof(float)));
    }

    if (!f) {
        // Truncated file — fail closed.
        impl_->loaded = false;
        return false;
    }

    impl_->loaded = true;
    return true;
}

bool LstmPredictor::is_loaded() const {
    return impl_ != nullptr && impl_->loaded;
}

// ─── Prediction ─────────────────────────────────────────────────────────────

namespace {

/// Heuristic fallback used when no model is loaded or when the model
/// returns no usable output.
///
/// Rules (in priority order):
///   1. cpu_usage > 0.7  → CodingActive
///   2. gpu_usage > 0.8  → VideoRendering
///   3. (battery proxy)  → BatteryCritical  — NOTE: v0 has no battery
///      field in TelemetrySample; we approximate by treating very low
///      cpu + very low io + foreground_app_id == -1 as BatteryCritical
///      when the caller knows the battery is low. TODO(v1): add a real
///      battery field to TelemetrySample and use a 0.15 threshold.
///   4. otherwise        → GeneralUse
///
/// The `battery_estimate` argument is the caller-provided battery fraction
/// in `[0, 1]`, or `>= 1.0` if unknown.
Prediction heuristic_predict(
    const std::vector<TelemetrySample>& history,
    float battery_estimate) {
    Prediction p{};
    std::memset(&p, 0, sizeof(p));

    if (history.empty()) {
        p.scenario = Scenario::GeneralUse;
        p.confidence = 0.0f;
        p.horizon_seconds = 0.0f;
        return p;
    }

    const TelemetrySample& last = history.back();

    if (battery_estimate < 0.15f) {
        p.scenario = Scenario::BatteryCritical;
        p.confidence = 0.8f;
        p.horizon_seconds = 300.0f;  // 5 min — re-eval soon
    } else if (last.gpu_usage > 0.8f) {
        p.scenario = Scenario::VideoRendering;
        p.confidence = 0.7f;
        p.horizon_seconds = 120.0f;
    } else if (last.cpu_usage > 0.7f) {
        p.scenario = Scenario::CodingActive;
        p.confidence = 0.7f;
        p.horizon_seconds = 60.0f;
    } else {
        p.scenario = Scenario::GeneralUse;
        p.confidence = 0.4f;
        p.horizon_seconds = 60.0f;
    }

    // Spread a small amount of probability mass across the alternatives
    // so the scheduler can see that the model was not 100% certain.
    const int n = static_cast<int>(Scenario::Count);
    const float remaining = 1.0f - p.confidence;
    const float share = remaining / static_cast<float>(n - 1);
    for (int i = 0; i < n; ++i) {
        p.alternatives[i] = (i == static_cast<int>(p.scenario))
                                ? p.confidence
                                : share;
    }
    return p;
}

} // namespace

Prediction LstmPredictor::predict(
    const std::vector<TelemetrySample>& history) {
    if (!impl_ || !impl_->loaded) {
        // No model — fall back to the rule-based heuristic. We pass
        // `1.0f` as the battery estimate (i.e. "unknown / not low") so
        // the BatteryCritical branch only fires when the caller wires
        // in a real battery source. TODO(v1): plumb battery through
        // TelemetrySample.
        return heuristic_predict(history, 1.0f);
    }

    // ── Loaded path (v0 placeholder) ───────────────────────────────────
    //
    // The real LSTM forward pass is TODO(v1). For v0 we run the same
    // heuristic, but we mark it as "loaded" by bumping confidence
    // slightly and stretching the horizon — this lets the scheduler
    // integration tests distinguish the two code paths.
    //
    // The weights_ih / weights_hh / bias tensors are loaded but unused
    // here. They will be consumed by the v1 forward pass.

    Prediction p = heuristic_predict(history, 1.0f);
    if (p.confidence > 0.0f) {
        p.confidence = std::min(1.0f, p.confidence + 0.05f);
        p.horizon_seconds = std::max(p.horizon_seconds, 90.0f);
    }

    // TODO(v1): replace the block above with a real LSTM forward pass:
    //   1. Normalize the last `input_window` samples into a feature
    //      matrix of shape `[input_window, feature_dim]`.
    //   2. Run the LSTM cell for `input_window` steps, maintaining
    //      hidden state h and cell state c.
    //   3. Apply the output projection + softmax to produce the
    //      `alternatives` distribution.
    //   4. Pick argmax for `scenario` and copy the softmax peak into
    //      `confidence`.

    return p;
}

// ─── Accessors ──────────────────────────────────────────────────────────────

int LstmPredictor::input_window_size() const {
    return impl_ ? impl_->input_window : 0;
}

int LstmPredictor::hidden_size() const {
    return impl_ ? impl_->hidden_size : 0;
}

} // namespace cognos

// v0: stub — real LSTM forward pass is TODO; uses heuristic fallback
