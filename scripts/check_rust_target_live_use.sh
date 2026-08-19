#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Refuse to wipe RUST_TARGET_DIR while a live process has an open file,
# mapping, cwd, or executable under it.
set -eu

if [ "$#" -lt 1 ]; then
	echo "usage: $0 RUST_TARGET_DIR" >&2
	exit 1
fi

TARGET=$1
if [ ! -e "$TARGET" ]; then
	exit 0
fi
if command -v realpath >/dev/null 2>&1; then
	TARGET=$(realpath "$TARGET")
else
	TARGET=$(cd "$TARGET" && pwd -P)
fi

check_path() {
	pid=$1
	kind=$2
	raw=$3
	[ -n "$raw" ] || return 0
	stripped=${raw% (deleted)}
	case "$stripped" in
	"$TARGET" | "$TARGET"/*)
		echo "pid $pid $kind $stripped"
		hits=$((hits + 1))
		;;
	esac
}

os=$(uname -s)
case "$os" in
Linux)
	if [ ! -d /proc ] || ! ls /proc >/dev/null 2>&1; then
		echo "make clean: /proc is missing or unwalkable; cannot census live use of $TARGET" >&2
		exit 1
	fi
	hits=0
	# EACCES/ENOENT on foreign pids is not a census failure.
	set +e
	for proc in /proc/[0-9]*; do
		[ -d "$proc" ] || continue
		pid=${proc#/proc/}
		if cwd=$(readlink "$proc/cwd" 2>/dev/null); then
			check_path "$pid" cwd "$cwd"
		fi
		if exe=$(readlink "$proc/exe" 2>/dev/null); then
			check_path "$pid" exe "$exe"
		fi
		if [ -d "$proc/fd" ]; then
			for fd in "$proc/fd"/*; do
				[ -e "$fd" ] || [ -L "$fd" ] || continue
				if target=$(readlink "$fd" 2>/dev/null); then
					check_path "$pid" fd "$target"
				fi
			done
		fi
		if cat "$proc/maps" >/dev/null 2>&1; then
			while IFS= read -r line || [ -n "$line" ]; do
				case "$line" in
				*" /"*)
					path="/${line#* /}"
					check_path "$pid" maps "$path"
					;;
				esac
			done <"$proc/maps"
		fi
	done
	set -e
	if [ "$hits" -gt 0 ]; then
		echo "make clean: refusing to remove $TARGET; live users listed above." >&2
		echo "Use CLEAN_FORCE=1 to override." >&2
		exit 1
	fi
	exit 0
	;;
Darwin)
	if ! command -v lsof >/dev/null 2>&1; then
		echo "make clean: lsof is required on macOS to census live use of $TARGET." >&2
		echo "Install lsof or set CLEAN_FORCE=1 to override." >&2
		exit 1
	fi
	set +e
	output=$(lsof +D "$TARGET" 2>&1)
	status=$?
	set -e
	if [ "$status" -eq 0 ]; then
		echo "make clean: refusing to remove $TARGET; live users:" >&2
		echo "$output" >&2
		echo "Use CLEAN_FORCE=1 to override." >&2
		exit 1
	fi
	if [ "$status" -eq 1 ]; then
		exit 0
	fi
	echo "make clean: lsof census failed (exit $status)" >&2
	echo "$output" >&2
	exit 1
	;;
*)
	echo "make clean: live-use census is unsupported on $os (set CLEAN_FORCE=1 to override)." >&2
	exit 1
	;;
esac
