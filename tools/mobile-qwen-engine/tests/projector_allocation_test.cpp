// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include "clip.h"
#include "mtmd.h"
#include "ggml-backend.h"

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <filesystem>
#include <string>
#if defined(__linux__)
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>
#endif

// An opt-in integration control uses the real pinned projector. Limit only the
// child, so a refused native allocation cannot kill or contaminate its parent.
int main(int argc, char ** argv) {
#if defined(__linux__)
    const char * projector = argc == 2 ? argv[1] : std::getenv("JOURNAL_MOBILE_QWEN_TEST_PROJECTOR");
    if (!projector || !*projector) { return 77; }
    constexpr rlim_t limit = 256 * 1024 * 1024;
    if (!std::filesystem::is_regular_file(projector) || std::filesystem::file_size(projector) <= limit) {
        std::fprintf(stderr, "A real projector larger than the allocation limit is required\n");
        return 1;
    }
    ggml_backend_load_all();
    clip_context_params params{};
    params.flash_attn_type = CLIP_FLASH_ATTN_TYPE_AUTO;
    // Preserve normal weights and CPU tensor loading; omit inference warmup.
    params.use_gpu = false;
    params.warmup = false;
    for (int generation = 0; generation != 2; ++generation) {
        const pid_t child = fork();
        if (child < 0) { return 1; }
        if (child == 0) {
            const rlimit core{0, 0};
            const rlimit memory{limit, limit};
            if (setrlimit(RLIMIT_CORE, &core) != 0 || setrlimit(RLIMIT_AS, &memory) != 0) { _exit(2); }
            std::string errors;
            mtmd_log_set([](ggml_log_level level, const char * text, void * data) {
                if (level == GGML_LOG_LEVEL_ERROR) { static_cast<std::string *>(data)->append(text); }
            }, &errors);
            const auto loaded = clip_init(projector, params);
            const bool refused = !loaded.ctx_v && !loaded.ctx_a && !loaded.ctx_gen_a;
            const bool allocation_error = errors.find("failed to allocate projector weights") != std::string::npos;
            std::fprintf(stderr, "limited generation %d: refused=%d allocation_error=%d\n", generation, refused, allocation_error);
            std::fflush(stderr);
            _exit(refused && allocation_error ? 0 : 3);
        }
        int status = 0;
        if (waitpid(child, &status, 0) != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
            std::fprintf(stderr, "limited projector load did not return safely: wait_status=%d signal=%d\n",
                         status, WIFSIGNALED(status) ? WTERMSIG(status) : 0);
            return 1;
        }
    }
    // Positive control: the same unmodified projector still loads without the
    // child memory restriction after both failed generations have been reaped.
    const auto loaded = clip_init(projector, params);
    if (!loaded.ctx_v || loaded.ctx_a || loaded.ctx_gen_a) { return 1; }
    clip_free(loaded.ctx_v);
    std::fprintf(stderr, "unlimited real-projector positive control passed\n");
    return 0;
#else
    (void) argc;
    (void) argv;
    return 77;
#endif
}
