// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include "llama.h"
#include "llama-model.h"
#include "ggml.h"
#include "ggml-alloc.h"
#include "ggml-backend.h"
#include "ggml-cpu.h"
#include "gguf.h"

#include <cmath>
#include <cstdio>
#include <cstring>
#include <memory>
#include <string>
#include <vector>

// Real native loading of tiny synthetic Q4_K weights. No trained model,
// tokenizer, server, token generation or production test flag is involved.
static int failures = 0;
#define CHECK(c) do { if (!(c)) { std::fprintf(stderr, "FAIL %d: %s\n", __LINE__, #c); ++failures; } } while (0)
constexpr int embedding = 256, vocabulary = 8;

static bool write_fixture(const char* path, bool untied, bool bad_shape = false) {
    gguf_context* file = gguf_init_empty();
    ggml_context* tensors = ggml_init({16 * 1024 * 1024, nullptr, false});
    if (!file || !tensors) return false;
    gguf_set_val_str(file, "general.architecture", "qwen35");
    gguf_set_val_str(file, "tokenizer.ggml.model", "none");
    gguf_set_val_u32(file, "qwen35.vocab_size", vocabulary);
    gguf_set_val_u32(file, "qwen35.context_length", 32);
    gguf_set_val_u32(file, "qwen35.embedding_length", embedding);
    gguf_set_val_u32(file, "qwen35.block_count", 1);
    gguf_set_val_u32(file, "qwen35.feed_forward_length", embedding);
    gguf_set_val_u32(file, "qwen35.attention.head_count", 4);
    gguf_set_val_u32(file, "qwen35.attention.head_count_kv", 4);
    gguf_set_val_f32(file, "qwen35.attention.layer_norm_rms_epsilon", 1e-6f);
    const int32_t sections[4] = {16, 16, 16, 16};
    gguf_set_arr_data(file, "qwen35.rope.dimension_sections", GGUF_TYPE_INT32, sections, 4);
    gguf_set_val_u32(file, "qwen35.ssm.conv_kernel", 4);
    gguf_set_val_u32(file, "qwen35.ssm.inner_size", embedding);
    gguf_set_val_u32(file, "qwen35.ssm.state_size", 64);
    gguf_set_val_u32(file, "qwen35.ssm.time_step_rank", 4);
    gguf_set_val_u32(file, "qwen35.ssm.group_count", 4);
    gguf_set_val_u32(file, "qwen35.full_attention_interval", 1);
    auto add = [&](const char* name, ggml_type type, int64_t columns, int64_t rows, float scale) {
        auto* tensor = ggml_new_tensor_2d(tensors, type, columns, rows);
        ggml_set_name(tensor, name);
        std::vector<float> values(columns * rows);
        for (int64_t row = 0; row < rows; ++row)
            for (int64_t col = 0; col < columns; ++col)
                values[row * columns + col] = scale * (row + 1);
        if (type == GGML_TYPE_F32) std::memcpy(tensor->data, values.data(), ggml_nbytes(tensor));
        else CHECK(ggml_quantize_chunk(type, values.data(), tensor->data, 0, rows, columns, nullptr)
                   == ggml_nbytes(tensor));
        gguf_add_tensor(file, tensor);
    };
    add("token_embd.weight", GGML_TYPE_Q4_K, embedding, vocabulary, 0.125f);
    add("output_norm.weight", GGML_TYPE_F32, embedding, 1, 1);
    if (untied) add("output.weight", GGML_TYPE_Q4_K, embedding, bad_shape ? vocabulary - 1 : vocabulary, 0.25f);
    add("blk.0.attn_norm.weight", GGML_TYPE_F32, embedding, 1, 1);
    add("blk.0.post_attention_norm.weight", GGML_TYPE_F32, embedding, 1, 1);
    add("blk.0.attn_q.weight", GGML_TYPE_F32, embedding, embedding * 2, 0);
    add("blk.0.attn_k.weight", GGML_TYPE_F32, embedding, embedding, 0);
    add("blk.0.attn_v.weight", GGML_TYPE_F32, embedding, embedding, 0);
    add("blk.0.attn_output.weight", GGML_TYPE_F32, embedding, embedding, 0);
    add("blk.0.attn_q_norm.weight", GGML_TYPE_F32, 64, 1, 1);
    add("blk.0.attn_k_norm.weight", GGML_TYPE_F32, 64, 1, 1);
    add("blk.0.ffn_gate.weight", GGML_TYPE_F32, embedding, embedding, 0);
    add("blk.0.ffn_down.weight", GGML_TYPE_F32, embedding, embedding, 0);
    add("blk.0.ffn_up.weight", GGML_TYPE_F32, embedding, embedding, 0);
    const bool ok = gguf_write_to_file(file, path, false);
    gguf_free(file); ggml_free(tensors);
    return ok;
}

