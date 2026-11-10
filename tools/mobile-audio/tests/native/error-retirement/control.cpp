// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include "backend.hpp"
#include "backend_pool.hpp"
#include "ggml_graph.hpp"
#include "model_loader.hpp"
#include "silero_vad.hpp"
#include "ggml.h"
#include "ggml-backend.h"
#include "ggml-cpu.h"
#include "gguf.h"
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdio>
#include <cstring>
#include <future>
#include <memory>
#include <stdexcept>
#include <thread>
#include <vector>

#ifndef NDEBUG
#error The caller contract must be exercised in release mode
#endif
namespace {
int failures = 0;
int backend_frees = 0;
bool check_restore = false, restored_before_free = false, free_observed_restore = false;
void observed_backend_free(ggml_backend_t backend) {
    if (check_restore) free_observed_restore = restored_before_free;
    ++backend_frees;
    ggml_backend_free(backend);
}
void observed_threads(ggml_backend_t backend, int count) {
    if (check_restore && count == 8) restored_before_free = true;
    ggml_backend_cpu_set_n_threads(backend, count);
}
}
// Transparent test-only observation of exact implementations. Every operation
// reaches real GGML; no failure/status/allocation result is injected.
#define ggml_backend_free observed_backend_free
#define ggml_backend_cpu_set_n_threads observed_threads
#include PARAKEET_TEST_BACKEND_SOURCE
#undef ggml_backend_cpu_set_n_threads
#undef ggml_backend_free
#include PARAKEET_TEST_GRAPH_SOURCE

