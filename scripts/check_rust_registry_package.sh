#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Cargo rejects --lib before executing any test when a package only has binary targets.
# Preserve every other Cargo failure rather than treating it as a target fallback.
set -eu

manifest=${1:?manifest path is required}
package=${CI_PACKAGE:?CI_PACKAGE is required}
features=${CI_FEATURES:-}

run_cargo() {
    if [ -n "$features" ]; then
        cargo test --manifest-path "$manifest" --locked --offline "$@" --features "$features" -- --test-threads=1
    else
        cargo test --manifest-path "$manifest" --locked --offline "$@" -- --test-threads=1
    fi
}

if output="$(run_cargo -p "$package" --lib --bins 2>&1)"; then
    printf '%s\n' "$output"
    exit 0
else
    cargo_status=$?
fi

if printf '%s\n' "$output" | grep -F -- "no library targets found in package \`$package\`" >/dev/null; then
    printf '%s\n' "package $package has no library target; testing binary targets" >&2
    run_cargo -p "$package" --bins
    exit $?
fi

printf '%s\n' "$output" >&2
exit "$cargo_status"
