#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Produce the admitted cleanroom builder from a clean checkout.
#
# The acquire image is built from the pinned rust base, then exported and
# normalized. A plain `docker create` + `docker export` is not the admitted
# rootfs: container metadata, hostnames, and file mtimes move with the
# build clock. This script is the producer of builder-rootfs.tar. The pin
# in builder-inputs.toml is whatever this script emits, not a one-off
# export from somebody's shell history.

set -eu
export LC_ALL=C

refuse() {
	printf 'acquire-builder: %s\n' "$*" >&2
	exit 2
}

ROOT=$(CDPATH='' cd -- "$(dirname "$0")/../.." && pwd)
DIST=$ROOT/core/distribution
PIN=$DIST/builder-inputs.toml
RUNTIME=${SOLSTONE_CONTAINER_RUNTIME:-docker}
WORKDIR=${SOLSTONE_BUILDER_WORKDIR:-$ROOT/target/cleanroom-builder}
BIN=${SOLSTONE_DISTRIBUTION_BIN:-}
STAMP_PIN=no
case ${1:-} in
--stamp-pin)
	STAMP_PIN=yes
	;;
'' ) ;;
*)
	refuse "usage: acquire-builder.sh [--stamp-pin]"
	;;
esac

command -v "$RUNTIME" >/dev/null 2>&1 || refuse "container runtime missing: $RUNTIME"
command -v tar >/dev/null 2>&1 || refuse "tar is required"
tar --help 2>&1 | grep -F -- '--sort=' >/dev/null || refuse "GNU tar with --sort is required"
command -v sha256sum >/dev/null 2>&1 || refuse "sha256sum is required"

if [ -z "$BIN" ]; then
	command -v cargo >/dev/null 2>&1 || refuse "cargo is required to build solstone-distribution"
	cargo build --manifest-path "$ROOT/core/Cargo.toml" -p solstone-core-distribution \
		--bin solstone-distribution --locked
	BIN=$ROOT/core/target/debug/solstone-distribution
fi
[ -x "$BIN" ] || refuse "producer binary is not executable: $BIN"

mkdir -p "$WORKDIR"
"$BIN" acquire builder-inputs --dest "$WORKDIR"

normalize_rootfs() {
	image=$1
	dest=$2
	scratch=$(mktemp -d "${TMPDIR:-/var/tmp}/solstone-builder-rootfs-XXXXXX")
	cid=
	cleanup() {
		if [ -n "${cid:-}" ]; then
			"$RUNTIME" rm -f "$cid" >/dev/null 2>&1 || true
		fi
		rm -rf "$scratch"
	}
	trap cleanup 0 1 2 15
	cid=$("$RUNTIME" create "$image")
	[ -n "$cid" ] || refuse "could not create a container from $image"
	"$RUNTIME" export "$cid" >"$scratch/raw.tar"
	"$RUNTIME" rm -f "$cid" >/dev/null
	cid=
	mkdir "$scratch/root"
	tar -C "$scratch/root" -xf "$scratch/raw.tar"
	rm -f "$scratch/root/.dockerenv"
	rm -f "$scratch/root/etc/hostname" "$scratch/root/etc/hosts" "$scratch/root/etc/resolv.conf"
	if [ -e "$scratch/root/etc/machine-id" ]; then
		: >"$scratch/root/etc/machine-id"
	fi
	rm -f "$scratch/root/var/lib/dbus/machine-id"
	if [ -d "$scratch/root/tmp" ]; then
		find "$scratch/root/tmp" -mindepth 1 -depth -delete
	fi
	if [ -d "$scratch/root/var/tmp" ]; then
		find "$scratch/root/var/tmp" -mindepth 1 -depth -delete
	fi
	if [ -d "$scratch/root/run" ]; then
		find "$scratch/root/run" -mindepth 1 -depth -delete
	fi
	# Zero mtimes and ownership so the admitted tar is a function of
	# file contents and names, not the clock that built the acquire
	# image. GNU format avoids pax headers that embed atime/ctime.
	tar --sort=name --mtime=@0 --owner=0 --group=0 --numeric-owner \
		--format=gnu -C "$scratch/root" -cf "$dest" .
	trap - 0 1 2 15
	rm -rf "$scratch"
}

"$RUNTIME" build \
	-f "$DIST/cleanroom-builder-acquire.Dockerfile" \
	-t solstone-cleanroom-builder-acquire \
	"$DIST"

normalize_rootfs solstone-cleanroom-builder-acquire "$WORKDIR/builder-rootfs.tar"

expected=$(awk -F' = ' '
	$1 == "filename" && $2 == "\"builder-rootfs.tar\"" { in_rootfs = 1; next }
	in_rootfs && $1 == "sha256" {
		gsub(/"/, "", $2)
		print $2
		exit
	}
' "$PIN")
[ -n "$expected" ] || refuse "builder-inputs.toml has no builder-rootfs sha256"
actual=$(sha256sum "$WORKDIR/builder-rootfs.tar" | awk '{ print $1 }')
size=$(wc -c <"$WORKDIR/builder-rootfs.tar" | tr -d ' ')
if [ "$actual" != "$expected" ]; then
	if [ "$STAMP_PIN" = yes ]; then
		tmp=$(mktemp)
		awk -F ' = ' -v digest="$actual" -v size="$size" '
			$1 == "filename" && $2 == "\"builder-rootfs.tar\"" { in_rootfs = 1 }
			in_rootfs && $1 == "sha256" {
				print "sha256 = \"" digest "\""
				next
			}
			in_rootfs && $1 == "size" {
				print "size = " size
				in_rootfs = 0
				next
			}
			{ print }
		' "$PIN" >"$tmp"
		mv "$tmp" "$PIN"
		grep -F "sha256 = \"$actual\"" "$PIN" >/dev/null \
			|| refuse "failed to stamp builder-rootfs sha256 into builder-inputs.toml"
		expected=$actual
		printf 'acquire-builder: stamped builder-rootfs pin sha256=%s size=%s\n' "$actual" "$size"
	else
		refuse "builder-rootfs digest mismatch
  expected: $expected
  actual:   $actual
  size:     $size
  dest:     $WORKDIR/builder-rootfs.tar
  repair:   this script is the producer; rerun with --stamp-pin only after two runs match"
	fi
fi

cp "$DIST/cleanroom-builder.Dockerfile" "$WORKDIR/cleanroom-builder.Dockerfile"
"$RUNTIME" build \
	-f "$WORKDIR/cleanroom-builder.Dockerfile" \
	-t solstone-cleanroom-builder \
	"$WORKDIR"

builder_id=$("$RUNTIME" image inspect --format '{{.Id}}' solstone-cleanroom-builder)
case $builder_id in
sha256:*[0-9a-f][0-9a-f]*) ;;
*) refuse "builder image id is not an immutable digest: $builder_id" ;;
esac

printf 'builder-rootfs ok sha256=%s size=%s\n' "$actual" "$size"
printf 'SOLSTONE_CLEANROOM_BUILDER_ID=%s\n' "$builder_id"
