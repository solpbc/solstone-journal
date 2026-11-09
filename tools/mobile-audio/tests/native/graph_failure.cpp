// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include "backend.hpp"
#include "backend_pool.hpp"
#include "ggml.h"
#include "ggml_graph.hpp"

#include <cassert>
#include <cstdio>
#include <memory>
#include <stdexcept>
#include <vector>

#ifndef NDEBUG
#error This control must exercise the release-mode caller contract
#endif

static int failures = 0;
#define CHECK(c) do { if (!(c)) { std::fprintf(stderr, "FAIL line %d: %s\n", __LINE__, #c); ++failures; } } while (0)

static void failure_is_not_consumed() {
    std::vector<float> output{123.0f};
    std::vector<float> captured{456.0f};
    bool consumed_stale_output = false;
    bool threw = false;
    try {
        const bool ok = pk::run_graph(0, 1, [&](ggml_context* ctx) -> ggml_tensor* {
            // Existing real Backend::compute failure seam: graph construction
            // yields no output. This is not an injected Metal/OOM result.
            pk::capture_graph_output(ggml_new_tensor_1d(ctx, GGML_TYPE_F32, 1), &captured);
            return nullptr;
        }, output);
        assert(ok && "graph failed"); // Same release-mode check as native callers.
        (void)ok;
        consumed_stale_output = output.at(0) == 123.0f;
    } catch (const std::runtime_error&) {
        threw = true;
    }
    CHECK(threw);
    CHECK(!consumed_stale_output);
    CHECK(output == std::vector<float>{123.0f});
    CHECK(captured == std::vector<float>{456.0f});
}

static void valid_graph() {
    std::vector<float> output;
    const float input[4] = {1, 2, 3, 4};
    CHECK(pk::run_graph(0, 1, [&](ggml_context* ctx) -> ggml_tensor* {
        const int64_t shape[1] = {4};
        auto* tensor = pk::graph_input_tensor(ctx, GGML_TYPE_F32, 1, shape, input, sizeof(input));
        return ggml_scale(ctx, tensor, 2.0f);
    }, output));
    CHECK(output == (std::vector<float>{2, 4, 6, 8}));
}

int main() {
    // Global route: failure unwinds the scoped thread override and mutex.
    pk::set_num_threads(0);
    valid_graph();
    const int original_threads = pk::backend_thread_count();
    failure_is_not_consumed();
    CHECK(pk::backend_thread_count() == original_threads);
    valid_graph();
    pk::shutdown_backend(); // No model or pool holds weights in this control.
    CHECK(pk::backend_thread_count() == 0);
    valid_graph(); // A fresh global backend still computes correctly.
    pk::shutdown_backend();

    // Pool route: failure releases its lock and lease; subsequent work uses the
    // same slot successfully, then the last owner destroys the pool.
    auto pool = std::make_shared<pk::BackendPool>(1, 1);
    std::weak_ptr<pk::BackendPool> weak_pool = pool;
    {
        pk::PoolLease lease(pool);
        failure_is_not_consumed();
        valid_graph();
    }
    CHECK(pk::current_route() == nullptr);
    CHECK(pool->in_use() == 0);
    CHECK(pool->total_runs() == 2);
    pool.reset();
    CHECK(weak_pool.expired());
    if (failures) return 1;
    std::puts("release-mode graph failures throw before output use; global and pooled valid graphs recover");
    return 0;
}
