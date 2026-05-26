// lstm_predictor.cpp — COGNOS/OS behavior prediction LSTM C++ runtime.
// Production inference: loads ONNX model, runs MC dropout for uncertainty.
#include "lstm_predictor.h"
#include <algorithm>
#include <cmath>
#include <cstring>
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

// ── ORT status helper ────────────────────────────────────────────────────────

bool LstmPredictor::check_ort(OrtStatus* status, const char* context) {
    if (!status) return true;
    const char* msg = api_->GetErrorMessage(status);
    model_info_ = std::string(context) + ": " + msg;
    api_->ReleaseStatus(status);
    return false;
}

// ── Model I/O validation (C1 + C2) ──────────────────────────────────────────

bool LstmPredictor::validate_model_io() {
    OrtAllocator* alloc = nullptr;
    if (!check_ort(api_->GetAllocatorWithDefaultOptions(&alloc), "GetAllocator"))
        return false;

    // --- Input: expect 1 input named "sequence", shape [*, SEQ_LEN, FEATURE_DIM] ---

    size_t n_in = 0;
    if (!check_ort(api_->SessionGetInputCount(session_, &n_in), "GetInputCount"))
        return false;
    if (n_in != 1) {
        model_info_ = "model_mismatch: expected 1 input, got " + std::to_string(n_in);
        return false;
    }

    char* iname = nullptr;
    if (!check_ort(api_->SessionGetInputName(session_, 0, alloc, &iname), "GetInputName"))
        return false;
    bool name_ok = (std::strcmp(iname, "sequence") == 0);
    alloc->Free(alloc, iname);
    if (!name_ok) {
        model_info_ = "model_mismatch: expected input named 'sequence'";
        return false;
    }

    OrtTypeInfo* ti = nullptr;
    if (!check_ort(api_->SessionGetInputTypeInfo(session_, 0, &ti), "GetInputTypeInfo"))
        return false;
    const OrtTensorTypeAndShapeInfo* si = nullptr;
    if (!check_ort(api_->CastTypeInfoToTensorInfo(ti, &si), "CastInputTypeInfo")) {
        api_->ReleaseTypeInfo(ti);
        return false;
    }
    size_t rank = 0;
    if (!check_ort(api_->GetDimensionsCount(si, &rank), "GetInputRank")) {
        api_->ReleaseTypeInfo(ti);
        return false;
    }
    if (rank != 3) {
        model_info_ = "model_mismatch: expected 3D input, got " + std::to_string(rank) + "D";
        api_->ReleaseTypeInfo(ti);
        return false;
    }
    int64_t dims[3];
    if (!check_ort(api_->GetDimensions(si, dims, 3), "GetInputDims")) {
        api_->ReleaseTypeInfo(ti);
        return false;
    }
    api_->ReleaseTypeInfo(ti);

    if ((dims[1] != -1 && dims[1] != SEQ_LEN) ||
        (dims[2] != -1 && dims[2] != FEATURE_DIM)) {
        model_info_ = "model_mismatch: input shape incompatible, expected [*," +
            std::to_string(SEQ_LEN) + "," + std::to_string(FEATURE_DIM) + "]";
        return false;
    }

    // --- Outputs: expect 3 named "app_logits", "domain_logits", "confidence" ---

    size_t n_out = 0;
    if (!check_ort(api_->SessionGetOutputCount(session_, &n_out), "GetOutputCount"))
        return false;
    if (n_out != 3) {
        model_info_ = "model_mismatch: expected 3 outputs, got " + std::to_string(n_out);
        return false;
    }

    const char*   expected_names[]    = {"app_logits", "domain_logits", "confidence"};
    const int64_t expected_last_dim[] = {N_APP, N_DOMAIN, 1};

    for (size_t i = 0; i < 3; ++i) {
        char* oname = nullptr;
        if (!check_ort(api_->SessionGetOutputName(session_, i, alloc, &oname), "GetOutputName"))
            return false;
        bool ok = (std::strcmp(oname, expected_names[i]) == 0);
        alloc->Free(alloc, oname);
        if (!ok) {
            model_info_ = std::string("model_mismatch: output[") + std::to_string(i) +
                         "] expected '" + expected_names[i] + "'";
            return false;
        }

        OrtTypeInfo* oti = nullptr;
        if (!check_ort(api_->SessionGetOutputTypeInfo(session_, i, &oti), "GetOutputTypeInfo"))
            return false;
        const OrtTensorTypeAndShapeInfo* osi = nullptr;
        if (!check_ort(api_->CastTypeInfoToTensorInfo(oti, &osi), "CastOutputTypeInfo")) {
            api_->ReleaseTypeInfo(oti);
            return false;
        }
        size_t odim_count = 0;
        if (!check_ort(api_->GetDimensionsCount(osi, &odim_count), "GetOutputRank")) {
            api_->ReleaseTypeInfo(oti);
            return false;
        }
        if (odim_count >= 1) {
            std::vector<int64_t> odims(odim_count);
            if (!check_ort(api_->GetDimensions(osi, odims.data(), odim_count), "GetOutputDims")) {
                api_->ReleaseTypeInfo(oti);
                return false;
            }
            int64_t last = odims[odim_count - 1];
            if (last != -1 && last != expected_last_dim[i]) {
                model_info_ = std::string("model_mismatch: '") + expected_names[i] +
                             "' last dim=" + std::to_string(last) +
                             ", expected " + std::to_string(expected_last_dim[i]);
                api_->ReleaseTypeInfo(oti);
                return false;
            }
        }
        api_->ReleaseTypeInfo(oti);
    }

    return true;
}

