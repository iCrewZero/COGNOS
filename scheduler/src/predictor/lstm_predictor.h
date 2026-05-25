// lstm_predictor.h — COGNOS/OS behavior prediction LSTM C++ runtime.
// Runs ONNX model exported from PyTorch. Target: <2% CPU, <5ms per predict().
#pragma once
#include <array>
#include <deque>
#include <string>
#include <cstdint>
#include <onnxruntime/core/session/onnxruntime_c_api.h>

namespace cognos {

static constexpr int SEQ_LEN     = 20;
static constexpr int FEATURE_DIM = 23;
static constexpr int N_APP       =  6;
static constexpr int N_DOMAIN    =  6;
static constexpr int MC_PASSES   = 10;
static constexpr float CONF_THRESHOLD = 0.85f;

struct Event {
    float hour_sin;
    float hour_cos;
    float day_onehot[7];
    float app_onehot[6];
    float domain_onehot[6];
    float session_depth;
    float time_since_last;

    std::array<float, FEATURE_DIM> to_array() const {
        std::array<float, FEATURE_DIM> v{};
        int i = 0;
        v[i++] = hour_sin;
        v[i++] = hour_cos;
        for (auto x : day_onehot)    v[i++] = x;
        for (auto x : app_onehot)    v[i++] = x;
        for (auto x : domain_onehot) v[i++] = x;
        v[i++] = session_depth;
        v[i++] = time_since_last;
        return v;
    }
};

struct Prediction {
    float app_probs[N_APP];
    float domain_probs[N_DOMAIN];
    float confidence;
    bool  should_preload;
};

class LstmPredictor {
public:
    explicit LstmPredictor(const std::string& model_path);
    ~LstmPredictor();

    void       push_event(const Event& event);
    Prediction predict();
    bool       is_loaded() const { return session_ != nullptr; }
    std::string model_info() const { return model_info_; }

private:
    OrtEnv*           env_     = nullptr;
    OrtSession*       session_ = nullptr;
    OrtSessionOptions* opts_   = nullptr;
    OrtMemoryInfo*    mem_info_ = nullptr;

    // Pre-allocated tensors (reused across inferences)
    std::vector<float> input_data_;
    std::vector<float> app_out_;
    std::vector<float> domain_out_;
    std::vector<float> conf_out_;

    // Circular event buffer — last SEQ_LEN events
    std::deque<std::array<float, FEATURE_DIM>> ring_;
    std::string model_info_;

    const OrtApi* api_ = nullptr;

    void run_once(float* app_p, float* dom_p, float* conf_p);
    void softmax(float* data, int n);
};

} // namespace cognos