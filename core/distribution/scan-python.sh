#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Print installed or runnable Python-runtime artifacts. Ordinary source files
# ending in .py are intentionally not evidence of an interpreter.

set -eu

scan_tmp_root=${TMPDIR:-/var/tmp}
case $scan_tmp_root in
/*) ;;
*)
	printf 'scan-python: TMPDIR must be absolute: %s\n' "$scan_tmp_root" >&2
	exit 2
	;;
esac

candidates=$(mktemp "$scan_tmp_root/solstone-python-scan.XXXXXX")
matches=$(mktemp "$scan_tmp_root/solstone-python-matches.XXXXXX")
trap 'rm -f "$candidates" "$matches"' 0 1 2 15

# Pseudo-filesystems are not image contents and may be infinite, volatile, or
# unreadable. Every other mounted and rootfs path is traversed, including any
# source, dependency, artifact, and product mounts present for the rung.
#
# The macOS entries are firmlink duplicates of the root view rather than
# separate content: /System/Volumes/Data is the same tree reached a second
# way, so traversing it doubles the scan without reaching one new file.
# /Volumes is deliberately NOT pruned - an interpreter on a mounted image is
# on the host and must be reported.
#
# 🔴 TRAVERSE PER ROOT, AND REPORT AN INCOMPLETE WALK.
# A single `find /` is not survivable on macOS: the first TCC-protected
# directory makes `find` abort the WHOLE traversal with `fts_read: Permission
# denied`, and everything after it is silently never visited. Measured
# 2026-08-17 on pro5e — a host running Python 3.14.6 at two paths — where one
# `find /` produced **zero** classified findings and 281 lines of permission
# errors, because the walk died before reaching either `/usr/bin` or
# `/opt/homebrew/bin`. ⛔ That zero is indistinguishable from a clean host.
# Splitting the walk contains the damage, and `scan-incomplete` makes what was
# NOT reached part of the output rather than an absence.
# Does this candidate actually behave as an interpreter?
#
# ⚠ BOUNDED, and with stdin closed. A candidate is an arbitrary executable the
# walk happened to find, and one of them will hang: measured 2026-08-17, an
# embedded interpreter inside an app sitting in the Trash blocked forever on
# `-c 'import sys'` and stalled the whole census behind it. ⛔ `timeout` is not
# available on a base macOS install, so the bound is a watchdog rather than a
# tool.
runs_as_interpreter() {
	"$1" -c 'import sys' </dev/null >/dev/null 2>&1 &
	probe=$!
	( sleep 5; kill -9 "$probe" 2>/dev/null ) >/dev/null 2>&1 &
	watchdog=$!
	if wait "$probe" 2>/dev/null; then
		kill "$watchdog" 2>/dev/null
		return 0
	fi
	kill "$watchdog" 2>/dev/null
	return 1
}

scan_root() {
	find "$1" \
		\( -path /proc -o -path /sys -o -path /dev -o -path /run \
		   -o -path /System/Volumes/Data -o -path /System/Volumes/Preboot \
		   -o -path /System/Volumes/VM -o -path /System/Volumes/Update \) -prune -o \
		\( -type f -o -type l -o -type d \) \
		\( -name 'python*' -o -name 'libpython*' -o -name pyvenv.cfg \
		   -o -name site-packages -o -name dist-packages \) \
		-print 2>/dev/null
}

# Descend on failure rather than abandoning a whole root.
#
# ⛔ Marking `/usr` incomplete because one file under it is unreadable throws
# away `/usr/bin`, which is where the interpreter this census exists to find
# would be. Measured 2026-08-17: a per-root split still left `/Library`,
# `/System`, `/private` and `/usr` unwalked on a real Mac. Recursing narrows an
# abort to the smallest unreadable subtree, so `scan-incomplete` names a leaf
# nobody can read rather than a root nobody looked at.
scan_tree() {
	tree=$1
	depth=$2
	if scan_root "$tree" >>"$candidates"; then
		return 0
	fi
	if [ "$depth" -le 0 ]; then
		printf 'scan-incomplete %s\n' "$tree" >>"$matches"
		return 0
	fi
	children=$(find "$tree" -mindepth 1 -maxdepth 1 -type d 2>/dev/null)
	if [ -z "$children" ]; then
		printf 'scan-incomplete %s\n' "$tree" >>"$matches"
		return 0
	fi
	# Files directly inside this level are still candidates.
	find "$tree" -mindepth 1 -maxdepth 1 \( -type f -o -type l \) \
		\( -name 'python*' -o -name 'libpython*' -o -name pyvenv.cfg \) \
		-print 2>/dev/null >>"$candidates"
	printf '%s\n' "$children" | while IFS= read -r child; do
		[ -n "$child" ] || continue
		scan_tree "$child" $((depth - 1))
	done
}

for root in / /*; do
	[ "$root" = "/" ] && continue
	case $root in
	/proc | /sys | /dev | /run) continue ;;
	esac
	scan_tree "$root" 5
done
# Files sitting directly in / are not covered by the per-child walk above.
find / -maxdepth 1 \( -type f -o -type l \) \
	\( -name 'python*' -o -name 'libpython*' \) -print 2>/dev/null >>"$candidates"

while IFS= read -r path; do
	base=${path##*/}
	case $base in
	python | python3 | python[0-9] | python[0-9].[0-9] | python[0-9].[0-9][0-9])
		if [ -f "$path" ] && [ -x "$path" ]; then
			# An executable file named python3 is not necessarily an
			# interpreter. macOS ships /usr/bin/python3 as a Command Line
			# Tools SHIM on every install, including one that has never had
			# Xcode: it is a regular file, it is +x, and running it without
			# CLT fails with `xcrun: error: invalid active developer path`.
			# Classifying it by name and mode alone reports a Python runtime
			# on a host that has none — a false POSITIVE, which on the
			# zero-Python subject is the one direction that reads as a
			# correctly-firing instrument rather than as a bug.
			if runs_as_interpreter "$path"; then
				printf 'executable %s\n' "$path" >>"$matches"
			else
				printf 'shim %s\n' "$path" >>"$matches"
			fi
		elif [ -d "$path" ] && [ -f "$path/os.py" ] && [ -f "$path/encodings/__init__.py" ]; then
			printf 'stdlib %s\n' "$path" >>"$matches"
		fi
		;;
	libpython*.so | libpython*.so.* | libpython*.dylib)
		[ -f "$path" ] && printf 'library %s\n' "$path" >>"$matches"
		;;
	pyvenv.cfg)
		[ -f "$path" ] && printf 'venv %s\n' "$path" >>"$matches"
		;;
	site-packages | dist-packages)
		[ -d "$path" ] && printf 'packages %s\n' "$path" >>"$matches"
		;;
	python[0-9]* )
		if [ -d "$path" ] && [ -f "$path/os.py" ] && [ -f "$path/encodings/__init__.py" ]; then
			printf 'stdlib %s\n' "$path" >>"$matches"
		fi
		;;
	esac
done <"$candidates"

LC_ALL=C sort -u "$matches"
