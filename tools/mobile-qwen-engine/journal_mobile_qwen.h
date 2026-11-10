// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#ifndef JOURNAL_MOBILE_QWEN_H
#define JOURNAL_MOBILE_QWEN_H
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

#define JOURNAL_MOBILE_QWEN_API __attribute__((visibility("default")))
#define JOURNAL_MOBILE_QWEN_ABI_V1 1u
#define JOURNAL_MOBILE_QWEN_ERROR_CAPACITY 512u

typedef struct journal_mobile_qwen_handle journal_mobile_qwen_handle;

typedef enum journal_mobile_qwen_state {
    JOURNAL_MOBILE_QWEN_CREATED = 0,
    JOURNAL_MOBILE_QWEN_LOADING = 1,
    JOURNAL_MOBILE_QWEN_READY = 2,
    JOURNAL_MOBILE_QWEN_STOPPING = 3,
    JOURNAL_MOBILE_QWEN_STOPPED = 4,
    JOURNAL_MOBILE_QWEN_FAILED = 5
} journal_mobile_qwen_state;

typedef enum journal_mobile_qwen_result {
    JOURNAL_MOBILE_QWEN_OK = 0,
    JOURNAL_MOBILE_QWEN_INVALID = 1,
    JOURNAL_MOBILE_QWEN_BUSY = 2,
    JOURNAL_MOBILE_QWEN_ENGINE_ERROR = 3
} journal_mobile_qwen_result;

// Rust proves the pinned model bytes and holds their generation lease until run
// returns. Paths are absolute app-owned files; no URL, argv or model resolution.
// Strings are copied by create. expected_runtime_identity must match the library
// admitted by the signed package. bearer_token is a new 32-byte hex secret for
// this generation and must never be written to diagnostics or journal config.
typedef struct journal_mobile_qwen_config_v1 {
    uint32_t abi_version;
    uint32_t struct_size;
    uint64_t generation;
    const char * expected_runtime_identity;
    const char * expected_source_revision;
    const char * model_generation;
    const char * model_path;
    const char * projector_path;
    const char * bearer_token;
} journal_mobile_qwen_config_v1;

typedef struct journal_mobile_qwen_snapshot_v1 {
    uint32_t abi_version;
    uint32_t struct_size;
    uint64_t generation;
    uint32_t state;
    uint32_t bound_port;
    int32_t engine_result;
    char error[JOURNAL_MOBILE_QWEN_ERROR_CAPACITY];
} journal_mobile_qwen_snapshot_v1;

JOURNAL_MOBILE_QWEN_API uint32_t journal_mobile_qwen_abi_version(void);
JOURNAL_MOBILE_QWEN_API const char * journal_mobile_qwen_source_revision(void);
JOURNAL_MOBILE_QWEN_API const char * journal_mobile_qwen_runtime_identity(void);

// At most one live handle in this process. A stopped handle must be destroyed
// before creating another. create does not load models or start threads.
JOURNAL_MOBILE_QWEN_API int32_t journal_mobile_qwen_create_v1(const journal_mobile_qwen_config_v1 * config,
    journal_mobile_qwen_handle ** out, char * error, size_t error_capacity);
// Call exactly once on an app-owned worker. Blocks through complete native/HTTP
// teardown. No process signals or process exit; bind is always 127.0.0.1:0.
JOURNAL_MOBILE_QWEN_API int32_t journal_mobile_qwen_run_v1(journal_mobile_qwen_handle * handle);
// Thread-safe and latched before/during load. Revokes READY immediately. Does not
// join; the owner must await run before freeing model leases/credentials.
JOURNAL_MOBILE_QWEN_API int32_t journal_mobile_qwen_request_stop_v1(journal_mobile_qwen_handle * handle);
JOURNAL_MOBILE_QWEN_API int32_t journal_mobile_qwen_get_snapshot_v1(journal_mobile_qwen_handle * handle,
    journal_mobile_qwen_snapshot_v1 * snapshot);
// Only after joining the run worker (or before it was started). No concurrent
// callers may retain the handle once destroy is invoked. BUSY never frees it.
JOURNAL_MOBILE_QWEN_API int32_t journal_mobile_qwen_destroy_v1(journal_mobile_qwen_handle * handle);
#ifdef __cplusplus
}
#endif
#endif
