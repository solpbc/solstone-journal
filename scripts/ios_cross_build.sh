#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Compile and link the journal workspace for aarch64-apple-ios (device).
#   prepare <dir>  acquire and verify the pinned iOS ONNX Runtime (network)
#   ready <dir>    verify the prepared input only (offline, no repair)
#   check <dir>    verify, then build the workspace and the MCP feature (offline inputs)
# This is compilation support. It does not package, install or run an iOS journal.
set -eu

mode=${1:?usage: ios_cross_build.sh prepare|ready|check <work-dir>}
work=${2:?work directory required}
root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

# Upstream ONNX Runtime 1.25.0 iOS package (static xcframework, CoreML provider).
ort_url=https://download.onnxruntime.ai/pod-archive-onnxruntime-c-1.25.0.zip
ort_zip=pod-archive-onnxruntime-c-1.25.0.zip
ort_zip_sha256=1d9414be5ed36d9198a6f51dda25c515e809e99e83a8581830a8b2075ed7dd1d
ort_member=onnxruntime.xcframework/ios-arm64/onnxruntime.framework/onnxruntime
# The thinned library digest is pinned in the Makefile (IOS_ONNX_RUNTIME_LIB_DIGEST).
ort_lib_sha256=${IOS_ONNX_RUNTIME_LIB_DIGEST:?IOS_ONNX_RUNTIME_LIB_DIGEST is required; run through make}
deployment_target=26.0
target=aarch64-apple-ios

test "$(uname -s)" = Darwin || { echo "ios cross-build: macOS host with Xcode required" >&2; exit 2; }
digest() { /usr/bin/shasum -a 256 "$1" | cut -d' ' -f1; }
lib="$work/lib/libonnxruntime.a"

ready() {
    test -f "$lib" && test "$(digest "$lib")" = "$ort_lib_sha256" || {
        echo "iOS ONNX Runtime input is not prepared; run make ci-full-prep-ios" >&2
        return 1
    }
}

prepare() {
    mkdir -p "$work/lib"
    if ready 2>/dev/null; then echo "iOS ONNX Runtime ready at $work/lib"; return 0; fi
    archive="$work/$ort_zip"
    if ! test -f "$archive" || test "$(digest "$archive")" != "$ort_zip_sha256"; then
        curl --fail --silent --show-error --location --retry 3 --output "$archive.part" "$ort_url"
        test "$(digest "$archive.part")" = "$ort_zip_sha256" || { echo "ONNX Runtime archive digest mismatch" >&2; exit 1; }
        mv "$archive.part" "$archive"
    fi
    rm -rf "$work/extract"
    mkdir -p "$work/extract"
    unzip -q "$archive" "$ort_member" LICENSE -d "$work/extract"
    # The framework binary is a single-slice universal archive; rustc needs a thin one.
    lipo -thin arm64 "$work/extract/$ort_member" -output "$lib.part"
    test "$(digest "$lib.part")" = "$ort_lib_sha256" || { echo "ONNX Runtime library digest mismatch" >&2; exit 1; }
    mv "$lib.part" "$lib"
    cp "$work/extract/LICENSE" "$work/lib/onnxruntime-LICENSE"
    echo "iOS ONNX Runtime ready at $work/lib"
}

check() {
    ready
    xcrun --sdk iphoneos --show-sdk-path >/dev/null
    cd "$root"
    # Set the minimum OS for cargo's children only: exported into a shell that
    # also builds host tools, Apple clang would target iOS for those too.
    build_env=(
        "IPHONEOS_DEPLOYMENT_TARGET=$deployment_target"
        "ORT_LIB_PATH=$work/lib"
        "CARGO_TARGET_DIR=$root/core/target-ios"
    )
    env "${build_env[@]}" cargo build --manifest-path core/Cargo.toml --locked \
        --target "$target" --workspace --lib --bins
    env "${build_env[@]}" cargo build --manifest-path core/Cargo.toml --locked \
        --target "$target" -p solstone-core --lib --bins --features journal-mcp-endpoint
    echo "check-rust-ios: workspace and journal-mcp-endpoint built for $target (minimum iOS $deployment_target)"
}

case "$mode" in
    prepare) prepare ;;
    ready) ready && echo "iOS ONNX Runtime ready at $work/lib" ;;
    check) check ;;
    *) echo "unknown mode: $mode" >&2; exit 2 ;;
esac
