// Compile/link probe only; target execution is outside this build evidence.
#include "ced_capi.h"

static void (*volatile required_symbols[])(void) = {
    (void (*)(void))ced_capi_abi_version,
    (void (*)(void))ced_capi_load,
    (void (*)(void))ced_capi_free,
    (void (*)(void))ced_capi_last_error,
    (void (*)(void))ced_capi_classify_pcm_json,
    (void (*)(void))ced_capi_free_string,
};
int main(int argc, char **argv) {
    (void)argv;
    unsigned int count = sizeof(required_symbols) / sizeof(required_symbols[0]);
    return required_symbols[(unsigned int)argc % count] ? 0 : 1;
}
