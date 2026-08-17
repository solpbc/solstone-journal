# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Acquisition phase only. This is intentionally separate from the offline
# proof image. Export, normalize, and digest this rootfs; the proof build uses
# only that admitted rootfs tar with cleanroom-builder.Dockerfile.

FROM rust@sha256:2775a09d208ff0d7c1f50490c45b62db929e87ba1dcbc3f2132ac71a704bcdd3

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      git=1:2.39.5-0+deb12u3 \
      libclang-14-dev=1:14.0.6-12 \
      make=4.3-4.1 \
      nasm=2.16.01-1 \
      pkg-config=1.8.1-1 \
 && apt-get clean \
 && rm -rf /var/lib/apt/lists/* /var/cache/apt/archives/* \
 && find /var/log/apt /var/log/dpkg.log -type f -exec sh -c ': > "$1"' sh {} \;

RUN rustc --version | grep -Fx 'rustc 1.97.1 (8bab26f4f 2026-07-14)' \
 && cargo --version | grep -Fx 'cargo 1.97.1 (c980f4866 2026-06-30)' \
 && git --version \
 && make --version \
 && nasm -v \
 && pkg-config --version \
 && test -f /usr/lib/llvm-14/lib/libclang.so