static void check_projection(ggml_tensor* weights, float scale) {
    auto* backend = ggml_backend_cpu_init();
    auto* ctx = ggml_init({1024 * 1024, nullptr, true});
    if (!backend || !ctx) { CHECK(false); return; }
    ggml_backend_cpu_set_n_threads(backend, 4);
    auto* input = ggml_new_tensor_2d(ctx, GGML_TYPE_F32, embedding, 1);
    ggml_set_input(input);
    auto* output = ggml_mul_mat(ctx, weights, input);
    ggml_set_output(output);
    auto* graph = ggml_new_graph(ctx);
    ggml_build_forward_expand(graph, output);
    auto* allocator = ggml_gallocr_new(ggml_backend_get_default_buffer_type(backend));
    const bool allocated = ggml_gallocr_alloc_graph(allocator, graph);
    CHECK(allocated);
    if (allocated) {
        std::vector<float> values(embedding, 1.0f / embedding);
        ggml_backend_tensor_set(input, values.data(), 0, values.size() * sizeof(float));
        CHECK(ggml_backend_graph_compute(backend, graph) == GGML_STATUS_SUCCESS);
        std::vector<float> result(vocabulary);
        ggml_backend_tensor_get(output, result.data(), 0, result.size() * sizeof(float));
        for (int row = 0; row < vocabulary; ++row) CHECK(std::fabs(result[row] - scale * (row + 1)) < 0.002f);
    }
    ggml_gallocr_free(allocator); ggml_free(ctx); ggml_backend_free(backend);
}

using Model = std::unique_ptr<llama_model, decltype(&llama_model_free)>;
static Model load(const char* path) {
    auto params = llama_model_default_params();
    params.n_gpu_layers = 0; // This CPU host control is not a target policy.
    params.load_mode = LLAMA_LOAD_MODE_MMAP;
    params.use_extra_bufts = true;
    return Model(llama_model_load_from_file(path, params), llama_model_free);
}

int main() {
    llama_backend_init();
    CHECK(write_fixture("tied.gguf", false));
    CHECK(write_fixture("untied.gguf", true));
    CHECK(write_fixture("bad-untied.gguf", true, true));
    for (int generation = 0; generation < 2; ++generation) {
        auto tied = load("tied.gguf");
        CHECK(tied != nullptr);
        if (tied) {
            CHECK(tied->output == tied->tok_embd);
            std::printf("tied generation=%d alias=%d type=%s buffer=%s\n", generation,
                        tied->output == tied->tok_embd, ggml_type_name(tied->output->type),
                        ggml_backend_buffer_name(tied->output->buffer));
            check_projection(tied->output, 0.125f);
        }
        tied.reset();
        auto untied = load("untied.gguf");
        CHECK(untied != nullptr);
        if (untied) {
            CHECK(untied->output != untied->tok_embd);
            CHECK(untied->output->ne[0] == embedding && untied->output->ne[1] == vocabulary);
            std::printf("untied generation=%d alias=%d type=%s buffer=%s\n", generation,
                        untied->output == untied->tok_embd, ggml_type_name(untied->output->type),
                        ggml_backend_buffer_name(untied->output->buffer));
            check_projection(untied->output, 0.25f);
        }
        untied.reset();
        CHECK(load("bad-untied.gguf") == nullptr); // Explicit invalid head is not replaced by embeddings.
    }
    for (auto name : {"tied.gguf", "untied.gguf", "bad-untied.gguf"}) std::remove(name);
    llama_backend_free();
    return failures ? 1 : 0;
}
