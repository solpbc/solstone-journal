# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# No registry-resolved FROM exists in this proof build. Both ADD inputs are
# preloaded bytes admitted by builder-inputs.toml before `docker build` runs.

FROM scratch

ADD builder-rootfs.tar /
ADD zig-x86_64-linux-0.16.0.tar.xz /opt/

ENV PATH=/opt/zig-x86_64-linux-0.16.0:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    RUST_VERSION=1.97.1 \
    SOLSTONE_ZIG=/opt/zig-x86_64-linux-0.16.0/zig

RUN rustc --version | grep -Fx 'rustc 1.97.1 (8bab26f4f 2026-07-14)' \
 && cargo --version | grep -Fx 'cargo 1.97.1 (c980f4866 2026-06-30)' \
 && zig version | grep -Fx '0.16.0' \
 && git --version \
 && make --version \
 && nasm -v \
 && pkg-config --version \
 && test -f /usr/lib/llvm-14/lib/libclang.so

WORKDIR /source
