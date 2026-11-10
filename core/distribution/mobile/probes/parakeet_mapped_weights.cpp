// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
// Host native behavior probe: link the actual maintained Parakeet/GGML archives.
// Linux fault controls use -DJOURNAL_MMAP_FAULT_CONTROL and --wrap=mmap plus
// --wrap=ggml_backend_is_cpu/ggml_backend_dev_get_props and
// --wrap=ggml_backend_dev_buffer_from_host_ptr. No product hook is added.
#include "model_loader.hpp"
#include "parakeet_capi.h"
#include "ggml.h"
#include "gguf.h"
#include "ggml-backend.h"
#include "ggml-cpu.h"
#include <algorithm>
#include <array>
#include <cassert>
#include <cerrno>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <iterator>
#include <string>
#include <sys/mman.h>
#include <unistd.h>
#include <vector>

#ifdef JOURNAL_MMAP_FAULT_CONTROL
static bool refuse_map = false, route_upload = false, refuse_device_map = false;
static void* last_mapping = nullptr;
extern "C" void* __real_mmap(void*, size_t, int, int, int, off_t);
extern "C" bool __real_ggml_backend_is_cpu(ggml_backend_t);
extern "C" void __real_ggml_backend_dev_get_props(ggml_backend_dev_t, ggml_backend_dev_props*);
extern "C" ggml_backend_buffer_t __real_ggml_backend_dev_buffer_from_host_ptr(ggml_backend_dev_t, void*, size_t, size_t);
extern "C" void* __wrap_mmap(void* p, size_t n, int prot, int flags, int fd, off_t off) {
    if (refuse_map) { errno = ENOMEM; return MAP_FAILED; }
    void* result = __real_mmap(p, n, prot, flags, fd, off);
    if (result != MAP_FAILED) last_mapping = result;
    return result;
}
extern "C" bool __wrap_ggml_backend_is_cpu(ggml_backend_t backend) {
    return (route_upload || refuse_device_map) ? false : __real_ggml_backend_is_cpu(backend);
}
extern "C" void __wrap_ggml_backend_dev_get_props(ggml_backend_dev_t device, ggml_backend_dev_props* props) {
    __real_ggml_backend_dev_get_props(device, props);
    if (route_upload) props->caps.buffer_from_host_ptr = false;
}
extern "C" ggml_backend_buffer_t __wrap_ggml_backend_dev_buffer_from_host_ptr(
        ggml_backend_dev_t device, void* ptr, size_t size, size_t largest) {
    return refuse_device_map ? nullptr : __real_ggml_backend_dev_buffer_from_host_ptr(device, ptr, size, largest);
}
static void assert_unmapped() {
    unsigned char vec = 0;
    errno = 0;
    assert(last_mapping && mincore(last_mapping, 1, &vec) == -1 && errno == ENOMEM);
}
#endif

static void fixture(const std::string& path) {
    ggml_context* ctx = ggml_init({12u << 20, nullptr, false});
    ggml_tensor* weight = ggml_new_tensor_2d(ctx, GGML_TYPE_F32, 4, 3);
    ggml_set_name(weight, "linear.weight");
    for (int i = 0; i < 12; ++i) ((float*)weight->data)[i] = float(i + 1);
    ggml_tensor* bulk = ggml_new_tensor_1d(ctx, GGML_TYPE_I8, (8u << 20) + 17);
    ggml_set_name(bulk, "bulk.weight");
    for (size_t i = 0; i < ggml_nbytes(bulk); ++i) ((uint8_t*)bulk->data)[i] = i % 251;
    gguf_context* gguf = gguf_init_empty();
    gguf_set_val_str(gguf, "parakeet.arch", "tdt");
    gguf_set_val_u32(gguf, "parakeet.encoder.d_model", 4);
    gguf_set_val_u32(gguf, "parakeet.vocab_size", 2);
    gguf_add_tensor(gguf, weight);
    gguf_add_tensor(gguf, bulk);
    assert(gguf_write_to_file(gguf, path.c_str(), false));
    gguf_free(gguf);
    ggml_free(ctx);
}

