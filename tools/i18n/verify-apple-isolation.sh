#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
set -eu
if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
    echo "usage: verify-apple-isolation.sh <generated-selector.swift>" >&2
    exit 2
fi
script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec xcrun swiftc -swift-version 6 -default-isolation MainActor -warnings-as-errors \
    -typecheck "$1" "$script_dir/AppleIsolationProbe.swift"
