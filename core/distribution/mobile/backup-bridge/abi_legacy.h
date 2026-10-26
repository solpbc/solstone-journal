// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#include <stdint.h>

#ifndef SOLSTONE_RESTIC_LEGACY_TYPES
#define SOLSTONE_RESTIC_LEGACY_TYPES
typedef struct {
    int32_t operation;
    const char *password;
    uint32_t password_len;
    const char *recovery_password;
    uint32_t recovery_password_len;
    const char *repo_path;
    uint32_t repo_path_len;
    const char *source_path;
    uint32_t source_path_len;
    const char *restore_path;
    uint32_t restore_path_len;
    const char *snapshot_id;
    uint32_t snapshot_id_len;
} restic_request;

typedef struct {
    int32_t status;
    char key_id[65];
    char snapshot_id[65];
    uint32_t snapshot_count;
    char snapshot_ids[4096][65];
    char diagnostic[256];
} restic_result;
#endif

int32_t restic_bridge_start(restic_request*,uint64_t*);
int32_t restic_bridge_wait(uint64_t,restic_result*);
int32_t restic_bridge_cancel(uint64_t);
int32_t restic_bridge_free(uint64_t);

#ifndef SOLSTONE_RESTIC_S3_REQUEST
#define SOLSTONE_RESTIC_S3_REQUEST
typedef struct {
    uint32_t version;
    int32_t operation;
    const char *endpoint; uint32_t endpoint_len;
    const char *region; uint32_t region_len;
    const char *bucket; uint32_t bucket_len;
    const char *prefix; uint32_t prefix_len;
    const char *access_key; uint32_t access_key_len;
    const char *secret_key; uint32_t secret_key_len;
    const char *session_token; uint32_t session_token_len;
    const char *password; uint32_t password_len;
    const char *recovery_password; uint32_t recovery_password_len;
    const char *state_path; uint32_t state_path_len;
    const char *source_path; uint32_t source_path_len;
    const char *restore_path; uint32_t restore_path_len;
    const char *snapshot_id; uint32_t snapshot_id_len;
    const char *workspace_path; uint32_t workspace_path_len;
} restic_s3_request;
#endif

int32_t restic_bridge_archive_sources_v1_start(uint32_t,const char*,uint32_t,const char*,uint32_t,const char*,uint32_t,int,const char*,uint32_t,uint64_t*);
int32_t restic_bridge_list_nodes_v1_start(uint32_t,const char*,uint32_t,const char*,uint32_t,const char*,uint32_t,uint64_t*);
int32_t restic_bridge_download_exact_v1_start(uint32_t,const char*,uint32_t,const char*,uint32_t,const char*,uint32_t,const char*,uint32_t,int,uint64_t,uint64_t*);
int32_t restic_bridge_s3_archive_sources_v1_start(restic_s3_request*,int,int,const char*,uint32_t,const char*,uint32_t,uint32_t,uint64_t*);
int32_t restic_bridge_s3_list_nodes_v1_start(restic_s3_request*,int,uint32_t,uint64_t*);
int32_t restic_bridge_s3_download_exact_v1_start(restic_s3_request*,int,int,const char*,uint32_t,uint64_t,uint32_t,uint64_t*);