static std::array<float, 3> calculate(ggml_tensor* weight, ggml_backend_t backend) {
    ggml_context* ctx = ggml_init({65536, nullptr, true});
    ggml_tensor* input = ggml_new_tensor_2d(ctx, GGML_TYPE_F32, 4, 1);
    ggml_tensor* out = ggml_mul_mat(ctx, weight, input);
    ggml_cgraph* graph = ggml_new_graph_custom(ctx, 16, false);
    ggml_build_forward_expand(graph, out);
    ggml_backend_buffer_t buffer = ggml_backend_alloc_ctx_tensors(ctx, backend);
    assert(buffer);
    const float ones[] = {1, 1, 1, 1};
    ggml_backend_tensor_set(input, ones, 0, sizeof(ones));
    assert(ggml_backend_graph_compute(backend, graph) == GGML_STATUS_SUCCESS);
    std::array<float, 3> result{};
    ggml_backend_tensor_get(out, result.data(), 0, sizeof(result));
    ggml_backend_buffer_free(buffer);
    ggml_free(ctx);
    return result;
}

static std::vector<char> read(const std::string& path) {
    std::ifstream file(path, std::ios::binary);
    return {std::istreambuf_iterator<char>(file), {}};
}
static void write(const std::string& path, const std::vector<char>& bytes) {
    std::ofstream file(path, std::ios::binary);
    file.write(bytes.data(), bytes.size());
    assert(file.good());
}

static int compare_model(const char* path) {
    ggml_context* old = nullptr;
    gguf_context* gguf = gguf_init_from_file(path, {false, &old});
    assert(gguf);
    ggml_backend_t backend = ggml_backend_cpu_init();
    assert(backend);
    size_t bytes = 0;
    const int64_t count = gguf_get_n_tensors(gguf);
    {
        pk::ModelLoader loader;
        assert(loader.load(path) && loader.realize_weights(backend));
        assert(ggml_get_no_alloc(loader.ggml_ctx()));
        for (int64_t i = 0; i < count; ++i) {
            const char* name = gguf_get_tensor_name(gguf, i);
            ggml_tensor* reference = ggml_get_tensor(old, name);
            ggml_tensor* mapped = loader.tensor(name);
            assert(mapped && mapped->type == reference->type);
            assert(std::memcmp(mapped->ne, reference->ne, sizeof(mapped->ne)) == 0);
            assert(ggml_nbytes(mapped) == ggml_nbytes(reference));
            assert(std::memcmp(mapped->data, reference->data, ggml_nbytes(mapped)) == 0);
            bytes += ggml_nbytes(mapped);
        }
        std::printf("PASS full-model byte equivalence tensors=%lld bytes=%zu metadata_context_bytes=%zu\n",
                    (long long)count, bytes, ggml_get_mem_size(loader.ggml_ctx()));
    }
#ifdef JOURNAL_MMAP_FAULT_CONTROL
    assert_unmapped();
#endif
    ggml_backend_free(backend);
    gguf_free(gguf);
    ggml_free(old);
    return 0;
}

