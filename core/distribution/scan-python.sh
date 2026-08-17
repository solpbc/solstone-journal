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
find / \
	\( -path /proc -o -path /sys -o -path /dev -o -path /run \
	   -o -path /System/Volumes/Data -o -path /System/Volumes/Preboot \
	   -o -path /System/Volumes/VM -o -path /System/Volumes/Update \) -prune -o \
	\( -type f -o -type l -o -type d \) \
	\( -name 'python*' -o -name 'libpython*' -o -name pyvenv.cfg \
	   -o -name site-packages -o -name dist-packages \) \
	-print >"$candidates"

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
			if "$path" -c 'import sys' >/dev/null 2>&1; then
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
