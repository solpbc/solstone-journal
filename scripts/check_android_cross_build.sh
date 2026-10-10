#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
set -eu

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
work=$(cd "${1:?native build directory required}" && pwd)
mode=${2:-build}
case "$mode" in build|--generate-bindings) ;; *) echo "unknown build mode: $mode" >&2; exit 2 ;; esac
test "$(uname -s)" = Linux && test "$(uname -m)" = x86_64 || { echo "Linux x86_64 builder required" >&2; exit 2; }
cd "$root"
receipts="$work/receipts"
mkdir -p "$receipts"
toolchain="$work/android-ndk-r30/toolchains/llvm/prebuilt/linux-x86_64"
grep -qx 'Pkg.Revision = 30.0.16248370' "$work/android-ndk-r30/source.properties"
IFS= read -r archive < "$work/ffmpeg-source-archive.path"
IFS= read -r expected < "$work/ffmpeg-source-archive.sha256"
actual=$(sha256sum -- "$archive")
test "${actual%% *}" = "$expected" || { echo "FFmpeg source digest mismatch" >&2; exit 1; }
IFS= read -r expected < "$work/onnxruntime-library.sha256"
actual=$(sha256sum -- "$work/lib/libonnxruntime.so")
test "${actual%% *}" = "$expected" || { echo "ONNX Runtime library digest mismatch" >&2; exit 1; }

export TMPDIR="$work" CARGO_INCREMENTAL=0
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/core/target-android}"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$toolchain/bin/aarch64-linux-android31-clang"
export CC_aarch64_linux_android="$toolchain/bin/aarch64-linux-android31-clang"
export CXX_aarch64_linux_android="$toolchain/bin/aarch64-linux-android31-clang++"
export AR_aarch64_linux_android="$toolchain/bin/llvm-ar"
export CARGO_NDK_SYSROOT_PATH="$toolchain/sysroot"
export SOLSTONE_FFMPEG_SOURCE_ARCHIVE="$archive"
export ORT_LIB_PATH="$work/lib" ORT_PREFER_DYNAMIC_LINK=1
# FFmpeg's producer reads the hyphenated cc coordinates and appends -gcc.
# Supplying clang itself plus its target keeps that compiler coordinate valid.
native_env=(
    "CC_aarch64-linux-android=$toolchain/bin/clang"
    "CXX_aarch64-linux-android=$toolchain/bin/clang++"
    "AR_aarch64-linux-android=$toolchain/bin/llvm-ar"
    "CFLAGS_aarch64-linux-android=--target=aarch64-linux-android31"
    "CXXFLAGS_aarch64-linux-android=--target=aarch64-linux-android31"
)
if test "$mode" = --generate-bindings; then
    export SOLSTONE_FFMPEG_BINDINGS_OUT="$root/core/vendor/ffmpeg-sys-next/bindings"
    resource_dir=$("$toolchain/bin/clang" -print-resource-dir)
    export BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android="--target=aarch64-linux-android31 -resource-dir=\"$resource_dir\""
    export LIBCLANG_PATH="$toolchain/lib"
fi

run() {
    local name=$1 code
    shift
    printf '%s:' "$name"
    printf ' %q' "$@"
    printf '\n'
    if env "${native_env[@]}" "$@" > "$receipts/$name.log" 2> "$receipts/$name.stderr"; then code=0; else code=$?; fi
    printf '%s\n' "$code" > "$receipts/$name.exit"
    if test "$code" -ne 0; then tail -n 60 "$receipts/$name.log" "$receipts/$name.stderr" >&2; return "$code"; fi
}
printf '%s\n' "${native_env[@]}" "CARGO_TARGET_DIR=$CARGO_TARGET_DIR" "CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER" "CC_aarch64_linux_android=$CC_aarch64_linux_android" "CXX_aarch64_linux_android=$CXX_aarch64_linux_android" "AR_aarch64_linux_android=$AR_aarch64_linux_android" "ORT_LIB_PATH=$ORT_LIB_PATH" "ORT_PREFER_DYNAMIC_LINK=$ORT_PREFER_DYNAMIC_LINK" "SOLSTONE_FFMPEG_SOURCE_ARCHIVE=$SOLSTONE_FFMPEG_SOURCE_ARCHIVE" > "$receipts/rust-build-environment.txt"
for key in CARGO_BUILD_JOBS CARGO_NET_OFFLINE CARGO_PROFILE_DEV_DEBUG CARGO_INCREMENTAL RUSTFLAGS LIBCLANG_PATH BINDGEN_EXTRA_CLANG_ARGS_aarch64_linux_android SOLSTONE_FFMPEG_BINDINGS_OUT; do
    printf '%s=%s\n' "$key" "${!key-}" >> "$receipts/rust-build-environment.txt"
done
git rev-parse HEAD > "$receipts/rust-source-commit.txt"
git status --porcelain > "$receipts/rust-source-status.txt"
run workspace-metadata cargo metadata --manifest-path core/Cargo.toml --locked --no-deps --format-version 1
command=(cargo build --manifest-path core/Cargo.toml --locked --target aarch64-linux-android --workspace --lib --bins --message-format=json-render-diagnostics)
if test "$mode" = --generate-bindings; then
    command+=(--features ffmpeg-sys-next/generate-bindings)
fi
run android-workspace "${command[@]}"
if test "$mode" = build; then
    run android-mcp cargo build --manifest-path core/Cargo.toml --locked --target aarch64-linux-android -p solstone-core --lib --bins --features journal-mcp-endpoint --message-format=json-render-diagnostics
fi
