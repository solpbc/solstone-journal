// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
#include "parakeet_capi.h"
#include <stdio.h>
int main(void) {
    const int requested[] = {4,2,4,0,1,2,4,6,8};
    const int default_threads=parakeet_capi_get_threads();
    if (default_threads <= 0) {
        fprintf(stderr,"default budget failed: %s\n",parakeet_capi_load_error());
        return 2;
    }
    const int expected[] = {4,2,4,default_threads,1,2,4,6,8};
    if (parakeet_capi_abi_version()!=10) return 1;
    parakeet_ctx* context=parakeet_capi_load("/missing.gguf");
    if (context) parakeet_capi_free(context);
    puts("[");
    for (unsigned i=0;i<sizeof(requested)/sizeof(requested[0]);++i) {
        int applied=parakeet_capi_set_threads(requested[i]);
        if (applied <= 0) {
            fprintf(stderr,"apply budget failed: %s\n",parakeet_capi_load_error());
            return 2;
        }
        int actual=parakeet_capi_get_threads();
        if (actual <= 0) {
            fprintf(stderr,"read budget failed: %s\n",parakeet_capi_load_error());
            return 2;
        }
        printf("{\"requested\":%d,\"applied\":%d,\"actual\":%d}%s\n",requested[i],applied,actual,i+1<sizeof(requested)/sizeof(requested[0])?",":"");
        if (actual!=expected[i] || applied!=expected[i]) {
            fprintf(stderr,"thread budget failed: %s\n",parakeet_capi_load_error());
            return 2;
        }
    }
    puts("]");
    return 0;
}