int main(int argc, char** argv) {
    if (argc == 3 && std::string(argv[1]) == "--compare-model") return compare_model(argv[2]);
    assert(argc == 2); // owned scratch prefix; no model/network input
    const std::string path = std::string(argv[1]) + ".gguf";
    fixture(path);
    const auto bytes = read(path);
    ggml_backend_t backend = ggml_backend_cpu_init();
    assert(backend);
    ggml_backend_cpu_set_n_threads(backend, 2);

    // Reference the original GGUF blob path, then compare a real graph with
    // mapped weights byte-for-byte and output-for-output.
    ggml_context* old = nullptr;
    gguf_context* old_gguf = gguf_init_from_file(path.c_str(), {false, &old});
    assert(old_gguf);
    ggml_backend_buffer_t old_buffer = ggml_backend_cpu_buffer_from_ptr(
        ggml_get_mem_buffer(old), ggml_get_mem_size(old));
    ggml_tensor* reference = ggml_get_tensor(old, "linear.weight");
    ggml_tensor* reference_bulk = ggml_get_tensor(old, "bulk.weight");
    reference->buffer = old_buffer;
    const auto expected = calculate(reference, backend);
    assert((expected == std::array<float, 3>{10, 26, 42}));
    for (int i = 0; i < 32; ++i) {
        {
            pk::ModelLoader loader;
            assert(loader.load(path));
            assert(ggml_get_no_alloc(loader.ggml_ctx()));
            assert(ggml_get_mem_size(loader.ggml_ctx()) < 4096);
            ggml_tensor* weight = loader.tensor("linear.weight");
            assert(weight && std::memcmp(weight->data, reference->data, ggml_nbytes(weight)) == 0);
            ggml_tensor* bulk = loader.tensor("bulk.weight");
            assert(std::memcmp(bulk->data, reference_bulk->data, ggml_nbytes(bulk)) == 0);
            assert(loader.realize_weights(backend));
            assert(loader.realize_weights(backend)); // idempotent wrapper ownership
            assert(calculate(weight, backend) == expected);
        }
#ifdef JOURNAL_MMAP_FAULT_CONTROL
        assert_unmapped();
#endif
    }

#ifdef JOURNAL_MMAP_FAULT_CONTROL
    refuse_map = true;
    { pk::ModelLoader loader; assert(!loader.load(path)); }
    refuse_map = false;
    refuse_device_map = true;
    assert(parakeet_capi_load(path.c_str()) == nullptr); // no falsely ready native model
    refuse_device_map = false;
    assert_unmapped();
    parakeet_ctx* recovered = parakeet_capi_load(path.c_str());
    assert(recovered);
    parakeet_capi_free(recovered);
    assert_unmapped(); // failure did not poison later ordinary admission
    {
        pk::ModelLoader loader;
        assert(loader.load(path));
        route_upload = true; // CPU-backed stand-in for a non-mapping device
        assert(loader.realize_weights(backend));
        route_upload = false;
        assert_unmapped(); // upload completed before source mapping retirement
        assert(calculate(loader.tensor("linear.weight"), backend) == expected);
        std::vector<char> uploaded(ggml_nbytes(reference_bulk));
        ggml_backend_tensor_get(loader.tensor("bulk.weight"), uploaded.data(), 0, uploaded.size());
        assert(std::memcmp(uploaded.data(), reference_bulk->data, uploaded.size()) == 0);
    }
#endif

    // Real truncation, and a hostile tensor offset, must fail without touching
    // absent payload bytes or aborting inside an anonymous allocation.
    auto truncated = bytes;
    truncated.resize(truncated.size() - 20);
    write(path + ".truncated", truncated);
    { pk::ModelLoader loader; assert(!loader.load(path + ".truncated")); }
    auto bad_offset = bytes;
    const std::string name = "linear.weight";
    auto start = std::search(bad_offset.begin(), bad_offset.end(), name.begin(), name.end());
    assert(start != bad_offset.end());
    const size_t offset_field = size_t(start - bad_offset.begin()) + name.size() + 4 + 2 * 8 + 4;
    const uint64_t offset = UINT64_MAX - 31;
    std::memcpy(bad_offset.data() + offset_field, &offset, sizeof(offset));
    write(path + ".bad-offset", bad_offset);
    { pk::ModelLoader loader; assert(!loader.load(path + ".bad-offset")); }

    ggml_backend_buffer_free(old_buffer);
    gguf_free(old_gguf);
    ggml_free(old);
    ggml_backend_free(backend);
    std::printf("PASS mapped graph equivalence, 32 lifecycles, metadata-only context, range/truncation");
#ifdef JOURNAL_MMAP_FAULT_CONTROL
    std::printf(", map refusal and bounded-upload retirement");
#endif
    std::puts("");
}
