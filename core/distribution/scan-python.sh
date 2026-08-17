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
find / \
	\( -path /proc -o -path /sys -o -path /dev -o -path /run \) -prune -o \
	\( -type f -o -type l -o -type d \) \
	\( -name 'python*' -o -name 'libpython*' -o -name pyvenv.cfg \
	   -o -name site-packages -o -name dist-packages \) \
	-print >"$candidates"

while IFS= read -r path; do
	base=${path##*/}
	case $base in
	python | python3 | python[0-9] | python[0-9].[0-9] | python[0-9].[0-9][0-9])
		if [ -f "$path" ] && [ -x "$path" ]; then
			printf 'executable %s\n' "$path" >>"$matches"
		elif [ -d "$path" ] && [ -f "$path/os.py" ] && [ -f "$path/encodings/__init__.py" ]; then
			printf 'stdlib %s\n' "$path" >>"$matches"
		fi
		;;
	libpython*.so | libpython*.so.*)
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
