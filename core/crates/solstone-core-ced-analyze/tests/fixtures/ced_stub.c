/* SPDX-License-Identifier: AGPL-3.0-only */
/* Copyright (c) 2026 sol pbc */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifndef CED_TEST_ABI
#define CED_TEST_ABI 1
#endif

typedef struct ced_ctx {
    int active;
} ced_ctx;

static char model_path[4096];
static const char *last_error = "stub failure";

static void record(const char *event) {
    char path[8192];
    FILE *file;
    if (model_path[0] == '\0') return;
    snprintf(path, sizeof(path), "%s.counts", model_path);
    file = fopen(path, "a");
    if (file != NULL) {
        fprintf(file, "%s\n", event);
        fclose(file);
    }
}

int ced_capi_abi_version(void) { return CED_TEST_ABI; }

ced_ctx *ced_capi_load(const char *gguf_path) {
    char marker[16] = {0};
    FILE *file;
    snprintf(model_path, sizeof(model_path), "%s", gguf_path);
    record("load");
    file = fopen(gguf_path, "r");
    if (file != NULL) {
        fread(marker, 1, sizeof(marker) - 1, file);
        fclose(file);
    }
    if (strstr(marker, "NULL_LOAD") != NULL) {
        last_error = "stub null load";
        return NULL;
    }
    return malloc(sizeof(ced_ctx));
}

void ced_capi_free(ced_ctx *ctx) {
    record("context_free");
    free(ctx);
}

const char *ced_capi_last_error(const ced_ctx *ctx) {
    (void)ctx;
    return last_error;
}

char *ced_capi_classify_pcm_json(
    ced_ctx *ctx,
    const float *samples,
    int n_samples,
    int sample_rate,
    int top_k
) {
    const char *json = "[{\"label\":\"Below\",\"score\":0.09},{\"label\":\"Floor\",\"score\":0.1},{\"label\":\"Above\",\"score\":0.11},{\"label\":\"Music\",\"score\":0.9}]";
    (void)ctx;
    (void)sample_rate;
    (void)top_k;
    record("classify");
    if (n_samples > 0 && samples[0] < 0.0f) {
        last_error = "stub classify failure";
        return NULL;
    }
    return strdup(json);
}

void ced_capi_free_string(char *text) {
    record("free_string");
    free(text);
}