// ── Input tensor management (B1) ─────────────────────────────────────────────

bool LstmPredictor::rebuild_input_tensor() {
    if (input_tensor_) {
        api_->ReleaseValue(input_tensor_);
        input_tensor_ = nullptr;
    }
    int64_t shape[] = {1, SEQ_LEN, FEATURE_DIM};
    return check_ort(
        api_->CreateTensorWithDataAsOrtValue(
            mem_info_, input_data_.data(),
            input_data_.size() * sizeof(float),
            shape, 3, ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,
            &input_tensor_),
        "CreateInputTensor");
}

// ── Constructor (A2 + A3 hardened) ───────────────────────────────────────────

LstmPredictor::LstmPredictor(const std::string& model_path)
    : input_data_(SEQ_LEN * FEATURE_DIM, 0.0f)
    , app_out_(N_APP, 0.0f)
    , domain_out_(N_DOMAIN, 0.0f)
    , conf_out_(1, 0.0f)
{
    const OrtApiBase* api_base = OrtGetApiBase();
    if (!api_base) {
        model_info_ = "init_failed: OrtGetApiBase() returned null";
        return;
    }
    api_ = api_base->GetApi(ORT_API_VERSION);
    if (!api_) {
        model_info_ = "init_failed: GetApi() returned null (ABI version mismatch)";
        return;
    }

    if (!check_ort(api_->CreateEnv(ORT_LOGGING_LEVEL_WARNING, "cognos_lstm", &env_),
                   "CreateEnv"))
        return;

    if (!check_ort(api_->CreateSessionOptions(&opts_), "CreateSessionOptions"))
        return;
    if (!check_ort(api_->SetIntraOpNumThreads(opts_, 1), "SetIntraOpNumThreads"))
        return;
    if (!check_ort(api_->SetInterOpNumThreads(opts_, 1), "SetInterOpNumThreads"))
        return;
    if (!check_ort(api_->EnableMemPattern(opts_), "EnableMemPattern"))
        return;
    if (!check_ort(api_->EnableCpuMemArena(opts_), "EnableCpuMemArena"))
        return;

    if (!check_ort(api_->CreateCpuMemoryInfo(OrtArenaAllocator, OrtMemTypeDefault, &mem_info_),
                   "CreateCpuMemoryInfo"))
        return;

    if (!check_ort(api_->CreateSession(env_, model_path.c_str(), opts_, &session_),
                   "CreateSession")) {
        session_ = nullptr;
        return;
    }

    if (!validate_model_io()) {
        api_->ReleaseSession(session_);
        session_ = nullptr;
        return;
    }

    if (!check_ort(api_->CreateRunOptions(&run_opts_), "CreateRunOptions")) {
        api_->ReleaseSession(session_);
        session_ = nullptr;
        return;
    }

    if (!rebuild_input_tensor()) {
        api_->ReleaseSession(session_);
        session_ = nullptr;
        return;
    }

    model_info_ = "loaded:" + model_path;
}

// ── Destructor (E2: release in reverse order; OrtEnv last) ───────────────────

LstmPredictor::~LstmPredictor() {
    if (!api_) return;
    if (input_tensor_) api_->ReleaseValue(input_tensor_);
    if (run_opts_)     api_->ReleaseRunOptions(run_opts_);
    if (session_)      api_->ReleaseSession(session_);
    if (mem_info_)     api_->ReleaseMemoryInfo(mem_info_);
    if (opts_)         api_->ReleaseSessionOptions(opts_);
    if (env_)          api_->ReleaseEnv(env_);
}

// ── Event buffer ─────────────────────────────────────────────────────────────

void LstmPredictor::push_event(const Event& event) {
    ring_.push_back(event.to_array());
    if ((int)ring_.size() > SEQ_LEN) ring_.pop_front();
}

// ── Prediction (D6: tolerates failed MC passes) ──────────────────────────────

