#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# POSIX tests for core/distribution/install.sh.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
INSTALL=$ROOT/core/distribution/install.sh
FAILS=0
PASSES=0

say() {
	printf '%s\n' "$*"
}

fail() {
	FAILS=$((FAILS + 1))
	printf 'FAIL %s\n' "$*" >&2
}

pass() {
	PASSES=$((PASSES + 1))
	printf 'ok %s\n' "$*"
}

expect_refuse() {
	_want=$1
	_name=$2
	shift 2
	_out=$(mktemp)
	_status=0
	"$@" >"$_out" 2>&1 || _status=$?
	_text=$(cat "$_out")
	rm -f "$_out"
	if [ "$_status" -eq 0 ]; then
		fail "$_name: expected refusal $_want, succeeded"
		return
	fi
	case $_text in
	*${_want}*) pass "$_name" ;;
	*) fail "$_name: wanted $_want in: $_text" ;;
	esac
}

make_release() {
	_dest=$1
	_version=$2
	_target=$3
	printf '%s\n' \
		"product=solstone-journal" \
		"version=${_version}" \
		"target=${_target}" \
		"commit=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" \
		"lock_sha256=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" \
		>"$_dest"
}

make_tree_tar() {
	_dest=$1
	_stage=$2
	mkdir -p "$_stage/bin"
	printf 'ok\n' >"$_stage/bin/journal"
	chmod 755 "$_stage/bin/journal"
	tar -C "$_stage" -czf "$_dest" bin
}

sha_sidecar() {
	_archive=$1
	_dest=$2
	sha256sum "$_archive" >"$_dest"
}

BASE=$(mktemp -d)
trap 'rm -rf "$BASE"' 0 1 2 15

HOST_ARCH=$(uname -m)
case $HOST_ARCH in
x86_64 | amd64) TARGET=linux-x86_64 ;;
aarch64 | arm64) TARGET=linux-aarch64 ;;
*)
	say "skip: unsupported host arch $HOST_ARCH"
	exit 0
	;;
esac

# unsupported-platform
expect_refuse unsupported-platform platform-os \
	env SOLSTONE_UNAME_S=Darwin SOLSTONE_UNAME_M=x86_64 HOME="$BASE/home" \
	"$INSTALL" --prefix "$BASE/p" --version 1.0.22 --archive /nope --sha256 /nope --release /nope

expect_refuse unsupported-platform platform-arch \
	env SOLSTONE_UNAME_S=Linux SOLSTONE_UNAME_M=ppc64 HOME="$BASE/home" \
	"$INSTALL" --prefix "$BASE/p" --version 1.0.22 --archive /nope --sha256 /nope --release /nope

# origin-refused
expect_refuse origin-refused origin-http-public \
	env HOME="$BASE/home" \
	"$INSTALL" --prefix "$BASE/p" --version 1.0.22 --origin http://updates.solstone.app

expect_refuse origin-refused origin-other-host \
	env HOME="$BASE/home" \
	"$INSTALL" --prefix "$BASE/p" --version 1.0.22 --origin https://example.com

# fetcher-missing
BINDIR=$BASE/nofetch
mkdir -p "$BINDIR"
for cmd in sh tar sha256sum uname mktemp mkdir cp mv ln cat awk chmod wc tr cut printf dirname rm ls env; do
	_path=$(command -v "$cmd" || true)
	[ -n "$_path" ] && ln -s "$_path" "$BINDIR/$cmd"
done
expect_refuse fetcher-missing fetcher-absent \
	env PATH="$BINDIR" HOME="$BASE/home" SOLSTONE_UNAME_S=Linux SOLSTONE_UNAME_M="$HOST_ARCH" \
	"$INSTALL" --prefix "$BASE/p" --version 1.0.22 --origin https://updates.solstone.app

# good local archive
STAGE=$BASE/stage
ARCHIVE=$BASE/good.tar.gz
SHA=$BASE/good.sha256
REL=$BASE/good.release
make_tree_tar "$ARCHIVE" "$STAGE"
sha_sidecar "$ARCHIVE" "$SHA"
make_release "$REL" 1.0.22 "$TARGET"
HOME=$BASE/home
mkdir -p "$HOME"
PREFIX=$BASE/prefix
mkdir -p "$PREFIX"
printf 'keep\n' >"$PREFIX/unrelated"
chmod 640 "$PREFIX/unrelated"

if ! env HOME="$HOME" SOLSTONE_PROFILE="$HOME/.profile" \
	"$INSTALL" --prefix "$PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL"; then
	fail "happy-path install"
else
	pass "happy-path install"
fi

if [ -L "$PREFIX/current" ] && [ -x "$PREFIX/current/bin/journal" ]; then
	pass "current symlink and bin"
else
	fail "current symlink and bin"
fi

if [ "$(cat "$PREFIX/unrelated")" = "keep" ]; then
	pass "unrelated content preserved"
else
	fail "unrelated content preserved"
fi
_mode=$(ls -l "$PREFIX/unrelated" | awk '{print $1}')
case $_mode in
*rw-r-----* | -rw-r-----*) pass "unrelated mode preserved" ;;
*) fail "unrelated mode preserved: $_mode" ;;
esac

