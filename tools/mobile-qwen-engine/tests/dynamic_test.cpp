// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include "journal_mobile_qwen.h"
#include <cassert>
#include <cstdlib>
#include <csignal>
#include <dlfcn.h>
#include <filesystem>
#include <fstream>
#include <string>

// Simulate the app's other GGML family. Any Qwen call interposed by these
// process-global symbols must fail; the private image must bind its own closure.
extern "C" void llama_backend_init() { std::abort(); }
extern "C" void llama_backend_free() { std::abort(); }
extern "C" size_t ggml_backend_dev_count() { std::abort(); }

void app_sigpipe(int) {}

int main() {
    const auto prior_sigpipe = std::signal(SIGPIPE, app_sigpipe);
    assert(dlsym(RTLD_DEFAULT, "llama_backend_init") == reinterpret_cast<void *>(&llama_backend_init));
    assert(journal_mobile_qwen_abi_version() == 1);
    int marker = 0;
    const auto path = std::filesystem::temp_directory_path() / ("journal-qwen-dynamic-test-" + std::to_string(reinterpret_cast<uintptr_t>(&marker)));
    { std::ofstream file(path); file << "invalid GGUF; shared-image failure test"; }
    const char * token = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    for (uint64_t generation = 1; generation <= 2; ++generation) {
        journal_mobile_qwen_config_v1 config{1, sizeof(config), generation,
            journal_mobile_qwen_runtime_identity(), journal_mobile_qwen_source_revision(),
            "proved-test-model-generation", path.c_str(), path.c_str(), token};
        journal_mobile_qwen_handle * handle = nullptr;
        char error[512]{};
        assert(journal_mobile_qwen_create_v1(&config, &handle, error, sizeof(error)) == JOURNAL_MOBILE_QWEN_OK);
        assert(journal_mobile_qwen_run_v1(handle) == JOURNAL_MOBILE_QWEN_ENGINE_ERROR);
        journal_mobile_qwen_snapshot_v1 snapshot{1, sizeof(snapshot)};
        assert(journal_mobile_qwen_get_snapshot_v1(handle, &snapshot) == JOURNAL_MOBILE_QWEN_OK);
        assert(snapshot.generation == generation && snapshot.state == JOURNAL_MOBILE_QWEN_FAILED && snapshot.bound_port == 0);
        assert(journal_mobile_qwen_destroy_v1(handle) == JOURNAL_MOBILE_QWEN_OK);
    }
    std::filesystem::remove(path);
    assert(std::signal(SIGPIPE, prior_sigpipe) == app_sigpipe);
}
