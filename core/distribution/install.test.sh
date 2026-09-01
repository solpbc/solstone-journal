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
	_out=$(mktemp "$BASE/solstone-install-test-output-XXXXXX")
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

TMP_ROOT=${TMPDIR:-/var/tmp}
if [ ! -d "$TMP_ROOT" ] || [ ! -w "$TMP_ROOT" ]; then
	printf '%s\n' "tmpdir-unusable: $TMP_ROOT" >&2
	exit 1
fi
BASE=$(mktemp -d "$TMP_ROOT/solstone-install-test-XXXXXX") || {
	printf '%s\n' "tmpdir-unusable: $TMP_ROOT" >&2
	exit 1
}
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

HAPPY_OUT=$BASE/happy.out
if ! env HOME="$HOME" SOLSTONE_PROFILE="$HOME/.profile" \
	"$INSTALL" --prefix "$PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" >"$HAPPY_OUT"; then
	fail "happy-path install"
else
	pass "happy-path install"
fi
if grep -F "installed solstone-journal 1.0.22 at $PREFIX" "$HAPPY_OUT" >/dev/null \
	&& grep -F "current ->" "$HAPPY_OUT" >/dev/null \
	&& grep -F "PATH updated in $HOME/.profile" "$HAPPY_OUT" >/dev/null \
	&& grep -F "then: journal --version" "$HAPPY_OUT" >/dev/null; then
	pass "success prints version, prefix, and PATH"
else
	fail "success prints version, prefix, and PATH: $(cat "$HAPPY_OUT")"
fi

