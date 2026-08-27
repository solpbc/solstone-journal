#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Isolated, non-Docker proof that stopped_before_publish() (shared with
# python3-interpreter's TERM/INT trap in the legacy-upgrade-v1022 fixture)
# actually discriminates in both directions. Run directly: no container, no
# systemd, no `journal setup` involved.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
. "$ROOT/stop-before-publish-check.sh"

FAILS=0

pass() { printf 'ok %s\n' "$*"; }
fail() {
	FAILS=$((FAILS + 1))
	printf 'FAIL %s\n' "$*" >&2
}

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# Positive case: a v1.0.22-shaped unit (no guard line at all) -- the trap
# must fire (stopped_before_publish returns true / exit 0).
without_guard="$WORK/without-guard.service"
cat > "$without_guard" <<'UNIT'
[Unit]
Description=Solstone Supervisor
After=default.target
StartLimitIntervalSec=120
StartLimitBurst=10

[Service]
Type=notify
TimeoutStartSec=120
ExecStart=/home/solstone/.local/bin/journal start 5015
Restart=on-failure
RestartSec=5
KillMode=control-group
TimeoutStopSec=30
LimitNOFILE=4096
StandardOutput=append:/home/solstone/journal/health/service.log
StandardError=append:/home/solstone/journal/health/service.log
Environment=HOME=/home/solstone
Environment=PATH=/home/solstone/.local/bin:/usr/bin:/bin
Environment=PYTHONUNBUFFERED=1

[Install]
WantedBy=default.target
UNIT

if stopped_before_publish "$without_guard"; then
	pass "fires without the guard line (correct order: v1 stopped before V2 published)"
else
	fail "did not fire without the guard line -- the detector would never catch a real ordering bug"
fi

# Negative case: the same unit, but with the guard line already present --
# a detector that fires unconditionally would be a bug in the fixture
# itself, not a proof of correct ordering. Cover both render forms
# (systemd.rs::render_environment quotes only when the value is not
# "safe" -- see systemd.rs::is_safe).
for guard_line in \
	'Environment=SOLSTONE_INSTALLATION_NAMESPACE=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef' \
	'Environment="SOLSTONE_INSTALLATION_NAMESPACE=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"'
do
	with_guard="$WORK/with-guard.service"
	cat "$without_guard" > "$with_guard"
	printf '%s\n' "$guard_line" >> "$with_guard"

	if stopped_before_publish "$with_guard"; then
		fail "fired WITH the guard line present ($guard_line) -- the detector fires unconditionally, which is a bug, not a proof"
	else
		pass "does not fire with the guard line present ($guard_line)"
	fi
done

printf 'passed=%s failed=%s\n' "$((3 - FAILS))" "$FAILS"
[ "$FAILS" -eq 0 ]
