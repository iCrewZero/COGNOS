// lstm_predictor.cpp — COGNOS/OS behavior prediction LSTM C++ runtime.
// Production inference: loads ONNX model, runs MC dropout for uncertainty.
#include "lstm_predictor.h"
#include <onnxruntime/core/session/onnxruntime_cxx_api.h>
#include <algorithm>
#include <cassert>
#include <cmath>
#include <cstring>
#include <iostream>
#include <numeric>
#include <stdexcept>
#include <string>
#include <vector>

// ── IPC JSON line protocol ────────────────────────────────────────────────────
// Subprocess interface: stdin → JSON commands, stdout → JSON responses.
// Used by the Python scheduler daemon to talk to this binary.

#include <cstdio>

static void json_response(const char* type, const cognos::Prediction& p) {
    printf("{\"type\":\"%s\","
           "\"app_probs\":[%.4f,%.4f,%.4f,%.4f,%.4f,%.4f],"
           "\"domain_probs\":[%.4f,%.4f,%.4f,%.4f,%.4f,%.4f],"
           "\"confidence\":%.4f,"
           "\"should_preload\":%s}\n",
           type,
           p.app_probs[0], p.app_probs[1], p.app_probs[2],
           p.app_probs[3], p.app_probs[4], p.app_probs[5],
           p.domain_probs[0], p.domain_probs[1], p.domain_probs[2],
           p.domain_probs[3], p.domain_probs[4], p.domain_probs[5],
           p.confidence,
           p.should_preload ? "true" : "false");
    fflush(stdout);
}

static void json_error(const char* msg) {
    printf("{\"type\":\"error\",\"message\":\"%s\"}\n", msg);
    fflush(stdout);
}

// ── LstmPredictor ─────────────────────────────────────────────────────────────

namespace cognos {

LstmPredictor::LstmPredictor(const std::string& model_path)
    : input_data_(SEQ_LEN * FEATURE_DIM, 0.0f)
    , app_out_(N_APP, 0.0f)
    , domain_out_(N_DOMAIN, 0.0f)
    , conf_out_(1, 0.0f)
{
    api_ = OrtGetApiBase()->GetApi(ORT_API_VERSION);

    // Environment
    api_->CreateEnv(ORT_LOGGING_LEVEL_WARNING, "cognos_lstm", &env_);

    // Session options: single-threaded for predictable latency
    api_->CreateSessionOptions(&opts_);
    api_->SetIntraOpNumThreads(opts_, 1);
    api_->SetInterOpNumThreads(opts_, 1);
    api_->EnableMemPattern(opts_);
    api_->EnableCpuMemArena(opts_);

    // Memory info
    api_->CreateCpuMemoryInfo(OrtArenaAllocator, OrtMemTypeDefault, &mem_info_);

    // Load model
    OrtStatus* status = api_->CreateSession(env_, model_path.c_str(), opts_, &session_);
    if (status) {
        const char* msg = api_->GetErrorMessage(status);
        model_info_ = std::string("load_failed: ") + msg;
        api_->ReleaseStatus(status);
        session_ = nullptr;
        return;
    }

    model_info_ = "loaded:" + model_path;
}

LstmPredictor::~LstmPredictor() {
    if (session_)  api_->ReleaseSession(session_);
    if (opts_)     api_->ReleaseSessionOptions(opts_);
    if (mem_info_) api_->ReleaseMemoryInfo(mem_info_);
    if (env_)      api_->ReleaseEnv(env_);
}

void LstmPredictor::push_event(const Event& event) {
    ring_.push_back(event.to_array());
    if ((int)ring_.size() > SEQ_LEN) ring_.pop_front();
}

Prediction LstmPredictor::predict() {
    Prediction result{};

    if (!is_loaded() || (int)ring_.size() < 5) {
        // Not enough history — return zero-confidence prediction
        std::fill(result.app_probs,    result.app_probs    + N_APP,    1.0f/N_APP);
        std::fill(result.domain_probs, result.domain_probs + N_DOMAIN, 1.0f/N_DOMAIN);
        result.confidence     = 0.0f;
        result.should_preload = false;
        return result;
    }

    // Build input tensor: pad front with zeros if buffer not full yet
    std::fill(input_data_.begin(), input_data_.end(), 0.0f);
    int offset = SEQ_LEN - (int)ring_.size();
    for (int i = 0; i < (int)ring_.size(); ++i) {
        const auto& row = ring_[i];
        std::copy(row.begin(), row.end(),
                  input_data_.begin() + (offset + i) * FEATURE_DIM);
    }

    // MC dropout: run inference MC_PASSES times, collect outputs
    float app_sum[N_APP]    = {};
    float dom_sum[N_DOMAIN] = {};
    float conf_sum          = 0.0f;

    // Track per-pass top predictions for std computation
    float app_tops[MC_PASSES]  = {};
    float dom_tops[MC_PASSES]  = {};

    for (int pass = 0; pass < MC_PASSES; ++pass) {
        run_once(app_out_.data(), domain_out_.data(), conf_out_.data());

        for (int j = 0; j < N_APP;    ++j) app_sum[j]  += app_out_[j];
        for (int j = 0; j < N_DOMAIN; ++j) dom_sum[j]  += domain_out_[j];
        conf_sum += conf_out_[0];

        app_tops[pass] = *std::max_element(app_out_.begin(), app_out_.end());
        dom_tops[pass] = *std::max_element(domain_out_.begin(), domain_out_.end());
    }

    // Mean of passes
    for (int j = 0; j < N_APP;    ++j) result.app_probs[j]    = app_sum[j]  / MC_PASSES;
    for (int j = 0; j < N_DOMAIN; ++j) result.domain_probs[j] = dom_sum[j] / MC_PASSES;

    // Confidence = 1 - mean(std of top predictions)
    auto std_of = [](float* arr, int n) {
        float mean = 0.0f;
        for (int i = 0; i < n; ++i) mean += arr[i];
        mean /= n;
        float var = 0.0f;
        for (int i = 0; i < n; ++i) var += (arr[i]-mean)*(arr[i]-mean);
        return std::sqrt(var / n);
    };

    float std_app = std_of(app_tops, MC_PASSES);
    float std_dom = std_of(dom_tops, MC_PASSES);
    result.confidence = 1.0f - (std_app + std_dom) * 0.5f;
    result.confidence = std::max(0.0f, std::min(1.0f, result.confidence));
    result.should_preload = result.confidence >= CONF_THRESHOLD;

    return result;
}

void LstmPredictor::run_once(float* app_p, float* dom_p, float* conf_p) {
    // Input tensor shape: [1, SEQ_LEN, FEATURE_DIM]
    int64_t input_shape[] = {1, SEQ_LEN, FEATURE_DIM};
    OrtValue* input_tensor = nullptr;
    api_->CreateTensorWithDataAsOrtValue(
        mem_info_,
        input_data_.data(),
        input_data_.size() * sizeof(float),
        input_shape, 3,
        ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,
        &input_tensor);

    const char* input_names[]  = {"sequence"};
    const char* output_names[] = {"app_logits", "domain_logits", "confidence"};
    OrtValue*   outputs[3]     = {nullptr, nullptr, nullptr};

    api_->Run(session_, nullptr,
              input_names,  &input_tensor, 1,
              output_names, 3, outputs);

    // Extract output data
    float* app_data  = nullptr; api_->GetTensorMutableData(outputs[0], (void**)&app_data);
    float* dom_data  = nullptr; api_->GetTensorMutableData(outputs[1], (void**)&dom_data);
    float* conf_data = nullptr; api_->GetTensorMutableData(outputs[2], (void**)&conf_data);

    // Apply softmax to logits
    std::copy(app_data, app_data + N_APP, app_p);
    std::copy(dom_data, dom_data + N_DOMAIN, dom_p);
    conf_p[0] = conf_data[0];
    softmax(app_p, N_APP);
    softmax(dom_p, N_DOMAIN);

    api_->ReleaseValue(input_tensor);
    for (auto* v : outputs) if (v) api_->ReleaseValue(v);
}

void LstmPredictor::softmax(float* data, int n) {
    float mx = *std::max_element(data, data + n);
    float sum = 0.0f;
    for (int i = 0; i < n; ++i) { data[i] = std::exp(data[i] - mx); sum += data[i]; }
    for (int i = 0; i < n; ++i) data[i] /= sum;
}

} // namespace cognos

