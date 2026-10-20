// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include <stdbool.h>
#include <stdint.h>

char *journal_runtime(const char *input);
void journal_runtime_free(char *result);
uint64_t journal_ios_processing_generation(void);
void journal_ios_processing_request_stop(void);
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