Prediction LstmPredictor::predict() {
    Prediction result{};

    if (!is_loaded() || (int)ring_.size() < 5) {
        std::fill(result.app_probs,    result.app_probs    + N_APP,    1.0f/N_APP);
        std::fill(result.domain_probs, result.domain_probs + N_DOMAIN, 1.0f/N_DOMAIN);
        result.confidence     = 0.0f;
        result.should_preload = false;
        return result;
    }

    std::fill(input_data_.begin(), input_data_.end(), 0.0f);
    int offset = SEQ_LEN - (int)ring_.size();
    for (int i = 0; i < (int)ring_.size(); ++i) {
        const auto& row = ring_[i];
        std::copy(row.begin(), row.end(),
                  input_data_.begin() + (offset + i) * FEATURE_DIM);
    }

    float app_sum[N_APP]      = {};
    float dom_sum[N_DOMAIN]   = {};
    float app_tops[MC_PASSES] = {};
    float dom_tops[MC_PASSES] = {};
    int   good_passes         = 0;

    for (int pass = 0; pass < MC_PASSES; ++pass) {
        if (!run_once(app_out_.data(), domain_out_.data(), conf_out_.data()))
            continue;

        for (int j = 0; j < N_APP;    ++j) app_sum[j] += app_out_[j];
        for (int j = 0; j < N_DOMAIN; ++j) dom_sum[j] += domain_out_[j];

        app_tops[good_passes] = *std::max_element(app_out_.begin(), app_out_.end());
        dom_tops[good_passes] = *std::max_element(domain_out_.begin(), domain_out_.end());
        ++good_passes;
    }

    if (good_passes == 0) {
        std::fill(result.app_probs,    result.app_probs    + N_APP,    1.0f/N_APP);
        std::fill(result.domain_probs, result.domain_probs + N_DOMAIN, 1.0f/N_DOMAIN);
        result.confidence     = 0.0f;
        result.should_preload = false;
        return result;
    }

    for (int j = 0; j < N_APP;    ++j) result.app_probs[j]    = app_sum[j]  / good_passes;
    for (int j = 0; j < N_DOMAIN; ++j) result.domain_probs[j] = dom_sum[j] / good_passes;

    auto std_of = [](float* arr, int n) {
        float mean = 0.0f;
        for (int i = 0; i < n; ++i) mean += arr[i];
        mean /= n;
        float var = 0.0f;
        for (int i = 0; i < n; ++i) var += (arr[i]-mean)*(arr[i]-mean);
        return std::sqrt(var / n);
    };

    float std_app = std_of(app_tops, good_passes);
    float std_dom = std_of(dom_tops, good_passes);
    result.confidence = 1.0f - (std_app + std_dom) * 0.5f;
    result.confidence = std::max(0.0f, std::min(1.0f, result.confidence));
    result.should_preload = result.confidence >= CONF_THRESHOLD;

    return result;
}

// ── Single inference pass (A1 hardened, B3 pre-allocated tensor) ─────────────

bool LstmPredictor::run_once(float* app_p, float* dom_p, float* conf_p) {
    const char* input_names[]  = {"sequence"};
    const char* output_names[] = {"app_logits", "domain_logits", "confidence"};
    OrtValue*   outputs[3]     = {nullptr, nullptr, nullptr};

    OrtStatus* status = api_->Run(
        session_, run_opts_,
        input_names, &input_tensor_, 1,
        output_names, 3, outputs);
    if (status) {
        last_error_ = std::string("Run: ") + api_->GetErrorMessage(status);
        api_->ReleaseStatus(status);
        return false;
    }

    float* app_data  = nullptr;
    float* dom_data  = nullptr;
    float* conf_data = nullptr;
    bool ok = true;

    status = api_->GetTensorMutableData(outputs[0], (void**)&app_data);
    if (status) {
        last_error_ = std::string("GetTensorData[app]: ") + api_->GetErrorMessage(status);
        api_->ReleaseStatus(status);
        ok = false;
    }
    if (ok) {
        status = api_->GetTensorMutableData(outputs[1], (void**)&dom_data);
        if (status) {
            last_error_ = std::string("GetTensorData[dom]: ") + api_->GetErrorMessage(status);
            api_->ReleaseStatus(status);
            ok = false;
        }
    }
    if (ok) {
        status = api_->GetTensorMutableData(outputs[2], (void**)&conf_data);
        if (status) {
            last_error_ = std::string("GetTensorData[conf]: ") + api_->GetErrorMessage(status);
            api_->ReleaseStatus(status);
            ok = false;
        }
    }

    if (ok) {
        std::copy(app_data, app_data + N_APP, app_p);
        std::copy(dom_data, dom_data + N_DOMAIN, dom_p);
        conf_p[0] = conf_data[0];
        softmax(app_p, N_APP);
        softmax(dom_p, N_DOMAIN);
    }

    for (auto* v : outputs) if (v) api_->ReleaseValue(v);
    return ok;
}

// ── Softmax ──────────────────────────────────────────────────────────────────

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