// ── Main: IPC subprocess loop ─────────────────────────────────────────────────

#include <csignal>
static volatile bool running = true;
static void handle_sigterm(int) { running = false; }

int main(int argc, char** argv) {
    std::string model_path = (argc > 1) ? argv[1] : (
        std::string(getenv("HOME") ? getenv("HOME") : "/tmp") +
        "/.cognos/predictor/model.onnx");

    cognos::LstmPredictor predictor(model_path);
    if (!predictor.is_loaded()) {
        fprintf(stderr, "[lstm] Model not loaded: %s\n", predictor.model_info().c_str());
    } else {
        fprintf(stderr, "[lstm] Ready: %s\n", predictor.model_info().c_str());
    }

    signal(SIGTERM, handle_sigterm);
    signal(SIGINT,  handle_sigterm);

    char line[4096];
    while (running && fgets(line, sizeof(line), stdin)) {
        // Minimal JSON dispatch — no external parser dependency
        std::string cmd(line);

        if (cmd.find("\"push_event\"") != std::string::npos) {
            // Parse event fields from JSON (simplified, exact structure expected)
            cognos::Event e{};
            // In production, use a proper JSON parser. Here we rely on the
            // Python caller to send well-formed data since both sides are ours.
            sscanf(cmd.c_str(),
                   "{\"type\":\"push_event\",\"hour_sin\":%f,\"hour_cos\":%f,"
                   "\"session_depth\":%f,\"time_since_last\":%f}",
                   &e.hour_sin, &e.hour_cos, &e.session_depth, &e.time_since_last);
            predictor.push_event(e);
            printf("{\"type\":\"ok\"}\n");
            fflush(stdout);

        } else if (cmd.find("\"predict\"") != std::string::npos) {
            cognos::Prediction p = predictor.predict();
            json_response("prediction", p);

        } else if (cmd.find("\"quit\"") != std::string::npos) {
            break;

        } else {
            json_error("unknown_command");
        }
    }

    fflush(stdout);
    return 0;
}