# profile block
if grep -q 'BEGIN solstone-journal PATH' "$HOME/.profile"; then
	pass "profile marked block"
else
	fail "profile marked block"
fi
_oldpath=$PATH
PATH=/usr/bin:/bin
# shellcheck disable=SC1090
. "$HOME/.profile"
case $PATH in
*"$PREFIX/current/bin"*) pass "fresh-login PATH" ;;
*) fail "fresh-login PATH: $PATH" ;;
esac
PATH=$_oldpath

_count=$(grep -c 'BEGIN solstone-journal PATH' "$HOME/.profile")
# idempotent reinstall
_before=$(readlink "$PREFIX/current")
if ! env HOME="$HOME" SOLSTONE_PROFILE="$HOME/.profile" \
	"$INSTALL" --prefix "$PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL"; then
	fail "idempotent reinstall"
else
	pass "idempotent reinstall"
fi
_after=$(readlink "$PREFIX/current")
if [ "$_before" = "$_after" ]; then
	pass "idempotent current unchanged"
else
	fail "idempotent current rewritten"
fi
_count2=$(grep -c 'BEGIN solstone-journal PATH' "$HOME/.profile")
if [ "$_count" -eq 1 ] && [ "$_count2" -eq 1 ]; then
	pass "profile block not duplicated"
else
	fail "profile block count $_count -> $_count2"
fi

# digest-mismatch
printf '%s\n' "0000000000000000000000000000000000000000000000000000000000000000  good.tar.gz" >"$BASE/bad.sha256"
expect_refuse digest-mismatch digest-bad \
	env HOME="$HOME" \
	"$INSTALL" --prefix "$PREFIX" --archive "$ARCHIVE" --sha256 "$BASE/bad.sha256" --release "$REL"

# release-invalid
printf '%s\n' "product=solstone-journal" >"$BASE/short.release"
expect_refuse release-invalid release-short \
	env HOME="$HOME" \
	"$INSTALL" --prefix "$BASE/other" --archive "$ARCHIVE" --sha256 "$SHA" --release "$BASE/short.release"

# version-mismatch
make_release "$BASE/other.release" 9.9.9 "$TARGET"
expect_refuse version-mismatch version-other \
	env HOME="$HOME" \
	"$INSTALL" --prefix "$BASE/other" --version 1.0.22 --archive "$ARCHIVE" --sha256 "$SHA" --release "$BASE/other.release"

# conflicting-digest: same version, different bytes
STAGE2=$BASE/stage2
ARCHIVE2=$BASE/other.tar.gz
SHA2=$BASE/other.sha256
REL2=$BASE/other2.release
mkdir -p "$STAGE2/bin"
printf 'other\n' >"$STAGE2/bin/journal"
chmod 755 "$STAGE2/bin/journal"
tar -C "$STAGE2" -czf "$ARCHIVE2" bin
sha_sidecar "$ARCHIVE2" "$SHA2"
make_release "$REL2" 1.0.22 "$TARGET"
expect_refuse conflicting-digest conflict \
	env HOME="$HOME" \
	"$INSTALL" --prefix "$PREFIX" --archive "$ARCHIVE2" --sha256 "$SHA2" --release "$REL2"

# failed upgrade leaves previous current
_prev=$(readlink "$PREFIX/current")
if [ -x "$PREFIX/current/bin/journal" ] && [ "$(cat "$PREFIX/current/bin/journal")" = "ok" ]; then
	pass "failed upgrade left previous current usable"
else
	fail "failed upgrade left previous current usable (current=$_prev)"
fi

# archive-absolute-path
ABS=$BASE/abs
mkdir -p "$ABS"
printf 'x\n' >"$ABS/file"
tar --absolute-names -czf "$BASE/abs.tar.gz" -C / "$(echo "$ABS/file" | sed 's|^/||')" 2>/dev/null || \
	tar -P -czf "$BASE/abs.tar.gz" "$ABS/file"
if tar -tzf "$BASE/abs.tar.gz" | grep -q '^/'; then
	sha_sidecar "$BASE/abs.tar.gz" "$BASE/abs.sha256"
	make_release "$BASE/abs.release" 2.0.0 "$TARGET"
	expect_refuse archive-absolute-path abs-member \
		env HOME="$HOME" \
		"$INSTALL" --prefix "$BASE/abs-prefix" --archive "$BASE/abs.tar.gz" --sha256 "$BASE/abs.sha256" --release "$BASE/abs.release"
else
	# GNU tar may strip; synthesize a header name with a leading slash via --transform
	tar -C "$ABS" --transform='s|^file|/etc/solstone-abs|' -czf "$BASE/abs.tar.gz" file
	if tar -tzf "$BASE/abs.tar.gz" | grep -q '^/'; then
		sha_sidecar "$BASE/abs.tar.gz" "$BASE/abs.sha256"
		make_release "$BASE/abs.release" 2.0.0 "$TARGET"
		expect_refuse archive-absolute-path abs-member \
			env HOME="$HOME" \
			"$INSTALL" --prefix "$BASE/abs-prefix" --archive "$BASE/abs.tar.gz" --sha256 "$BASE/abs.sha256" --release "$BASE/abs.release"
	else
		fail "archive-absolute-path: could not craft absolute member"
	fi
