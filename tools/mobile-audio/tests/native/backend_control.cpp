// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include "ggml.h"
#include <cstddef>

namespace {
std::size_t freed_contexts = 0;
void counted_context_free(ggml_context* ctx) {
    ggml_free(ctx);
    ++freed_contexts;
}
}

// White-box observation only: use the exact selected native implementation,
// forwarding every free to real GGML. No failure or backend result is injected.
#define ggml_free counted_context_free
#include PARAKEET_TEST_BACKEND_SOURCE
#undef ggml_free

bool graph_control_active_backend_is_clear() { return pk::t_active == nullptr; }
std::size_t graph_control_freed_contexts() { return freed_contexts; }
