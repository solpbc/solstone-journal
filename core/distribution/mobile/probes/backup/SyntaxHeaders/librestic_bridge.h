
#include <stdint.h>

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

int32_t restic_bridge_start(restic_request*,uint64_t*);
int32_t restic_bridge_wait(uint64_t,restic_result*);
int32_t restic_bridge_cancel(uint64_t);
int32_t restic_bridge_free(uint64_t);
int32_t restic_bridge_start_with_workspace(restic_request*,char*,uint32_t,uint64_t*);
int32_t restic_bridge_archive_sources_v1_start(uint32_t,const char*,uint32_t,const char*,uint32_t,const char*,uint32_t,int,const char*,uint32_t,uint64_t*);
int32_t restic_bridge_list_nodes_v1_start(uint32_t,const char*,uint32_t,const char*,uint32_t,const char*,uint32_t,uint64_t*);
int32_t restic_bridge_download_exact_v1_start(uint32_t,const char*,uint32_t,const char*,uint32_t,const char*,uint32_t,const char*,uint32_t,int,uint64_t,uint64_t*);
