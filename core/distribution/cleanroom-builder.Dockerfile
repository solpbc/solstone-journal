# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# No registry-resolved FROM exists in this proof build. Both ADD inputs are
# preloaded bytes admitted by builder-inputs.toml before `docker build` runs.

FROM scratch

ADD builder-rootfs.tar /
ADD zig-x86_64-linux-0.16.0.tar.xz /opt/
ADD rust-std-1.97.1-aarch64-unknown-linux-gnu.tar.xz /opt/
ADD rust-std-1.97.1-aarch64-unknown-linux-musl.tar.xz /opt/
ADD rust-std-1.97.1-x86_64-unknown-linux-musl.tar.xz /opt/

ENV PATH=/opt/zig-x86_64-linux-0.16.0:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin \
    RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    RUST_VERSION=1.97.1 \
    SOLSTONE_ZIG=/opt/zig-x86_64-linux-0.16.0/zig

RUN /opt/rust-std-1.97.1-aarch64-unknown-linux-gnu/install.sh \
      --prefix=/usr/local/rustup/toolchains/1.97.1-x86_64-unknown-linux-gnu \
      --disable-ldconfig \
 && /opt/rust-std-1.97.1-aarch64-unknown-linux-musl/install.sh \
      --prefix=/usr/local/rustup/toolchains/1.97.1-x86_64-unknown-linux-gnu \
      --disable-ldconfig \
 && /opt/rust-std-1.97.1-x86_64-unknown-linux-musl/install.sh \
      --prefix=/usr/local/rustup/toolchains/1.97.1-x86_64-unknown-linux-gnu \
      --disable-ldconfig \
 && find /opt/rust-std-1.97.1-aarch64-unknown-linux-gnu \
         /opt/rust-std-1.97.1-aarch64-unknown-linux-musl \
         /opt/rust-std-1.97.1-x86_64-unknown-linux-musl \
      -depth -delete \
 && rustc --version | grep -Fx 'rustc 1.97.1 (8bab26f4f 2026-07-14)' \
 && cargo --version | grep -Fx 'cargo 1.97.1 (c980f4866 2026-06-30)' \
 && for target in \
      aarch64-unknown-linux-gnu \
      aarch64-unknown-linux-musl \
      x86_64-unknown-linux-gnu \
      x86_64-unknown-linux-musl; \
    do test -n "$(find "/usr/local/rustup/toolchains/1.97.1-x86_64-unknown-linux-gnu/lib/rustlib/$target/lib" -name 'libstd-*.rlib' -print -quit)"; \
    done \
 && zig version | grep -Fx '0.16.0' \
 && git --version \
 && make --version \
 && nasm -v \
 && pkg-config --version \
 && test -f /usr/lib/llvm-14/lib/libclang.so

WORKDIR /source
