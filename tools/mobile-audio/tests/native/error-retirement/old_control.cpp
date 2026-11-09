// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include "backend.hpp"
#include "ggml_graph.hpp"
#include "model_loader.hpp"
#include <cstdio>
#include <stdexcept>
#include <vector>
#include PARAKEET_TEST_BACKEND_SOURCE
#include PARAKEET_TEST_GRAPH_SOURCE

// Safe negative: real null-output failure, no crash, injected backend result,
// trained model or deliberately deadlocked old process.
int main() {
    pk::set_num_threads(0);
    bool threw = false;
    {
        pk::ModelLoader loader;
        std::vector<float> out{123};
        try { pk::run_graph(0, 1, [](ggml_context*) -> ggml_tensor* { return nullptr; }, out); }
        catch (const std::runtime_error&) { threw = true; }
    }
    const int retained_threads = pk::backend_thread_count();
    std::printf("old_null_output_threw=%d; after_final_loader_threads=%d; expected_retired=0\n",
                threw, retained_threads);
    pk::shutdown_backend();
    return threw && retained_threads == 0 ? 0 : 1;
}
