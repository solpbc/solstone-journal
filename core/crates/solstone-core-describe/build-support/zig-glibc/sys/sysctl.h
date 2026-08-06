/*
 * Zig's glibc 2.27 sysroot exports sysctl but omits this deprecated header.
 * FFmpeg probes the symbol and then includes the header unconditionally.
 */
#ifndef SOLSTONE_ZIG_GLIBC_SYSCTL_H
#define SOLSTONE_ZIG_GLIBC_SYSCTL_H

#include <stddef.h>

int sysctl(int *name, int nlen, void *oldval, size_t *oldlenp, void *newval, size_t newlen);

#endif
