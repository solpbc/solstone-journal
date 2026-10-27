// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include <stdbool.h>
#include <stdint.h>

char *journal_runtime(const char *input);
void journal_runtime_free(char *result);
uint64_t journal_ios_processing_generation(void);
void journal_ios_processing_request_stop(void);
bool journal_ios_processing_request_owner_stop(void);
char *journal_ios_owner_open(const uint8_t *request, uintptr_t len, int32_t fd);
char *journal_ios_owner_open_stream(const uint8_t *request, uintptr_t len, int32_t response_fd, int32_t body_fd);
uint64_t journal_ios_owner_next_id(void);
bool journal_ios_owner_sender_admit(bool main_frame, const char *protocol, const char *host, uint16_t port);
uint64_t journal_ios_owner_authority_issue(void);
void journal_ios_owner_authority_retire(uint64_t authority);
char *journal_ios_owner_outcome_admit(uint64_t authority, const char *page_id);
char *journal_ios_owner_outcome_keep(uint64_t authority, const char *page_id, const char *reply_json);
char *journal_ios_owner_outcome_take(uint64_t authority, const char *page_id);
void journal_ios_owner_outcome_cancel(uint64_t authority, const char *page_id);
char *journal_ios_owner_poll(uint64_t id);
char *journal_ios_owner_reclaim(const char *cache, const char *live_json);
char *journal_ios_owner_bridge_allocate(const char *cache, uint64_t authority);
char *journal_ios_owner_bridge_commit(const char *cache, uint64_t authority, uint64_t id, const char *token);
char *journal_ios_owner_bridge_release(const char *cache, uint64_t authority, uint64_t id, const char *token, int32_t receipt_created);
char *journal_ios_owner_bridge_disarm(const char *cache, uint64_t authority, uint64_t id, const char *token);
void journal_ios_owner_cancel(uint64_t id);
typedef bool (*journal_ios_folder_copy_fn)(void *context, const char *destination);
char *journal_ios_owner_folder_copy(const char *cache, const uint8_t *request, uintptr_t length, void *context, journal_ios_folder_copy_fn copy);
bool journal_ios_owner_folder_copy_continues(void);
char *runtime_resources(void);
void runtime_resources_free(char *result);

typedef char *(*journal_ios_transport_fn)(const char *request_json);

bool journal_ios_verifier_admit(void);
char *journal_ios_verifier_admission_diagnostic(void);
bool journal_ios_verifier_register_callback(journal_ios_transport_fn transport);
void journal_ios_verifier_cancel(void);
bool journal_ios_verifier_drain_ready(void);

// Fixed startup admission only; these are not journal owner IPC operations.
bool journal_ios_backup_admit(void);
char *journal_ios_backup_admission_diagnostic(void);
char *journal_ios_backup_private_root(void);
void journal_ios_backup_private_root_free(char *result);
