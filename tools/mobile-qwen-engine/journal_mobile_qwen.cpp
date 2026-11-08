// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include "journal_mobile_qwen.h"
#include "common.h"
#include "server-embedded.h"

#include <algorithm>
#include <cstring>
#include <filesystem>
#include <memory>
#include <mutex>
#include <string>

#ifndef JOURNAL_MOBILE_QWEN_RUNTIME_IDENTITY
#error "The producer must bind a runtime identity to the native adapter"
#endif
#ifndef JOURNAL_MOBILE_QWEN_SOURCE_REVISION
#error "The producer must bind the pinned llama source revision"
#endif

struct journal_mobile_qwen_handle {
    std::mutex mutex;
    server_embedded_control control;
    std::string model_path;
    std::string projector_path;
    std::string model_generation;
    std::string token;
    uint64_t generation;
    journal_mobile_qwen_state state = JOURNAL_MOBILE_QWEN_CREATED;
    uint32_t port = 0;
    int32_t engine_result = 0;
    std::string error;
    bool run_started = false;
    bool running = false;
};

namespace {
std::mutex singleton_mutex;
journal_mobile_qwen_handle * singleton = nullptr;

void copy_error(char * destination, size_t capacity, const char * text) {
    if (!destination || capacity == 0) { return; }
    const size_t count = std::min(capacity - 1, std::strlen(text));
    std::memcpy(destination, text, count);
    destination[count] = '\0';
}
bool regular_absolute_file(const char * value) {
    if (!value || !*value) { return false; }
    std::error_code error;
    const std::filesystem::path path(value);
    return path.is_absolute() && std::filesystem::is_regular_file(path, error) && !error;
}
bool valid_token(const char * value) {
    if (!value || std::strlen(value) != 64) { return false; }
    return std::all_of(value, value + 64, [](char c) {
        return (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f');
    });
}
int32_t invalid(char * error, size_t capacity, const char * message) {
    copy_error(error, capacity, message);
    return JOURNAL_MOBILE_QWEN_INVALID;
}
}

extern "C" uint32_t journal_mobile_qwen_abi_version(void) { return JOURNAL_MOBILE_QWEN_ABI_V1; }
extern "C" const char * journal_mobile_qwen_source_revision(void) { return JOURNAL_MOBILE_QWEN_SOURCE_REVISION; }
extern "C" const char * journal_mobile_qwen_runtime_identity(void) { return JOURNAL_MOBILE_QWEN_RUNTIME_IDENTITY; }

extern "C" int32_t journal_mobile_qwen_create_v1(const journal_mobile_qwen_config_v1 * config,
    journal_mobile_qwen_handle ** out, char * error, size_t capacity) {
    try {
        if (!out) { return invalid(error, capacity, "missing output handle"); }
        *out = nullptr;
        if (!config || config->abi_version != JOURNAL_MOBILE_QWEN_ABI_V1 || config->struct_size != sizeof(*config)) {
            return invalid(error, capacity, "native Qwen ABI mismatch");
        }
        if (!config->expected_runtime_identity || std::strcmp(config->expected_runtime_identity, journal_mobile_qwen_runtime_identity()) != 0 ||
            !config->expected_source_revision || std::strcmp(config->expected_source_revision, journal_mobile_qwen_source_revision()) != 0) {
            return invalid(error, capacity, "native Qwen package/source identity mismatch");
        }
        if (config->generation == 0 || !config->model_generation || !*config->model_generation ||
            !regular_absolute_file(config->model_path) || !regular_absolute_file(config->projector_path)) {
            return invalid(error, capacity, "native Qwen requires a proved model generation and absolute model files");
        }
        if (!valid_token(config->bearer_token)) { return invalid(error, capacity, "native Qwen requires a fresh 32-byte hex bearer secret"); }
        std::lock_guard<std::mutex> lock(singleton_mutex);
        if (singleton) { copy_error(error, capacity, "native Qwen generation already exists"); return JOURNAL_MOBILE_QWEN_BUSY; }
        auto handle = std::make_unique<journal_mobile_qwen_handle>();
        handle->model_path = config->model_path;
        handle->projector_path = config->projector_path;
        handle->model_generation = config->model_generation;
        handle->token = config->bearer_token;
        handle->generation = config->generation;
        auto * ptr = handle.get();
        handle->control.on_ready = [ptr](int port) {
            std::lock_guard<std::mutex> ready_lock(ptr->mutex);
            if (!ptr->control.cancelled.load() && port > 0 && port <= 65535) {
                ptr->port = static_cast<uint32_t>(port);
                ptr->state = JOURNAL_MOBILE_QWEN_READY;
            }
        };
        singleton = handle.release();
        *out = singleton;
        copy_error(error, capacity, "");
        return JOURNAL_MOBILE_QWEN_OK;
    } catch (...) {
        copy_error(error, capacity, "native Qwen handle allocation failed");
        return JOURNAL_MOBILE_QWEN_ENGINE_ERROR;
    }
}

extern "C" int32_t journal_mobile_qwen_run_v1(journal_mobile_qwen_handle * handle) {
    if (!handle) { return JOURNAL_MOBILE_QWEN_INVALID; }
    {
        std::lock_guard<std::mutex> lock(handle->mutex);
        if (handle->run_started) { return JOURNAL_MOBILE_QWEN_BUSY; }
        handle->run_started = true;
        handle->running = true;
        handle->state = handle->control.cancelled.load() ? JOURNAL_MOBILE_QWEN_STOPPING : JOURNAL_MOBILE_QWEN_LOADING;
    }
    int result = 1;
    try {
        common_params params;
        params.model.path = handle->model_path;
        params.mmproj.path = handle->projector_path;
        params.model_alias = {"local/qwen3.5-4b"};
        params.hostnames = {"127.0.0.1"};
        params.port = 0;
        params.api_keys = {handle->token};
        params.offline = true;
        params.fit_params = false;
        params.n_ctx = 32768;
        params.n_parallel = 1;
        params.n_batch = 128;
        params.n_ubatch = 128;
        params.cpuparams.n_threads = 4;
        params.cpuparams_batch.n_threads = 4;
        params.n_threads_http = 4;
        params.cache_ram_mib = 0;
        params.cache_ram_explicit = true;
        params.default_template_kwargs["enable_thinking"] = "false";
        params.sampling.temp = 0.6f;
        params.sampling.top_k = 20;
        params.sampling.top_p = 0.95f;
        params.sampling.min_p = 0.0f;
        params.sampling.penalty_repeat = 1.0f;
#if defined(JOURNAL_MOBILE_QWEN_METAL)
        params.n_gpu_layers = 999;
#else
        params.n_gpu_layers = 0;
        params.mmproj_use_gpu = false;
#endif
        result = llama_server_embedded(params, handle->control);
    } catch (...) {
        // Upstream exception strings can contain private paths or request text.
        // Keep the ABI error bounded and independent of owner content/secrets.
        result = 1;
    }
    {
        std::lock_guard<std::mutex> lock(handle->mutex);
        handle->engine_result = result;
        handle->port = 0;
        handle->state = result == 0 ? JOURNAL_MOBILE_QWEN_STOPPED : JOURNAL_MOBILE_QWEN_FAILED;
        if (result != 0) { handle->error = "native Qwen load or serving failed"; }
        handle->running = false;
    }
    return result == 0 ? JOURNAL_MOBILE_QWEN_OK : JOURNAL_MOBILE_QWEN_ENGINE_ERROR;
}

extern "C" int32_t journal_mobile_qwen_request_stop_v1(journal_mobile_qwen_handle * handle) {
    if (!handle) { return JOURNAL_MOBILE_QWEN_INVALID; }
    {
        std::lock_guard<std::mutex> lock(handle->mutex);
        if (handle->state == JOURNAL_MOBILE_QWEN_STOPPED || handle->state == JOURNAL_MOBILE_QWEN_FAILED) {
            return JOURNAL_MOBILE_QWEN_OK;
        }
        handle->state = JOURNAL_MOBILE_QWEN_STOPPING;
        handle->port = 0;
        // Latch under the same state lock used by on_ready, so READY cannot be
        // published between revocation and the upstream cancellation request.
        handle->control.cancelled.store(true);
    }
    handle->control.request_stop();
    return JOURNAL_MOBILE_QWEN_OK;
}

extern "C" int32_t journal_mobile_qwen_get_snapshot_v1(journal_mobile_qwen_handle * handle,
    journal_mobile_qwen_snapshot_v1 * snapshot) {
    if (!handle || !snapshot || snapshot->abi_version != JOURNAL_MOBILE_QWEN_ABI_V1 || snapshot->struct_size != sizeof(*snapshot)) {
        return JOURNAL_MOBILE_QWEN_INVALID;
    }
    std::lock_guard<std::mutex> lock(handle->mutex);
    snapshot->generation = handle->generation;
    snapshot->state = handle->state;
    snapshot->bound_port = handle->port;
    snapshot->engine_result = handle->engine_result;
    copy_error(snapshot->error, sizeof(snapshot->error), handle->error.c_str());
    return JOURNAL_MOBILE_QWEN_OK;
}

extern "C" int32_t journal_mobile_qwen_destroy_v1(journal_mobile_qwen_handle * handle) {
    if (!handle) { return JOURNAL_MOBILE_QWEN_INVALID; }
    std::lock_guard<std::mutex> singleton_lock(singleton_mutex);
    if (singleton != handle) { return JOURNAL_MOBILE_QWEN_INVALID; }
    {
        std::lock_guard<std::mutex> lock(handle->mutex);
        if (handle->running) { return JOURNAL_MOBILE_QWEN_BUSY; }
    }
    singleton = nullptr;
    delete handle;
    return JOURNAL_MOBILE_QWEN_OK;
}
