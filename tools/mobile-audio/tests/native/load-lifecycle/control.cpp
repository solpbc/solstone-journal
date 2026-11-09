// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include <cstdio>
#include <memory>
#include <new>
#include <set>
#include <stdexcept>
#include <string>

// These are caller-contract doubles, not a native model or allocator fixture.
// The four projected function bodies below remain byte-exact source text.
enum class Callee { Valid, Null, Throw };
static Callee callee = Callee::Valid;
static bool weight_failure = false;
static int live_loaders = 0;
static int failures = 0;
#define PK_LOG(...) do {} while (0)
#define CHECK(c) do { if (!(c)) { std::fprintf(stderr, "FAIL %d: %s\n", __LINE__, #c); ++failures; } } while (0)

namespace pk {
struct ParakeetConfig {
    std::string arch;
    struct { bool present = false; } ternary;
    struct { bool present = true; } vad, diarization;
};
struct ModelLoader {
    ParakeetConfig cfg;
    bool prepared = false;
    ModelLoader() { ++live_loaders; }
    ~ModelLoader() { --live_loaders; }
    bool load(const std::string& path) {
        if (callee == Callee::Throw) throw std::runtime_error("test-only callee failure");
        cfg.arch = path == "diar" ? "diarization" : path;
        return callee != Callee::Null;
    }
    bool load_component(const std::string& path, const std::string&) { return load(path); }
    const ParakeetConfig& config() const { return cfg; }
    bool has_tensor_with_suffix(const std::string&) const { return false; }
    bool weights_realized() const { return prepared; }
};
static void ensure_weights_realized(ModelLoader& loader) {
    if (weight_failure) throw std::runtime_error("test-only weight failure");
    loader.prepared = true;
}
static std::string ternary_flag_consistency_error(const ModelLoader&) { return ""; }
static void ternary_prepare(const ModelLoader&) {}
struct Backend { const char* device_name() const { return "cpu"; } };
static Backend& global_backend() { static Backend backend; return backend; }
struct Component { explicit Component(const ModelLoader&) {} };
using MelFrontend = Component;
using DiarizationEncoder = Component;
using DiarizationHead = Component;
class Model {
public:
    static std::unique_ptr<Model> load(const std::string&, const std::string& = "");
    static std::unique_ptr<Model> load_vad_only(const std::string&, const std::string& = "");
private:
    Model() = default;
    ModelLoader loader_;
};
class DiarizationModel {
public:
    static std::unique_ptr<DiarizationModel> load(const std::string&, const std::string& = "");
private:
    DiarizationModel() = default;
    ModelLoader loader_;
    std::unique_ptr<Component> mel_, encoder_, head_;
};
struct Auxiliary {
    struct Fingerprint { std::string weights = "test-only"; };
    Fingerprint fingerprint() const { return {}; }
    static std::unique_ptr<Auxiliary> load(const std::string&) {
        if (callee == Callee::Throw) throw std::runtime_error("test-only callee failure");
        return callee == Callee::Null ? nullptr : std::make_unique<Auxiliary>();
    }
    static std::unique_ptr<Auxiliary> load(const std::string& path, std::string*) { return load(path); }
};
using CedTagger = Auxiliary;
using SileroVad = Auxiliary;
using SpeakerEncoder = Auxiliary;
static bool gguf_is_ced(const char* path) { return std::string(path) == "ced"; }
static bool gguf_is_silero(const char* path) { return std::string(path) == "silero"; }
static bool gguf_is_vad_only(const char* path) { return std::string(path) == "vad"; }
static bool gguf_is_voicedetect(const char* path) { return std::string(path) == "voice"; }
}
struct parakeet_ctx;
static std::set<parakeet_ctx*> live_contexts;
struct parakeet_ctx {
    std::unique_ptr<pk::Model> model, vad_model;
    std::unique_ptr<pk::DiarizationModel> diar;
    std::unique_ptr<pk::Auxiliary> tagger, speaker, silero;
    std::string speaker_identity;
    parakeet_ctx() { live_contexts.insert(this); }
    ~parakeet_ctx() { live_contexts.erase(this); }
};

#include LOAD_SOURCE_INCLUDE

static void check_no_context_leak() {
    CHECK(live_contexts.empty());
    // Bound the old negative's leaked TEST objects after recording the failure.
    while (!live_contexts.empty()) delete *live_contexts.begin();
    CHECK(live_loaders == 0);
}

int main() {
    weight_failure = true;
    auto eager = [](auto load) {
        bool threw = false, returned = false;
        try { returned = bool(load()); } catch (...) { threw = true; }
        CHECK(!threw);
        CHECK(!returned);
        CHECK(live_loaders == 0);
    };
    eager([] { return pk::Model::load("asr"); });
    eager([] { return pk::Model::load_vad_only("vad"); });
    eager([] { return pk::DiarizationModel::load("diar"); });
    weight_failure = false;
    callee = Callee::Throw;
    eager([] { return pk::Model::load("asr"); });
    eager([] { return pk::Model::load_vad_only("vad"); });
    eager([] { return pk::DiarizationModel::load("diar"); });
    callee = Callee::Valid;
    CHECK(load_plain(nullptr) == nullptr);
    for (const char* path : {"ced", "silero", "vad", "voice", "asr", "diar"}) {
        callee = Callee::Throw;
        CHECK(load_plain(path) == nullptr);
        check_no_context_leak();
        callee = Callee::Null;
        CHECK(load_plain(path) == nullptr);
        check_no_context_leak();
        callee = Callee::Valid;
        auto* ctx = load_plain(path);
        CHECK(ctx != nullptr);
        CHECK(live_contexts.size() == 1);
        delete ctx;
        check_no_context_leak();
    }
    return failures ? 1 : 0;
}
