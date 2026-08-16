#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Operator harness. Subject definitions come from inventory.toml via
# `solstone-distribution cleanroom-plan`. Subjects run --network=none.
# The loopback cleanroom-serve stand-in is not a second origin: it serves
# the same bytes the installer digest-verifies.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
INVENTORY=$ROOT/core/distribution/inventory.toml
INSTALL_SH=$ROOT/core/distribution/install.sh
BIN=${SOLSTONE_DISTRIBUTION_BIN:-$ROOT/core/target/debug/solstone-distribution}

refuse() {
	printf '%s\n' "$1" >&2
	exit 2
}

[ -f "$INVENTORY" ] || refuse "missing required: $INVENTORY"
[ -f "$INSTALL_SH" ] || refuse "missing required: $INSTALL_SH"

if [ ! -x "$BIN" ]; then
	cargo build --manifest-path "$ROOT/core/Cargo.toml" -p solstone-core-distribution --bin solstone-distribution --locked --offline
	BIN=$ROOT/core/target/debug/solstone-distribution
fi

PLAN=$("$BIN" cleanroom-plan "$INVENTORY")
printf '%s\n' "$PLAN" | grep -q '^SUBJECT ' || refuse "unpinned cleanroom subject"

FAILED=
while IFS= read -r line; do
	case $line in
	SUBJECT\ *)
		id=$(printf '%s' "$line" | awk '{print $2}')
		digest=$(printf '%s' "$line" | awk '{print $4}')
		network=$(printf '%s' "$line" | awk '{print $5}')
		case $digest in
		sha256:?*) ;;
		*) FAILED="${FAILED} unpinned:${id}" ;;
		esac
		[ "$network" = "none" ] || FAILED="${FAILED} network:${id}"
		;;
	TOOLS\ *)
		case $line in
		*python* | *pip* | *maturin*) FAILED="${FAILED} python-tool" ;;
		esac
		;;
	esac
done <<EOF
$PLAN
EOF

if [ -n "$FAILED" ]; then
	refuse "unexpected:${FAILED}"
fi

# Definitions only. A live subject run is operator work; this harness
# refuses to invent a passing container result.
printf '%s\n' "$PLAN"
printf 'network=none\nloopback=127.0.0.1:0\naggregation=fail-closed\n'
exit 0