fi

# archive-parent-traversal
tar -C "$STAGE" --transform='s|^bin|../bin|' -czf "$BASE/dotdot.tar.gz" bin
sha_sidecar "$BASE/dotdot.tar.gz" "$BASE/dotdot.sha256"
make_release "$BASE/dotdot.release" 2.0.1 "$TARGET"
expect_refuse archive-parent-traversal parent-member \
	env HOME="$HOME" \
	"$INSTALL" --prefix "$BASE/dot-prefix" --archive "$BASE/dotdot.tar.gz" --sha256 "$BASE/dotdot.sha256" --release "$BASE/dotdot.release"

# archive-symlink-escape
SYM=$BASE/sym
mkdir -p "$SYM"
ln -s /etc/passwd "$SYM/link"
tar -C "$SYM" -czf "$BASE/sym.tar.gz" link
sha_sidecar "$BASE/sym.tar.gz" "$BASE/sym.sha256"
make_release "$BASE/sym.release" 2.0.2 "$TARGET"
expect_refuse archive-symlink-escape symlink-abs \
	env HOME="$HOME" \
	"$INSTALL" --prefix "$BASE/sym-prefix" --archive "$BASE/sym.tar.gz" --sha256 "$BASE/sym.sha256" --release "$BASE/sym.release"

# archive-hardlink-escape
HL=$BASE/hl
mkdir -p "$HL"
printf 'x\n' >"$HL/a"
ln "$HL/a" "$HL/b"
tar -C "$HL" -czf "$BASE/hl.tar.gz" a b
sha_sidecar "$BASE/hl.tar.gz" "$BASE/hl.sha256"
make_release "$BASE/hl.release" 2.0.3 "$TARGET"
expect_refuse archive-hardlink-escape hardlink \
	env HOME="$HOME" \
	"$INSTALL" --prefix "$BASE/hl-prefix" --archive "$BASE/hl.tar.gz" --sha256 "$BASE/hl.sha256" --release "$BASE/hl.release"

# archive-symlink-then-child
SC=$BASE/sc
mkdir -p "$SC"
ln -s target "$SC/dir"
printf 'z\n' >"$SC/file"
tar -C "$SC" -czf "$BASE/sc.tar.gz" dir --transform='s|^file|dir/file|' file
sha_sidecar "$BASE/sc.tar.gz" "$BASE/sc.sha256"
make_release "$BASE/sc.release" 2.0.4 "$TARGET"
expect_refuse archive-symlink-then-child symlink-child \
	env HOME="$HOME" \
	"$INSTALL" --prefix "$BASE/sc-prefix" --archive "$BASE/sc.tar.gz" --sha256 "$BASE/sc.sha256" --release "$BASE/sc.release"

# loopback serve + fetch (not a second origin; digest still verified)
BIN=$ROOT/core/target/debug/solstone-distribution
if [ ! -x "$BIN" ]; then
	cargo build --manifest-path "$ROOT/core/Cargo.toml" -p solstone-core-distribution --bin solstone-distribution --offline >/tmp/solstone-dist-build.log 2>&1 || true
fi
if [ -x "$BIN" ]; then
	SERVE=$BASE/origin
	mkdir -p "$SERVE"
	_base=solstone-journal-3.0.0-${TARGET}
	cp "$ARCHIVE" "$SERVE/${_base}.tar.gz"
	cp "$SHA" "$SERVE/${_base}.tar.gz.sha256"
	make_release "$SERVE/${_base}.release" 3.0.0 "$TARGET"
	# rebuild sha sidecar name to match fetched file
	sha_sidecar "$SERVE/${_base}.tar.gz" "$SERVE/${_base}.tar.gz.sha256"
	"$BIN" cleanroom-serve "$SERVE" >"$BASE/serve.out" 2>"$BASE/serve.err" &
	_srv=$!
	_i=0
	while [ "$_i" -lt 50 ]; do
		[ -s "$BASE/serve.out" ] && break
		_i=$((_i + 1))
		sleep 1
	done
	_addr=$(head -1 "$BASE/serve.out")
	if [ -n "$_addr" ]; then
		if env HOME="$HOME" \
			"$INSTALL" --prefix "$BASE/net-prefix" --version 3.0.0 --origin "http://${_addr}"; then
			if [ -x "$BASE/net-prefix/current/bin/journal" ]; then
				pass "loopback fetch install"
			else
				fail "loopback fetch install: missing bin"
			fi
		else
			fail "loopback fetch install"
		fi
	else
		fail "loopback serve produced no address: $(cat "$BASE/serve.err")"
	fi
	kill "$_srv" 2>/dev/null || true
	wait "$_srv" 2>/dev/null || true
else
	fail "loopback fetch install: distribution binary missing"
fi

say "passed=$PASSES failed=$FAILS"
[ "$FAILS" -eq 0 ]
