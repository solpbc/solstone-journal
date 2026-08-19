#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Start the journal supervisor in the background and print its pid.
# Replaces the Python starter so `make sandbox` needs no interpreter.
set -eu

log=${SANDBOX_LOG:?}
path=${SANDBOX_PATH:?}
bin=${JOURNAL_BIN:?}

mkdir -p "$(dirname "$log")"

# Ignore SIGHUP so the supervisor survives the make recipe shell exiting.
# Equivalent to Python's Popen(..., start_new_session=True) for this use:
# sandbox-stop signals the printed pid with TERM/KILL, never HUP.
trap '' HUP
PATH="$path" "$bin" supervisor 0 --no-daily </dev/null >>"$log" 2>&1 &
printf '%s\n' "$!"
