/* SPDX-License-Identifier: AGPL-3.0-only */
/* Copyright (c) 2026 sol pbc */
extern int solstone_sound_ced_capi_abi_version(void);
extern void *solstone_sound_ced_capi_load(const char *);
extern void solstone_sound_ced_capi_free(void *);
extern const char *solstone_sound_ced_capi_last_error(const void *);
extern char *solstone_sound_ced_capi_classify_pcm_json(void *, const float *, int, int, int);
extern void solstone_sound_ced_capi_free_string(char *);

int main(void) {
    void *context = solstone_sound_ced_capi_load("/missing.gguf");
    float sample = 0;
    char *tags = solstone_sound_ced_capi_classify_pcm_json(context, &sample, 1, 16000, 0);
    solstone_sound_ced_capi_free_string(tags);
    solstone_sound_ced_capi_free(context);
    return solstone_sound_ced_capi_abi_version() != 1 ||
           solstone_sound_ced_capi_last_error(0) == 0;
}