# temp-root resolution and cleanup
REAL_MKTEMP=$(command -v mktemp)
case $REAL_MKTEMP in
/*) ;;
*)
	fail "mktemp shim requires an absolute mktemp path: $REAL_MKTEMP"
	exit 1
	;;
esac
MKTEMP_SHIM_DIR=$BASE/mktemp-shim
mkdir -p "$MKTEMP_SHIM_DIR"
{
	printf '%s\n' '#!/bin/sh'
	printf '%s\n' "[ -z \"\${SOLSTONE_MKTEMP_LOG:-}\" ] || printf '%s\\n' \"\$*\" >>\"\$SOLSTONE_MKTEMP_LOG\""
	printf 'exec "%s" "$@"\n' "$REAL_MKTEMP"
} >"$MKTEMP_SHIM_DIR/mktemp"
chmod 755 "$MKTEMP_SHIM_DIR/mktemp"

DEFAULT_TMP_LOG=$BASE/default-tmp.log
DEFAULT_TMP_HOME=$BASE/default-tmp-home
mkdir -p "$DEFAULT_TMP_HOME"
if (
	unset TMPDIR
	SOLSTONE_MKTEMP_LOG="$DEFAULT_TMP_LOG" PATH="$MKTEMP_SHIM_DIR:$PATH" \
		HOME="$DEFAULT_TMP_HOME" SOLSTONE_PROFILE="$DEFAULT_TMP_HOME/.profile" \
		"$INSTALL" --prefix "$BASE/default-tmp-prefix" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL"
); then
	if awk '$0 ~ /^-d \/var\/tmp\/solstone-install-work-/ { found=1 } END { exit !found }' "$DEFAULT_TMP_LOG"; then
		pass "default TMPDIR uses var tmp work template"
	else
		fail "default TMPDIR uses var tmp work template"
	fi
else
	fail "default TMPDIR install"
fi

TMPDIR_PROBE=$BASE/tmpdir-probe
mkdir -p "$TMPDIR_PROBE"
PROBE_TMP_LOG=$BASE/probe-tmp.log
PROBE_TMP_HOME=$BASE/probe-tmp-home
mkdir -p "$PROBE_TMP_HOME"
if TMPDIR="$TMPDIR_PROBE" SOLSTONE_MKTEMP_LOG="$PROBE_TMP_LOG" PATH="$MKTEMP_SHIM_DIR:$PATH" \
	HOME="$PROBE_TMP_HOME" SOLSTONE_PROFILE="$PROBE_TMP_HOME/.profile" \
	"$INSTALL" --prefix "$BASE/probe-tmp-prefix" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL"; then
	if awk -v prefix="-d $TMPDIR_PROBE/solstone-install-work-" 'index($0, prefix) == 1 { found=1 } END { exit !found }' "$PROBE_TMP_LOG"; then
		pass "explicit TMPDIR uses probe work template"
	else
		fail "explicit TMPDIR uses probe work template"
	fi
else
	fail "explicit TMPDIR install"
fi

if [ -z "$(ls "$TMPDIR_PROBE")" ]; then
	pass "successful install cleans TMPDIR probe"
else
	fail "successful install leaves TMPDIR probe contents"
fi

UNUSABLE_TMPDIR=$BASE/tmpdir-not-a-directory
printf 'not a directory\n' >"$UNUSABLE_TMPDIR"
expect_refuse tmpdir-unusable tmpdir-unusable \
	env TMPDIR="$UNUSABLE_TMPDIR" HOME="$HOME" \
	"$INSTALL" --prefix "$BASE/unusable-tmp-prefix" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL"

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

# reordered sidecar: match the archive filename, not the first line
_digest=$(sha256sum "$ARCHIVE" | awk '{print $1}')
printf '%s\n' \
	"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  other.deb" \
	"${_digest}  good.tar.gz" \
	"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  other.rpm" \
	>"$BASE/reordered.sha256"
REORDER_HOME=$BASE/reorder-home
mkdir -p "$REORDER_HOME"
if env HOME="$REORDER_HOME" SOLSTONE_PROFILE="$REORDER_HOME/.profile" \
	"$INSTALL" --prefix "$BASE/reorder-prefix" --archive "$ARCHIVE" --sha256 "$BASE/reordered.sha256" --release "$REL"; then
	pass "reordered sidecar"
else
	fail "reordered sidecar"
fi
printf '%s\n' \
	"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  other.deb" \
	"${_digest}  other.tar.gz" \
	>"$BASE/wrong-name.sha256"
expect_refuse digest-mismatch sidecar-filename \
	env HOME="$REORDER_HOME" \
	"$INSTALL" --prefix "$BASE/wrong-prefix" --archive "$ARCHIVE" --sha256 "$BASE/wrong-name.sha256" --release "$REL"

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

_help=$("$INSTALL" --help)
case $_help in
*--no-path*) pass "usage names --no-path" ;;
*) fail "usage names --no-path: $_help" ;;
esac

NO_PATH_HOME=$BASE/nopath-home
mkdir -p "$NO_PATH_HOME"
printf 'keep-profile\n' >"$NO_PATH_HOME/.profile"
NO_PATH_PREFIX=$BASE/nopath-prefix
if env HOME="$NO_PATH_HOME" \
	"$INSTALL" --no-path --prefix "$NO_PATH_PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" \
	>"$BASE/nopath.out" 2>&1; then
	if [ -x "$NO_PATH_PREFIX/current/bin/journal" ]; then
		pass "no-path install tree"
	else
		fail "no-path install tree"
	fi
	if grep -q 'BEGIN solstone-journal PATH' "$NO_PATH_HOME/.profile"; then
		fail "no-path left PATH block"
	elif [ "$(cat "$NO_PATH_HOME/.profile")" = "keep-profile" ]; then
		pass "no-path preserves profile"
	else
		fail "no-path mutated profile"
	fi
	if grep -q 'PATH not updated (--no-path)' "$BASE/nopath.out"; then
		pass "no-path reports skip"
	else
		fail "no-path reports skip: $(cat "$BASE/nopath.out")"
	fi
else
	fail "no-path install: $(cat "$BASE/nopath.out")"
fi

# digest-mismatch
printf '%s\n' "0000000000000000000000000000000000000000000000000000000000000000  good.tar.gz" >"$BASE/bad.sha256"
expect_refuse digest-mismatch digest-bad \
	env HOME="$HOME" \
	"$INSTALL" --prefix "$PREFIX" --archive "$ARCHIVE" --sha256 "$BASE/bad.sha256" --release "$REL"

# a refusal must never touch the existing install
if [ -x "$PREFIX/current/bin/journal" ] && [ "$(cat "$PREFIX/current/bin/journal")" = "ok" ]; then
	pass "digest-mismatch refusal left previous current usable"
else
	fail "digest-mismatch refusal left previous current usable"
fi

REFUSAL_TMPDIR_PROBE=$BASE/tmpdir-refusal-probe
mkdir -p "$REFUSAL_TMPDIR_PROBE"
_status=0
TMPDIR="$REFUSAL_TMPDIR_PROBE" SOLSTONE_PROFILE="$HOME/.profile" \
	"$INSTALL" --prefix "$BASE/refusal-tmp-prefix" --archive "$ARCHIVE" --sha256 "$BASE/bad.sha256" --release "$REL" \
	>"$BASE/tmpdir-refusal.out" 2>&1 || _status=$?
_text=$(cat "$BASE/tmpdir-refusal.out")
if [ "$_status" -eq 0 ]; then
	fail "refusal cleanup TMPDIR probe: expected digest-mismatch, succeeded"
elif [ -n "$(ls "$REFUSAL_TMPDIR_PROBE")" ]; then
	fail "refusal cleanup TMPDIR probe: probe not empty"
else
	case $_text in
	*digest-mismatch*) pass "refusal cleans TMPDIR probe" ;;
	*) fail "refusal cleanup TMPDIR probe: wanted digest-mismatch in: $_text" ;;
	esac
fi

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

# same version, different bytes: a respin before release, which install.sh
# must accept as an upgrade in place -- not refuse. This used to be the
# `conflicting-digest` refusal, and refusing it was the ship blocker: a
# legitimate rebuild of the version already on disk could never be installed
# by the documented route. install.sh's directories are content-addressed by
# `${VERSION}-${DIGEST12}` (see DEST above), so two builds of one version are
# never actually a collision on disk -- only a policy refusal that treated
# them as one. The digest/release-record checks earlier in this file (see the
# digest-mismatch and version-mismatch tests above) are what keep verifying a
# tampered or foreign artifact; neither is touched by this change.
_prev_current=$(readlink "$PREFIX/current")
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
if env HOME="$HOME" \
	"$INSTALL" --prefix "$PREFIX" --archive "$ARCHIVE2" --sha256 "$SHA2" --release "$REL2" \
	>"$BASE/respin.out" 2>&1; then
	pass "same-version respin installs"
else
	fail "same-version respin installs: $(cat "$BASE/respin.out")"
fi
if [ "$(cat "$PREFIX/current/bin/journal" 2>/dev/null)" = "other" ]; then
	pass "same-version respin flips current to the new build"
else
	fail "same-version respin flips current to the new build"
fi
if [ -x "$PREFIX/$_prev_current/bin/journal" ] && [ "$(cat "$PREFIX/$_prev_current/bin/journal")" = "ok" ]; then
	pass "same-version respin keeps the prior build's version directory"
else
	fail "same-version respin keeps the prior build's version directory"
fi

# negative twin: a tarball whose digest does not match its own .sha256 must
# still be refused, even when it is a same-version respin sitting right next
# to an installable one.
printf '%s\n' "0000000000000000000000000000000000000000000000000000000000000000  other.tar.gz" >"$BASE/respin-bad.sha256"
expect_refuse digest-mismatch respin-digest-bad \
	env HOME="$HOME" \
	"$INSTALL" --prefix "$PREFIX" --archive "$ARCHIVE2" --sha256 "$BASE/respin-bad.sha256" --release "$REL2"

# negative twin: a .release record whose version disagrees with what was
# requested must still be refused, even for a same-version respin.
make_release "$BASE/respin-wrong-version.release" 9.9.9 "$TARGET"
expect_refuse version-mismatch respin-version-mismatch \
	env HOME="$HOME" \
	"$INSTALL" --prefix "$PREFIX" --version 1.0.22 --archive "$ARCHIVE2" --sha256 "$SHA2" \
	--release "$BASE/respin-wrong-version.release"

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
BUILD_LOG=$BASE/solstone-dist-build.log
if [ ! -x "$BIN" ]; then
	cargo build --manifest-path "$ROOT/core/Cargo.toml" -p solstone-core-distribution --bin solstone-distribution --offline >"$BUILD_LOG" 2>&1 || true
fi
if [ -x "$BIN" ]; then
	SERVE=$BASE/origin
	_base=solstone-journal-3.0.0-${TARGET}
	_object=$SERVE/solstone-journal/release/3.0.0
	mkdir -p "$_object"
	cp "$ARCHIVE" "$_object/${_base}.tar.gz"
	make_release "$_object/${_base}.release" 3.0.0 "$TARGET"
	sha_sidecar "$_object/${_base}.tar.gz" "$_object/${_base}.sha256"
	printf 'version=3.0.0\n' >"$SERVE/solstone-journal/release/latest"
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
			"$INSTALL" --prefix "$BASE/net-prefix" --origin "http://${_addr}"; then
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
	tail -n 20 "$BUILD_LOG" >&2 || true
fi

say "passed=$PASSES failed=$FAILS"
[ "$FAILS" -eq 0 ]