#define CHECK(c) do { if (!(c)) { std::fprintf(stderr, "FAIL %d: %s\n", __LINE__, #c); ++failures; } } while (0)
template<class F> bool refused(F call) {
    try { call(); } catch (const std::runtime_error&) { return true; }
    return false;
}
static bool backend_present() { return pk::backend_state().backend != nullptr; }
static void valid_graph() {
    const float input[4] = {1, 2, 3, 4};
    std::vector<float> out;
    CHECK(pk::run_graph(0, 1, [&](ggml_context* ctx) {
        const int64_t shape[1] = {4};
        return ggml_scale(ctx, pk::graph_input_tensor(ctx, GGML_TYPE_F32, 1, shape, input, sizeof(input)), 2);
    }, out));
    CHECK(out == (std::vector<float>{2, 4, 6, 8}));
}
static void fail_graph() {
    std::vector<float> out{123};
    CHECK(refused([&] { pk::run_graph(0, 1, [](ggml_context*) -> ggml_tensor* { return nullptr; }, out); }));
    CHECK(out == std::vector<float>{123});
}
static void healthy_retention() {
    {
        pk::ModelLoader one;
        valid_graph();
    }
    CHECK(backend_present());
    CHECK(!pk::backend_state().retire_pending);
    CHECK(pk::backend_state().owners == 0);
    pk::shutdown_backend();
    CHECK(!backend_present());
    CHECK(pk::backend_thread_count() == 0);
}
static void owners_and_rejection() {
    auto first = std::make_unique<pk::ModelLoader>();
    auto second = std::make_unique<pk::ModelLoader>();
    valid_graph();
    const int frees = backend_frees;
    fail_graph();
    CHECK(backend_present());
    CHECK(pk::backend_state().retire_pending);
    CHECK(pk::backend_state().owners == 2);
    CHECK(pk::backend_state().borrows == 0);
    CHECK(refused(valid_graph));
    CHECK(refused([] { (void)pk::backend_device_name(); }));
    CHECK(refused([] { (void)pk::backend_is_cpu(); }));
    CHECK(refused([&] { pk::ensure_weights_realized(*second); }));
    pk::set_num_threads(2); // Preserve an override without reviving failed state.
    CHECK(backend_frees == frees);
    CHECK(pk::backend_state().threads == 8);
#ifdef PARAKEET_TEST_HAS_THREAD_RELEASE
    pk::release_backend_threads();
    CHECK(backend_frees == frees);
    CHECK(pk::backend_state().retire_pending);
#endif
    first.reset();
    CHECK(backend_present());
    CHECK(backend_frees == frees);
    second.reset();
    CHECK(!backend_present());
    CHECK(backend_frees == frees + 1);
    CHECK(pk::backend_thread_count() == 0);
    CHECK(!pk::backend_state().retire_pending);
#ifdef PARAKEET_TEST_HAS_THREAD_RELEASE
    pk::release_backend_threads();
    CHECK(!backend_present());
#endif
    valid_graph();
    CHECK(pk::num_threads() == 2 && pk::backend_thread_count() == 2);
    pk::set_num_threads(0);
    pk::shutdown_backend();
}
static void reentrant_last_owner() {
    valid_graph();
    const int frees = backend_frees;
    check_restore = true;
    restored_before_free = false;
    free_observed_restore = false;
    std::vector<float> out{123};
    CHECK(refused([&] {
        pk::run_graph(0, 1, [&](ggml_context*) -> ggml_tensor* {
            pk::ModelLoader loader; // Last owner dies inside the outer borrow.
            pk::ensure_weights_realized(loader); // Actual null-context failure.
            return nullptr;
        }, out);
    }));
    check_restore = false;
    CHECK(restored_before_free && free_observed_restore);
    CHECK(backend_frees == frees + 1);
    CHECK(pk::t_active == nullptr);
    CHECK(!backend_present());
    CHECK(pk::backend_state().owners == 0 && pk::backend_state().borrows == 0);
    valid_graph();
    pk::shutdown_backend();
}
static void concurrent_realization() {
    // Eager realization races a global graph-build realization. Both own their
    // loader until joined. No deliberate old-code deadlock or process signal.
    for (int iteration = 0; iteration < 16; ++iteration) {
        pk::ModelLoader eager, callback;
        std::promise<void> graph_entered, eager_entered;
        auto graph_ready = graph_entered.get_future();
        auto eager_ready = eager_entered.get_future();
        std::atomic<int> errors{0};
        std::thread graph([&] {
            if (refused([&] {
                std::vector<float> out;
                pk::run_graph(0, 1, [&](ggml_context*) -> ggml_tensor* {
                    graph_entered.set_value();
                    eager_ready.wait();
                    pk::ensure_weights_realized(callback);
                    return nullptr;
                }, out);
            })) ++errors;
        });
        graph_ready.wait();
        std::thread loading([&] {
            eager_entered.set_value();
            if (refused([&] { pk::ensure_weights_realized(eager); })) ++errors;
        });
        graph.join(); loading.join();
        CHECK(errors == 2);
        CHECK(pk::backend_state().owners == 2 && pk::backend_state().borrows == 0);
        CHECK(pk::backend_state().retire_pending);
    }
    CHECK(!backend_present());
    valid_graph(); pk::shutdown_backend();
}
static void pool_failure_is_local() {
    pk::ModelLoader weights;
    valid_graph();
    const int frees = backend_frees;
    auto pool = std::make_shared<pk::BackendPool>(1, 1);
    std::weak_ptr<pk::BackendPool> weak = pool;
    {
        pk::PoolLease lease(pool);
        pool.reset(); // The active lease owns this pool's remaining lifetime.
        fail_graph();
        CHECK(backend_frees == frees);
        CHECK(!pk::backend_state().retire_pending);
        valid_graph();
    }
    CHECK(weak.expired());
    CHECK(pk::current_route() == nullptr);
    CHECK(backend_present() && !pk::backend_state().retire_pending);
    valid_graph();
    pk::shutdown_backend(); // Deferred until this weight owner is gone.
    CHECK(backend_present());
}
static void silero_weight_owner() {
    // Synthetic zero weights, no trained model or inference: exercise real
    // Silero validation/allocation/uploads/destruction and its owned buffer.
    const char* path = "retirement-synthetic-silero.gguf";
    auto* meta = gguf_init_empty();
    gguf_set_val_str(meta, "general.architecture", "silero_vad");
    gguf_set_val_u32(meta, "silero_vad.lstm.hidden", 128);
    gguf_set_val_u32(meta, "silero_vad.encoder.n_layers", 4);
    gguf_set_val_u32(meta, "silero_vad.encoder.kernel", 3);
    const int32_t strides[]{1,2,2,1}, channels[]{128,64,64,128}, rates[]{16000};
    gguf_set_arr_data(meta,"silero_vad.encoder.strides",GGUF_TYPE_INT32,strides,4);
    gguf_set_arr_data(meta,"silero_vad.encoder.channels",GGUF_TYPE_INT32,channels,4);
    gguf_set_arr_data(meta,"silero_vad.sample_rates",GGUF_TYPE_INT32,rates,1);
    const auto* rate = pk::silero::rate_params(16000);
    gguf_set_val_u32(meta,"silero_vad.16000.chunk_samples",rate->chunk);
    gguf_set_val_u32(meta,"silero_vad.16000.context_samples",rate->context);
    gguf_set_val_u32(meta,"silero_vad.16000.stft.n_fft",rate->n_fft);
    gguf_set_val_u32(meta,"silero_vad.16000.stft.hop",rate->hop);
    gguf_set_val_u32(meta,"silero_vad.16000.stft.right_reflect_pad",rate->rpad);
    auto* tensors = ggml_init({4*1024*1024,nullptr,false});
    CHECK(tensors != nullptr);
    auto add = [&](std::string name, std::initializer_list<int64_t> shape) {
        auto* t = ggml_new_tensor(tensors,GGML_TYPE_F32,(int)shape.size(),shape.begin());
        ggml_set_name(t, ("vad16k."+name).c_str());
        std::memset(t->data,0,ggml_nbytes(t)); gguf_add_tensor(meta,t);
    };
    const int64_t bins=rate->n_fft/2+1, in[]{bins,128,64,64}, out[]{128,64,64,128};
    add("stft.forward_basis_buffer",{rate->n_fft,1,2*bins});
    for(int i=0;i<4;++i) {
        auto pre="encoder."+std::to_string(i)+".reparam_conv.";
        add(pre+"weight",{3,in[i],out[i]}); add(pre+"bias",{out[i]});
    }
    add("decoder.rnn.weight_ih",{128,512}); add("decoder.rnn.weight_hh",{128,512});
    add("decoder.rnn.bias_ih",{512}); add("decoder.rnn.bias_hh",{512});
    add("decoder.decoder.2.weight",{1,128,1}); add("decoder.decoder.2.bias",{1});
    CHECK(gguf_write_to_file(meta,path,false)); gguf_free(meta); ggml_free(tensors);
    std::string error;
    auto silero=pk::SileroVad::load(path,&error);
    CHECK(silero != nullptr);
    CHECK(error.empty());
    CHECK(pk::backend_state().owners == 1);
    fail_graph();
    CHECK(backend_present() && pk::backend_state().retire_pending);
    auto second=pk::SileroVad::load(path,&error);
    CHECK(!second && !error.empty());
    CHECK(pk::backend_state().owners == 1);
    silero.reset();
    CHECK(!backend_present());
    auto next=pk::SileroVad::load(path,&error);
    CHECK(next != nullptr);
    next.reset();
    CHECK(backend_present()); // Healthy warm retention remains unchanged.
    pk::shutdown_backend(); std::remove(path);
}
int main() {
    pk::set_num_threads(0);
    healthy_retention(); owners_and_rejection(); reentrant_last_owner();
    concurrent_realization(); pool_failure_is_local();
    CHECK(!backend_present());
    silero_weight_owner();
    CHECK(pk::backend_state().owners == 0 && pk::backend_state().borrows == 0);
    CHECK(!backend_present());
    if(failures) return 1;
    std::puts("NDEBUG real failures/owners/borrows/Restore/pool/concurrent realization/Silero synthetic weights/next valid passed");
    return 0;
}
