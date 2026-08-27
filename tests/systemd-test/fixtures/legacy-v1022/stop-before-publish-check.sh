#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Reference implementation of the stop-before-publish signal used by the
# legacy-upgrade-v1022 fixture's python3-interpreter (a real Python script,
# not shell, so it can send its own Type=notify READY=1 in-process -- see
# that file's own comment). This shell version exists so
# stop-before-publish-check.test.sh can prove the signal discriminates in
# both directions without Docker or systemd; keep it and
# python3-interpreter's Python check in sync if the guard marker changes.
#
# The pre-seeded v1.0.22 unit's ExecStart is already "journal start <port>"
# -- the same form V2 itself writes -- so ExecStart cannot tell v1 from v2
# here. Every V2-published unit's environment unconditionally includes the
# SOLSTONE_INSTALLATION_NAMESPACE guard variable
# (core/crates/solstone-core-service-unit/src/systemd.rs::render_environment,
# called from build_service_environment), and the pre-seeded v1.0.22 unit has
# none of it. The guard line renders either quoted
# (Environment="SOLSTONE_INSTALLATION_NAMESPACE=...") or unquoted
# (Environment=SOLSTONE_INSTALLATION_NAMESPACE=...) depending on the value's
# byte content (systemd.rs::is_safe), so the check matches the key
# unanchored to either form.

# stopped_before_publish UNIT_FILE
# Exits 0 (true) if the unit file still lacks the guard line -- i.e. the v1
# process is being stopped before V2 published its unit, the order journal
# setup must guarantee. Exits 1 (false) if the guard line is already
# present, which would mean the new unit published before the old process
# died.
stopped_before_publish() {
	unit_file="$1"
	! grep -q "SOLSTONE_INSTALLATION_NAMESPACE=" "$unit_file"
}
