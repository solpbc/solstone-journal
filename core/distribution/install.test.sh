#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# POSIX tests for core/distribution/install.sh.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname "$0")/../.." && pwd)
INSTALL_SOURCE=$ROOT/core/distribution/install.sh
INSTALL=
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
		"upgrade_epoch=journal-v2" \
		"retention_window=3" \
		>"$_dest"
}

make_macos_release() {
	_dest=$1
	_version=$2
	make_release "$_dest" "$_version" macos-arm64
	printf '%s\n' \
		"archive_prebuild_input_sha256=cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc" \
		"archive_delivery_contract_sha256=dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd" \
		"archive_final_invocation_sha256=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee" \
		>>"$_dest"
}

make_tree_tar() {
	_dest=$1
	_stage=$2
	mkdir -p "$_stage/bin"
	printf '%s\n' \
		'#!/bin/sh' \
		'# fixture-build: ok' \
		'[ -z "${SOLSTONE_SETUP_ARGS_LOG:-}" ] || printf "%s\n" "$*" >"$SOLSTONE_SETUP_ARGS_LOG"' \
		'exit 0' >"$_stage/bin/journal"
	chmod 755 "$_stage/bin/journal"
	tar -C "$_stage" -czf "$_dest" bin
}

sha_sidecar() {
	_archive=$1
	_dest=$2
	sha256sum "$_archive" >"$_dest"
}

make_manifest() {
	_dest=$1
	_archive=$2
	_sha=$3
	_release=$4
	printf '{\n  "product": "solstone-journal",\n  "version": "1.0.22",\n  "target": "%s",\n  "files": {\n    "%s": "%s",\n    "%s": "%s",\n    "%s": "%s"\n  }\n}\n' \
		"$TARGET" \
		"${_archive##*/}" "$(sha256sum "$_archive" | awk '{print $1}')" \
		"${_sha##*/}" "$(sha256sum "$_sha" | awk '{print $1}')" \
		"${_release##*/}" "$(sha256sum "$_release" | awk '{print $1}')" \
		>"$_dest"
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

# Existing behavioral cases exercise the explicit opt-out while the dedicated
# signature cases below call INSTALL_SOURCE directly.
INSTALL=$BASE/install-with-signature-opt-out.sh
{
	printf '%s\n' '#!/bin/sh'
	printf 'exec "%s" --skip-signature "$@"\n' "$INSTALL_SOURCE"
} >"$INSTALL"
chmod 755 "$INSTALL"

HOST_ARCH=$(uname -m)
case $HOST_ARCH in
x86_64 | amd64) TARGET=linux-x86_64 ;;
aarch64 | arm64) TARGET=linux-aarch64 ;;
*)
	say "skip: unsupported host arch $HOST_ARCH"
	exit 0
	;;
esac
MINISIGN_PUBLIC_KEY_EXPECTED=$(sed -n '2p' "$ROOT/packaging/keys/solstone-journal-release.pub")
if grep -F "MINISIGN_PUBLIC_KEY=$MINISIGN_PUBLIC_KEY_EXPECTED" "$INSTALL_SOURCE" >/dev/null \
	&& grep -F 'MINISIGN_KEY_ID=B44073BF49E0D944' "$INSTALL_SOURCE" >/dev/null; then
	pass "installer pin matches the canonical product key"
else
	fail "installer pin drifted from the canonical product key"
fi

# unsupported-platform
expect_refuse unsupported-platform platform-os \
	env SOLSTONE_UNAME_S=Darwin SOLSTONE_UNAME_M=x86_64 HOME="$BASE/home" \
	"$INSTALL" --prefix "$BASE/p" --version 1.0.22 --archive /nope --sha256 /nope --release /nope

expect_refuse unsupported-platform platform-arch \
	env SOLSTONE_UNAME_S=Linux SOLSTONE_UNAME_M=ppc64 HOME="$BASE/home" \
	"$INSTALL" --prefix "$BASE/p" --version 1.0.22 --archive /nope --sha256 /nope --release /nope

expect_refuse lane-invalid lane-invalid \
	env HOME="$BASE/home" "$INSTALL" --lane nightly --prefix "$BASE/p" --version 1.0.22 --archive /nope --sha256 /nope --release /nope

# The route lock covers detection, extraction, selection, setup, receipt, and
# pruning. A live owner is never removed by a contender's cleanup.
BUSY_PREFIX=$BASE/busy-prefix
mkdir -p "$BUSY_PREFIX/.solstone-route.lock"
chmod 700 "$BUSY_PREFIX/.solstone-route.lock"
printf '%s\n' 'solstone-route-lock-v1' '0123456789abcdef0123456789abcdef' \
	>"$BUSY_PREFIX/.solstone-route.lock/owner"
chmod 600 "$BUSY_PREFIX/.solstone-route.lock/owner"
expect_refuse route-busy route-lock-is-exclusive \
	env HOME="$BASE/busy-home" "$INSTALL" --prefix "$BUSY_PREFIX" --archive /nope --sha256 /nope --release /nope
if [ -f "$BUSY_PREFIX/.solstone-route.lock/owner" ]; then
	pass "route-lock contender preserves the live owner"
else
	fail "route-lock contender removed the live owner"
fi

STALE_PREFIX=$BASE/stale-prefix
mkdir -p "$STALE_PREFIX/.solstone-route.lock"
chmod 700 "$STALE_PREFIX/.solstone-route.lock"
printf '%s\n' 'solstone-route-lock-v1' 'fedcba9876543210fedcba9876543210' \
	>"$STALE_PREFIX/.solstone-route.lock/owner"
printf '%s\n' '2147483647' >"$STALE_PREFIX/.solstone-route.lock/pid"
chmod 600 "$STALE_PREFIX/.solstone-route.lock/owner" "$STALE_PREFIX/.solstone-route.lock/pid"
expect_refuse digest-mismatch stale-route-lock-is-recovered \
	env HOME="$BASE/stale-home" "$INSTALL" --prefix "$STALE_PREFIX" --archive /nope --sha256 /nope --release /nope
if [ ! -e "$STALE_PREFIX/.solstone-route.lock" ]; then
	pass "recovered stale route lock is released"
else
	fail "stale route lock remained after recovery"
fi

# origin-refused
expect_refuse origin-refused origin-http-public \
	env HOME="$BASE/home" \
	"$INSTALL" --prefix "$BASE/p" --version 1.0.22 --origin http://updates.solstone.app

expect_refuse origin-refused origin-other-host \
	env HOME="$BASE/home" \
	"$INSTALL" --prefix "$BASE/p" --version 1.0.22 --origin https://example.com

# Latest-token validation and redirect-hop exhaustion use a deterministic curl
# seam; neither case reaches artifact fetching or installation.
HTTP_BIN=$BASE/http-bin
mkdir -p "$HTTP_BIN"
{
	printf '%s\n' '#!/bin/sh'
	printf '%s\n' 'headers=' 'dest=' 'while [ "$#" -gt 0 ]; do' '  case $1 in' '    -D) headers=$2; shift 2 ;;' '    -o) dest=$2; shift 2 ;;' '    -w) shift 2 ;;' '    -sS|--http1.1) shift ;;' '    *) shift ;;' '  esac' 'done'
	printf '%s\n' 'case ${SOLSTONE_CURL_MODE:-latest-invalid} in'
	printf '%s\n' '  latest-invalid)' "    printf 'HTTP/1.1 200 OK\\r\\n' >\"\$headers\"" "    printf 'not-a-version-token\\n' >\"\$dest\"" "    printf '200'" '    ;;'
	printf '%s\n' '  redirect)' "    printf 'HTTP/1.1 302 Found\\r\\nLocation: http://127.0.0.1/again\\r\\n' >\"\$headers\"" "    : >\"\$dest\"" "    printf '302'" '    ;;' 'esac'
} >"$HTTP_BIN/curl"
chmod 755 "$HTTP_BIN/curl"
expect_refuse latest-invalid latest-invalid \
	env PATH="$HTTP_BIN:$PATH" SOLSTONE_CURL_MODE=latest-invalid HOME="$BASE/latest-home" \
	"$INSTALL" --prefix "$BASE/latest-prefix" --origin http://127.0.0.1
expect_refuse latest-invalid redirect-hop-exhaustion \
	env PATH="$HTTP_BIN:$PATH" SOLSTONE_CURL_MODE=redirect HOME="$BASE/redirect-home" \
	"$INSTALL" --prefix "$BASE/redirect-prefix" --origin http://127.0.0.1

# fetcher-missing
BINDIR=$BASE/nofetch
mkdir -p "$BINDIR"
for cmd in sh tar sha256sum uname mktemp mkdir cp mv ln cat awk chmod wc tr cut printf dirname rm rmdir ls env od; do
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

# The same archive installer consumes the current ten-field macOS release
# record, including the three archive-chain digests.
MAC_STAGE=$BASE/mac-stage
MAC_ARCHIVE=$BASE/mac.tar.gz
MAC_SHA=$BASE/mac.sha256
MAC_REL=$BASE/mac.release
make_tree_tar "$MAC_ARCHIVE" "$MAC_STAGE"
sha_sidecar "$MAC_ARCHIVE" "$MAC_SHA"
make_macos_release "$MAC_REL" 1.0.22
if env HOME="$BASE/mac-home" SOLSTONE_UNAME_S=Darwin SOLSTONE_UNAME_M=arm64 \
	"$INSTALL" --no-path --prefix "$BASE/mac-prefix" --archive "$MAC_ARCHIVE" \
	--sha256 "$MAC_SHA" --release "$MAC_REL" >/dev/null; then
	pass "macos ten-field release installs through the archive route"
else
	fail "macos ten-field release was refused"
fi
HOME=$BASE/home
mkdir -p "$HOME"
PREFIX=$BASE/prefix
mkdir -p "$PREFIX"
printf 'keep\n' >"$PREFIX/unrelated"
chmod 640 "$PREFIX/unrelated"

# Signature verification binds the fetched archive, checksum sidecar, and
# release record to the signed manifest and uses the product pin embedded in
# the standalone installer.
SIGNED_ARCHIVE=$BASE/signed.tar.gz
SIGNED_SHA=$BASE/signed.sha256
SIGNED_REL=$BASE/signed.release
SIGNED_MANIFEST=$BASE/signed.manifest.json
SIGNED_MINISIG=$BASE/signed.manifest.json.minisig
cp "$ARCHIVE" "$SIGNED_ARCHIVE"
sha_sidecar "$SIGNED_ARCHIVE" "$SIGNED_SHA"
cp "$REL" "$SIGNED_REL"
make_manifest "$SIGNED_MANIFEST" "$SIGNED_ARCHIVE" "$SIGNED_SHA" "$SIGNED_REL"
printf 'fixture signature\n' >"$SIGNED_MINISIG"

VERIFY_BIN=$BASE/verify-bin
mkdir -p "$VERIFY_BIN"
{
	printf '%s\n' '#!/bin/sh'
	printf '%s\n' 'pin=' 'while [ "$#" -gt 0 ]; do' '  case $1 in' '    -p) pin=$2; shift 2 ;;' '    *) shift ;;' '  esac' 'done'
	printf '%s\n' '[ -n "$pin" ] || exit 1'
	printf '%s\n' "grep -F '$MINISIGN_PUBLIC_KEY_EXPECTED' \"\$pin\" >/dev/null || exit 1"
	printf '%s\n' 'exit 0'
} >"$VERIFY_BIN/minisign"
chmod 755 "$VERIFY_BIN/minisign"
if env PATH="$VERIFY_BIN:$PATH" HOME="$BASE/signed-home" \
	"$INSTALL_SOURCE" --prefix "$BASE/signed-prefix" --archive "$SIGNED_ARCHIVE" --sha256 "$SIGNED_SHA" --release "$SIGNED_REL" --manifest "$SIGNED_MANIFEST" --minisig "$SIGNED_MINISIG" >/dev/null; then
	pass "installer verifies the signed release manifest with the pinned key"
else
	fail "signed release manifest verification"
fi

cp "$SIGNED_ARCHIVE" "$BASE/signed-original.tar.gz"
printf 'tamper\n' >>"$SIGNED_ARCHIVE"
sha_sidecar "$SIGNED_ARCHIVE" "$SIGNED_SHA"
expect_refuse signature-invalid signature-binds-archive \
	env PATH="$VERIFY_BIN:$PATH" HOME="$BASE/tampered-archive-home" \
	"$INSTALL_SOURCE" --prefix "$BASE/tampered-archive-prefix" --archive "$SIGNED_ARCHIVE" --sha256 "$SIGNED_SHA" --release "$SIGNED_REL" --manifest "$SIGNED_MANIFEST" --minisig "$SIGNED_MINISIG"
cp "$BASE/signed-original.tar.gz" "$SIGNED_ARCHIVE"
sha_sidecar "$SIGNED_ARCHIVE" "$SIGNED_SHA"

FAIL_VERIFY_BIN=$BASE/fail-verify-bin
mkdir -p "$FAIL_VERIFY_BIN"
printf '%s\n' '#!/bin/sh' 'exit 1' >"$FAIL_VERIFY_BIN/minisign"
chmod 755 "$FAIL_VERIFY_BIN/minisign"
expect_refuse signature-invalid signature-refuses-tampered-manifest \
	env PATH="$FAIL_VERIFY_BIN:$PATH" HOME="$BASE/tampered-manifest-home" \
	"$INSTALL_SOURCE" --prefix "$BASE/tampered-manifest-prefix" --archive "$SIGNED_ARCHIVE" --sha256 "$SIGNED_SHA" --release "$SIGNED_REL" --manifest "$SIGNED_MANIFEST" --minisig "$SIGNED_MINISIG"

expect_refuse verifier-missing verifier-missing-names-install-command \
	env PATH="$BINDIR" HOME="$BASE/verifier-home" SOLSTONE_UNAME_S=Linux SOLSTONE_UNAME_M="$HOST_ARCH" \
	"$INSTALL_SOURCE" --prefix "$BASE/verifier-prefix" --archive "$SIGNED_ARCHIVE" --sha256 "$SIGNED_SHA" --release "$SIGNED_REL" --manifest "$SIGNED_MANIFEST" --minisig "$SIGNED_MINISIG"

HAPPY_OUT=$BASE/happy.out
if ! env HOME="$HOME" SOLSTONE_PROFILE="$HOME/.profile" \
	"$INSTALL" --prefix "$PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" >"$HAPPY_OUT"; then
	fail "happy-path install"
else
	pass "happy-path install"
fi
if grep -F "installed solstone-journal 1.0.22 at $PREFIX" "$HAPPY_OUT" >/dev/null \
	&& grep -F "lane=release" "$HAPPY_OUT" >/dev/null \
	&& grep -F "current ->" "$HAPPY_OUT" >/dev/null \
	&& grep -F "PATH updated in $HOME/.profile" "$HAPPY_OUT" >/dev/null \
	&& grep -F "then: journal --version" "$HAPPY_OUT" >/dev/null; then
	pass "success prints version, prefix, and PATH"
else
	fail "success prints version, prefix, and PATH: $(cat "$HAPPY_OUT")"
fi
if grep -F 'schema_version=1' "$PREFIX/install-receipt" >/dev/null \
	&& grep -F 'journal_version=1.0.22' "$PREFIX/install-receipt" >/dev/null \
	&& grep -F 'lane=release' "$PREFIX/install-receipt" >/dev/null \
	&& grep -F 'origin=local' "$PREFIX/install-receipt" >/dev/null \
	&& grep -F "architecture=$TARGET" "$PREFIX/install-receipt" >/dev/null \
	&& grep -F 'installer_revision=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' "$PREFIX/install-receipt" >/dev/null \
	&& grep -F 'route=tree' "$PREFIX/install-receipt" >/dev/null \
	&& grep -F 'signature_verification=skipped' "$PREFIX/install-receipt" >/dev/null \
	&& grep -F 'setup_status=complete' "$PREFIX/install-receipt" >/dev/null; then
	pass "tree install writes the forward-readable receipt"
else
	fail "tree install receipt: $(cat "$PREFIX/install-receipt" 2>/dev/null)"
fi
if [ ! -e "$PREFIX/.solstone-route.lock" ]; then
	pass "successful install releases the route lock"
else
	fail "successful install left the route lock"
fi

# A caught termination signal after the route flip must leave the candidate
# active with a pending receipt, release the lock, and make the exact same
# command retryable. It must not return to the success path.
SIGNAL_STAGE=$BASE/signal-stage
SIGNAL_ARCHIVE=$BASE/signal.tar.gz
SIGNAL_SHA=$BASE/signal.sha256
SIGNAL_REL=$BASE/signal.release
mkdir -p "$SIGNAL_STAGE/bin"
printf '%s\n' '#!/bin/sh' \
	'if [ ! -e "$HOME/.signal-fired" ]; then' \
	'  : >"$HOME/.signal-fired"' \
	'  kill -TERM "$PPID"' \
	'fi' \
	'exit 0' >"$SIGNAL_STAGE/bin/journal"
chmod 755 "$SIGNAL_STAGE/bin/journal"
tar -C "$SIGNAL_STAGE" -czf "$SIGNAL_ARCHIVE" bin
sha_sidecar "$SIGNAL_ARCHIVE" "$SIGNAL_SHA"
make_release "$SIGNAL_REL" 1.0.25 "$TARGET"
mkdir -p "$BASE/signal-home"
_signal_status=0
env HOME="$BASE/signal-home" "$INSTALL" --no-path --prefix "$BASE/signal-prefix" \
	--archive "$SIGNAL_ARCHIVE" --sha256 "$SIGNAL_SHA" --release "$SIGNAL_REL" \
	>"$BASE/signal.out" 2>&1 || _signal_status=$?
if [ "$_signal_status" -eq 143 ] \
	&& [ ! -e "$BASE/signal-prefix/.solstone-route.lock" ] \
	&& [ -L "$BASE/signal-prefix/current" ] \
	&& grep -F 'setup_status=pending' "$BASE/signal-prefix/install-receipt" >/dev/null \
	&& ! grep -F 'installed solstone-journal' "$BASE/signal.out" >/dev/null; then
	pass "termination preserves a retryable pending route transaction"
else
	fail "termination status=$_signal_status output=$(cat "$BASE/signal.out")"
fi
if env HOME="$BASE/signal-home" "$INSTALL" --no-path --prefix "$BASE/signal-prefix" \
	--archive "$SIGNAL_ARCHIVE" --sha256 "$SIGNAL_SHA" --release "$SIGNAL_REL" \
	>"$BASE/signal-retry.out" 2>&1 \
	&& grep -F 'setup_status=complete' "$BASE/signal-prefix/install-receipt" >/dev/null; then
	pass "the same command completes setup after an interrupted route flip"
else
	fail "signal retry did not complete: $(cat "$BASE/signal-retry.out")"
fi

# If the signal arrives before `current` reaches the candidate, the prior
# route and its completed receipt remain authoritative. The handler must not
# publish a pending receipt for a candidate that never became current.
PRE_FLIP_STAGE=$BASE/pre-flip-stage
PRE_FLIP_ARCHIVE=$BASE/pre-flip.tar.gz
PRE_FLIP_SHA=$BASE/pre-flip.sha256
PRE_FLIP_REL=$BASE/pre-flip.release
make_tree_tar "$PRE_FLIP_ARCHIVE" "$PRE_FLIP_STAGE"
sha_sidecar "$PRE_FLIP_ARCHIVE" "$PRE_FLIP_SHA"
make_release "$PRE_FLIP_REL" 1.0.26 "$TARGET"
PRE_FLIP_BIN=$BASE/pre-flip-bin
mkdir -p "$PRE_FLIP_BIN"
printf '%s\n' '#!/bin/sh' 'kill -TERM "$PPID"' 'exit 143' >"$PRE_FLIP_BIN/ln"
chmod 755 "$PRE_FLIP_BIN/ln"
_pre_flip_current=$(readlink "$PREFIX/current")
_pre_flip_receipt=$(sha256sum "$PREFIX/install-receipt" | awk '{print $1}')
_pre_flip_status=0
env PATH="$PRE_FLIP_BIN:$PATH" HOME="$HOME" "$INSTALL" --no-path --prefix "$PREFIX" \
	--archive "$PRE_FLIP_ARCHIVE" --sha256 "$PRE_FLIP_SHA" --release "$PRE_FLIP_REL" \
	>"$BASE/pre-flip.out" 2>&1 || _pre_flip_status=$?
if [ "$_pre_flip_status" -eq 143 ] \
	&& [ "$(readlink "$PREFIX/current")" = "$_pre_flip_current" ] \
	&& [ "$(sha256sum "$PREFIX/install-receipt" | awk '{print $1}')" = "$_pre_flip_receipt" ] \
	&& [ ! -e "$PREFIX/.solstone-route.lock" ]; then
	pass "termination before the flip preserves the prior route receipt"
else
	fail "pre-flip termination changed the prior route: status=$_pre_flip_status"
fi

# If pending-to-complete promotion cannot allocate its temporary file, the
# already-staged pending receipt is published before refusal. The exact retry
# can then finish without losing the selected route.
PROMOTE_FAIL_BIN=$BASE/promote-fail-bin
PROMOTE_FAIL_PREFIX=$BASE/promote-fail-prefix
PROMOTE_FAIL_HOME=$BASE/promote-fail-home
mkdir -p "$PROMOTE_FAIL_BIN" "$PROMOTE_FAIL_HOME"
_real_mktemp=$(command -v mktemp)
{
	printf '%s\n' '#!/bin/sh' 'case $* in'
	printf '%s\n' "*install-receipt-complete*) exit 1 ;;" 'esac'
	printf 'exec "%s" "$@"\n' "$_real_mktemp"
} >"$PROMOTE_FAIL_BIN/mktemp"
chmod 755 "$PROMOTE_FAIL_BIN/mktemp"
_promote_status=0
env PATH="$PROMOTE_FAIL_BIN:$PATH" HOME="$PROMOTE_FAIL_HOME" "$INSTALL" --no-path \
	--prefix "$PROMOTE_FAIL_PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" \
	>"$BASE/promote-fail.out" 2>&1 || _promote_status=$?
if [ "$_promote_status" -ne 0 ] \
	&& [ -L "$PROMOTE_FAIL_PREFIX/current" ] \
	&& grep -F 'setup_status=pending' "$PROMOTE_FAIL_PREFIX/install-receipt" >/dev/null \
	&& [ ! -e "$PROMOTE_FAIL_PREFIX/.solstone-route.lock" ]; then
	pass "receipt promotion failure preserves the selected pending route"
else
	fail "receipt promotion failure lost route state: status=$_promote_status"
fi
if env HOME="$PROMOTE_FAIL_HOME" "$INSTALL" --no-path --prefix "$PROMOTE_FAIL_PREFIX" \
	--archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" \
	>"$BASE/promote-retry.out" 2>&1 \
	&& grep -F 'setup_status=complete' "$PROMOTE_FAIL_PREFIX/install-receipt" >/dev/null; then
	pass "the same command completes a receipt left pending during promotion"
else
	fail "receipt promotion retry did not complete: $(cat "$BASE/promote-retry.out")"
fi

# A signal immediately after the complete-state swap must publish that staged
# complete receipt instead of deleting it in cleanup.
PROMOTE_SIGNAL_BIN=$BASE/promote-signal-bin
PROMOTE_SIGNAL_PREFIX=$BASE/promote-signal-prefix
PROMOTE_SIGNAL_HOME=$BASE/promote-signal-home
mkdir -p "$PROMOTE_SIGNAL_BIN" "$PROMOTE_SIGNAL_HOME"
_real_mv=$(command -v mv)
{
	printf '%s\n' '#!/bin/sh'
	printf '"%s" "$@" || exit $?\n' "$_real_mv"
	printf '%s\n' 'case $* in' '*install-receipt-complete-*) kill -TERM "$PPID" ;;' 'esac' 'exit 0'
} >"$PROMOTE_SIGNAL_BIN/mv"
chmod 755 "$PROMOTE_SIGNAL_BIN/mv"
_promote_signal_status=0
env PATH="$PROMOTE_SIGNAL_BIN:$PATH" HOME="$PROMOTE_SIGNAL_HOME" "$INSTALL" --no-path \
	--prefix "$PROMOTE_SIGNAL_PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" \
	>"$BASE/promote-signal.out" 2>&1 || _promote_signal_status=$?
if [ "$_promote_signal_status" -eq 143 ] \
	&& [ -L "$PROMOTE_SIGNAL_PREFIX/current" ] \
	&& grep -F 'setup_status=complete' "$PROMOTE_SIGNAL_PREFIX/install-receipt" >/dev/null \
	&& [ ! -e "$PROMOTE_SIGNAL_PREFIX/.solstone-route.lock" ]; then
	pass "termination after receipt promotion publishes the complete route"
else
	fail "post-promotion termination lost route state: status=$_promote_signal_status"
fi

# A failed rollback after setup's pre-mutation refusal must not let `set -e`
# discard the candidate's only pending receipt.
ROLLBACK_HOME=$BASE/rollback-home
ROLLBACK_PREFIX=$BASE/rollback-prefix
mkdir -p "$ROLLBACK_HOME"
env HOME="$ROLLBACK_HOME" "$INSTALL" --no-path --prefix "$ROLLBACK_PREFIX" \
	--archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" >/dev/null
ROLLBACK_STAGE=$BASE/rollback-stage
ROLLBACK_ARCHIVE=$BASE/rollback.tar.gz
ROLLBACK_SHA=$BASE/rollback.sha256
ROLLBACK_REL=$BASE/rollback.release
mkdir -p "$ROLLBACK_STAGE/bin"
printf '%s\n' '#!/bin/sh' \
	'if [ ! -e "$HOME/.setup-refused-once" ]; then' \
	'  : >"$HOME/.setup-refused-once"' \
	'  exit 1' \
	'fi' \
	'exit 0' >"$ROLLBACK_STAGE/bin/journal"
chmod 755 "$ROLLBACK_STAGE/bin/journal"
tar -C "$ROLLBACK_STAGE" -czf "$ROLLBACK_ARCHIVE" bin
sha_sidecar "$ROLLBACK_ARCHIVE" "$ROLLBACK_SHA"
make_release "$ROLLBACK_REL" 1.0.27 "$TARGET"
ROLLBACK_BIN=$BASE/rollback-bin
ROLLBACK_LN_MARKER=$BASE/rollback-ln-first
mkdir -p "$ROLLBACK_BIN"
_real_ln=$(command -v ln)
{
	printf '%s\n' '#!/bin/sh'
	printf 'if [ ! -e "%s" ]; then\n' "$ROLLBACK_LN_MARKER"
	printf '  : >"%s"\n' "$ROLLBACK_LN_MARKER"
	printf '  exec "%s" "$@"\n' "$_real_ln"
	printf '%s\n' 'fi' 'exit 1'
} >"$ROLLBACK_BIN/ln"
chmod 755 "$ROLLBACK_BIN/ln"
_rollback_status=0
env PATH="$ROLLBACK_BIN:$PATH" HOME="$ROLLBACK_HOME" "$INSTALL" --no-path \
	--prefix "$ROLLBACK_PREFIX" --archive "$ROLLBACK_ARCHIVE" --sha256 "$ROLLBACK_SHA" --release "$ROLLBACK_REL" \
	>"$BASE/rollback.out" 2>&1 || _rollback_status=$?
if [ "$_rollback_status" -ne 0 ] \
	&& grep -F 'previous current target could not be restored' "$BASE/rollback.out" >/dev/null \
	&& grep -F 'setup_status=pending' "$ROLLBACK_PREFIX/install-receipt" >/dev/null \
	&& [ ! -e "$ROLLBACK_PREFIX/.solstone-route.lock" ]; then
	pass "setup rollback failure preserves the selected pending route"
else
	fail "setup rollback failure lost route state: status=$_rollback_status"
fi
if env HOME="$ROLLBACK_HOME" "$INSTALL" --no-path --prefix "$ROLLBACK_PREFIX" \
	--archive "$ROLLBACK_ARCHIVE" --sha256 "$ROLLBACK_SHA" --release "$ROLLBACK_REL" \
	>"$BASE/rollback-retry.out" 2>&1 \
	&& grep -F 'setup_status=complete' "$ROLLBACK_PREFIX/install-receipt" >/dev/null; then
	pass "the same command completes after setup rollback failure"
else
	fail "setup rollback retry did not complete: $(cat "$BASE/rollback-retry.out")"
fi

# The initial flip can itself replace `current` and still report failure. If
# restoring the prior target then also fails, the now-selected candidate gets
# the same pending receipt and retry contract.
FLIP_FAIL_HOME=$BASE/flip-fail-home
FLIP_FAIL_PREFIX=$BASE/flip-fail-prefix
mkdir -p "$FLIP_FAIL_HOME"
env HOME="$FLIP_FAIL_HOME" "$INSTALL" --no-path --prefix "$FLIP_FAIL_PREFIX" \
	--archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" >/dev/null
FLIP_FAIL_STAGE=$BASE/flip-fail-stage
FLIP_FAIL_ARCHIVE=$BASE/flip-fail.tar.gz
FLIP_FAIL_SHA=$BASE/flip-fail.sha256
FLIP_FAIL_REL=$BASE/flip-fail.release
make_tree_tar "$FLIP_FAIL_ARCHIVE" "$FLIP_FAIL_STAGE"
sha_sidecar "$FLIP_FAIL_ARCHIVE" "$FLIP_FAIL_SHA"
make_release "$FLIP_FAIL_REL" 1.0.28 "$TARGET"
FLIP_FAIL_BIN=$BASE/flip-fail-bin
FLIP_FAIL_LN_MARKER=$BASE/flip-fail-ln-first
mkdir -p "$FLIP_FAIL_BIN"
{
	printf '%s\n' '#!/bin/sh'
	printf 'if [ ! -e "%s" ]; then\n' "$FLIP_FAIL_LN_MARKER"
	printf '  : >"%s"\n' "$FLIP_FAIL_LN_MARKER"
	printf '  "%s" "$@" || exit $?\n' "$_real_ln"
	printf '%s\n' '  exit 1' 'fi' 'exit 1'
} >"$FLIP_FAIL_BIN/ln"
chmod 755 "$FLIP_FAIL_BIN/ln"
_flip_fail_status=0
env PATH="$FLIP_FAIL_BIN:$PATH" HOME="$FLIP_FAIL_HOME" "$INSTALL" --no-path \
	--prefix "$FLIP_FAIL_PREFIX" --archive "$FLIP_FAIL_ARCHIVE" --sha256 "$FLIP_FAIL_SHA" --release "$FLIP_FAIL_REL" \
	>"$BASE/flip-fail.out" 2>&1 || _flip_fail_status=$?
if [ "$_flip_fail_status" -ne 0 ] \
	&& grep -F 'previous current target could not be restored' "$BASE/flip-fail.out" >/dev/null \
	&& grep -F 'setup_status=pending' "$FLIP_FAIL_PREFIX/install-receipt" >/dev/null \
	&& [ ! -e "$FLIP_FAIL_PREFIX/.solstone-route.lock" ]; then
	pass "flip rollback failure preserves the selected pending route"
else
	fail "flip rollback failure lost route state: status=$_flip_fail_status"
fi
if env HOME="$FLIP_FAIL_HOME" "$INSTALL" --no-path --prefix "$FLIP_FAIL_PREFIX" \
	--archive "$FLIP_FAIL_ARCHIVE" --sha256 "$FLIP_FAIL_SHA" --release "$FLIP_FAIL_REL" \
	>"$BASE/flip-fail-retry.out" 2>&1 \
	&& grep -F 'setup_status=complete' "$FLIP_FAIL_PREFIX/install-receipt" >/dev/null; then
	pass "the same command completes after flip rollback failure"
else
	fail "flip rollback retry did not complete: $(cat "$BASE/flip-fail-retry.out")"
fi

# On a first install, a flip that fails before creating `current` rolls back to
# an empty route. The transaction-owned candidate and empty scaffold must go
# with it so the exact command remains a valid first-install retry.
FRESH_FLIP_FAIL_BIN=$BASE/fresh-flip-fail-bin
FRESH_FLIP_FAIL_PREFIX=$BASE/fresh-flip-fail-prefix
FRESH_FLIP_FAIL_HOME=$BASE/fresh-flip-fail-home
mkdir -p "$FRESH_FLIP_FAIL_BIN" "$FRESH_FLIP_FAIL_HOME"
printf '%s\n' '#!/bin/sh' 'exit 1' >"$FRESH_FLIP_FAIL_BIN/ln"
chmod 755 "$FRESH_FLIP_FAIL_BIN/ln"
_fresh_flip_status=0
env PATH="$FRESH_FLIP_FAIL_BIN:$PATH" HOME="$FRESH_FLIP_FAIL_HOME" "$INSTALL" --no-path \
	--prefix "$FRESH_FLIP_FAIL_PREFIX" --archive "$FLIP_FAIL_ARCHIVE" --sha256 "$FLIP_FAIL_SHA" --release "$FLIP_FAIL_REL" \
	>"$BASE/fresh-flip-fail.out" 2>&1 || _fresh_flip_status=$?
if [ "$_fresh_flip_status" -ne 0 ] && [ ! -e "$FRESH_FLIP_FAIL_PREFIX" ]; then
	pass "failed first-install flip removes its transaction-owned tree"
else
	fail "failed first-install flip left an orphaned route: status=$_fresh_flip_status"
fi
if env HOME="$FRESH_FLIP_FAIL_HOME" "$INSTALL" --no-path --prefix "$FRESH_FLIP_FAIL_PREFIX" \
	--archive "$FLIP_FAIL_ARCHIVE" --sha256 "$FLIP_FAIL_SHA" --release "$FLIP_FAIL_REL" \
	>"$BASE/fresh-flip-retry.out" 2>&1 \
	&& grep -F 'setup_status=complete' "$FRESH_FLIP_FAIL_PREFIX/install-receipt" >/dev/null; then
	pass "the same first-install command succeeds after a failed flip"
else
	fail "failed first-install flip was not retryable: $(cat "$BASE/fresh-flip-retry.out")"
fi

# Ownership is recorded before the candidate rename, so a signal delivered by
# that rename cannot strand an orphaned version directory without `current`.
DEST_MOVE_SIGNAL_BIN=$BASE/dest-move-signal-bin
DEST_MOVE_SIGNAL_PREFIX=$BASE/dest-move-signal-prefix
DEST_MOVE_SIGNAL_HOME=$BASE/dest-move-signal-home
mkdir -p "$DEST_MOVE_SIGNAL_BIN" "$DEST_MOVE_SIGNAL_HOME"
{
	printf '%s\n' '#!/bin/sh'
	printf '"%s" "$@" || exit $?\n' "$_real_mv"
	printf '%s\n' 'case $* in' '*/versions/.partial-*) kill -TERM "$PPID" ;;' 'esac' 'exit 0'
} >"$DEST_MOVE_SIGNAL_BIN/mv"
chmod 755 "$DEST_MOVE_SIGNAL_BIN/mv"
_dest_move_status=0
env PATH="$DEST_MOVE_SIGNAL_BIN:$PATH" HOME="$DEST_MOVE_SIGNAL_HOME" "$INSTALL" --no-path \
	--prefix "$DEST_MOVE_SIGNAL_PREFIX" --archive "$FLIP_FAIL_ARCHIVE" --sha256 "$FLIP_FAIL_SHA" --release "$FLIP_FAIL_REL" \
	>"$BASE/dest-move-signal.out" 2>&1 || _dest_move_status=$?
if [ "$_dest_move_status" -eq 143 ] && [ ! -e "$DEST_MOVE_SIGNAL_PREFIX" ]; then
	pass "termination after the candidate move removes the orphaned tree"
else
	fail "candidate-move termination left an orphaned route: status=$_dest_move_status"
fi
if env HOME="$DEST_MOVE_SIGNAL_HOME" "$INSTALL" --no-path --prefix "$DEST_MOVE_SIGNAL_PREFIX" \
	--archive "$FLIP_FAIL_ARCHIVE" --sha256 "$FLIP_FAIL_SHA" --release "$FLIP_FAIL_REL" \
	>"$BASE/dest-move-retry.out" 2>&1 \
	&& grep -F 'setup_status=complete' "$DEST_MOVE_SIGNAL_PREFIX/install-receipt" >/dev/null; then
	pass "the same first-install command succeeds after candidate-move termination"
else
	fail "candidate-move termination was not retryable: $(cat "$BASE/dest-move-retry.out")"
fi

# A foreign directory raced into the destination makes POSIX mv place the
# partial tree inside it. The transaction marker must prevent cleanup from
# claiming or recursively deleting that raced-in directory.
DEST_RACE_BIN=$BASE/dest-race-bin
DEST_RACE_PREFIX=$BASE/dest-race-prefix
DEST_RACE_HOME=$BASE/dest-race-home
mkdir -p "$DEST_RACE_BIN" "$DEST_RACE_HOME"
{
	printf '%s\n' '#!/bin/sh' 'case $* in' '*/versions/.partial-*)'
	printf '%s\n' '  mkdir -p "$2"' '  printf "%s\n" foreign >"$2/foreign-sentinel"'
	printf '  exec "%s" "$@"\n' "$_real_mv"
	printf '%s\n' '  ;;' 'esac'
	printf 'exec "%s" "$@"\n' "$_real_mv"
} >"$DEST_RACE_BIN/mv"
chmod 755 "$DEST_RACE_BIN/mv"
expect_refuse route-busy destination-race-is-named \
	env PATH="$DEST_RACE_BIN:$PATH" HOME="$DEST_RACE_HOME" "$INSTALL" --no-path \
	--prefix "$DEST_RACE_PREFIX" --archive "$FLIP_FAIL_ARCHIVE" --sha256 "$FLIP_FAIL_SHA" --release "$FLIP_FAIL_REL"
if find "$DEST_RACE_PREFIX/versions" -name foreign-sentinel -type f | grep . >/dev/null \
	&& ! find "$DEST_RACE_PREFIX/versions" -name '.partial-*' -type d | grep . >/dev/null; then
	pass "destination-race cleanup preserves foreign data and removes only its own nested tree"
else
	fail "destination-race cleanup crossed ownership or leaked its nested tree"
fi

DEST_RACE_SIGNAL_BIN=$BASE/dest-race-signal-bin
DEST_RACE_SIGNAL_PREFIX=$BASE/dest-race-signal-prefix
DEST_RACE_SIGNAL_HOME=$BASE/dest-race-signal-home
mkdir -p "$DEST_RACE_SIGNAL_BIN" "$DEST_RACE_SIGNAL_HOME"
{
	printf '%s\n' '#!/bin/sh' 'case $* in' '*/versions/.partial-*)'
	printf '%s\n' '  mkdir -p "$2"' '  printf "%s\n" foreign >"$2/foreign-sentinel"'
	printf '  "%s" "$@" || exit $?\n' "$_real_mv"
	printf '%s\n' '  kill -TERM "$PPID"' '  exit 0' '  ;;' 'esac'
	printf 'exec "%s" "$@"\n' "$_real_mv"
} >"$DEST_RACE_SIGNAL_BIN/mv"
chmod 755 "$DEST_RACE_SIGNAL_BIN/mv"
_dest_race_signal_status=0
env PATH="$DEST_RACE_SIGNAL_BIN:$PATH" HOME="$DEST_RACE_SIGNAL_HOME" "$INSTALL" --no-path \
	--prefix "$DEST_RACE_SIGNAL_PREFIX" --archive "$FLIP_FAIL_ARCHIVE" --sha256 "$FLIP_FAIL_SHA" --release "$FLIP_FAIL_REL" \
	>"$BASE/dest-race-signal.out" 2>&1 || _dest_race_signal_status=$?
if [ "$_dest_race_signal_status" -eq 143 ] \
	&& find "$DEST_RACE_SIGNAL_PREFIX/versions" -name foreign-sentinel -type f | grep . >/dev/null \
	&& ! find "$DEST_RACE_SIGNAL_PREFIX/versions" -name '.partial-*' -type d | grep . >/dev/null; then
	pass "termination during a destination race removes only the owned nested tree"
else
	fail "destination-race termination crossed ownership or leaked its nested tree"
fi

# Setup is complete before PATH publication. If the latter fails, the selected
# tree retains an honest completed receipt instead of a stale or absent one.
PROFILE_FAIL_HOME=$BASE/profile-fail-home
PROFILE_FAIL_PREFIX=$BASE/profile-fail-prefix
mkdir -p "$PROFILE_FAIL_HOME"
printf '%s\n' 'not a directory' >"$PROFILE_FAIL_HOME/not-a-directory"
_profile_status=0
env HOME="$PROFILE_FAIL_HOME" SOLSTONE_PROFILE="$PROFILE_FAIL_HOME/not-a-directory/profile" \
	"$INSTALL" --prefix "$PROFILE_FAIL_PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" \
	>"$BASE/profile-fail.out" 2>&1 || _profile_status=$?
if [ "$_profile_status" -ne 0 ] \
	&& [ -L "$PROFILE_FAIL_PREFIX/current" ] \
	&& grep -F 'setup_status=complete' "$PROFILE_FAIL_PREFIX/install-receipt" >/dev/null \
	&& [ ! -e "$PROFILE_FAIL_PREFIX/.solstone-route.lock" ]; then
	pass "profile failure retains the completed install receipt"
else
	fail "profile failure lost transaction state: status=$_profile_status"
fi

# A tree-shaped prefix with malformed provenance is not positively identified.
# Its refusal must name both the class and the owner's concrete next step.
MALFORMED_PREFIX=$BASE/malformed-owned-prefix
MALFORMED_DIGEST=0000000000000000000000000000000000000000000000000000000000000000
mkdir -p "$MALFORMED_PREFIX/versions/1.0.22-000000000000/bin"
printf '%s\n' "$MALFORMED_DIGEST" >"$MALFORMED_PREFIX/versions/1.0.22-000000000000/.archive-sha256"
printf '%s\n' 'not-a-release-record' >"$MALFORMED_PREFIX/versions/1.0.22-000000000000/.release"
printf '%s\n' '#!/bin/sh' 'exit 0' >"$MALFORMED_PREFIX/versions/1.0.22-000000000000/bin/journal"
chmod 755 "$MALFORMED_PREFIX/versions/1.0.22-000000000000/bin/journal"
ln -s versions/1.0.22-000000000000 "$MALFORMED_PREFIX/current"
expect_refuse release-invalid malformed-owned-release-is-named \
	env HOME="$BASE/malformed-home" "$INSTALL" --upgrade --prefix "$MALFORMED_PREFIX" --archive /nope --sha256 /nope --release /nope
expect_refuse 'leave the existing tree untouched and run journal setup' malformed-owned-release-has-next-step \
	env HOME="$BASE/malformed-home" "$INSTALL" --upgrade --prefix "$MALFORMED_PREFIX" --archive /nope --sha256 /nope --release /nope

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
	"$INSTALL" --prefix "$PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" \
	>"$BASE/idempotent.out"; then
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
if grep -F 'installed solstone-journal 1.0.22' "$BASE/idempotent.out" >/dev/null \
	&& grep -F 'lane=release' "$BASE/idempotent.out" >/dev/null; then
	pass "identical-digest no-op reports version and lane"
else
	fail "identical-digest no-op omitted version or lane"
fi

# Unknown receipt keys are tolerated, while an unknown schema is named and
# leaves the installed tree untouched.
printf '%s\n' 'future_dispatch_hint=ignored-by-v1' >>"$PREFIX/install-receipt"
if env HOME="$HOME" "$INSTALL" --prefix "$PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" >/dev/null; then
	pass "receipt reader tolerates unknown keys"
else
	fail "receipt reader rejected an unknown key"
fi
cp "$PREFIX/install-receipt" "$BASE/receipt.good"
sed 's/^schema_version=1$/schema_version=99/' "$BASE/receipt.good" >"$PREFIX/install-receipt"
_receipt_current=$(readlink "$PREFIX/current")
expect_refuse receipt-schema-unsupported receipt-schema-newer \
	env HOME="$HOME" "$INSTALL" --prefix "$PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL"
if [ "$(readlink "$PREFIX/current")" = "$_receipt_current" ]; then
	pass "unsupported receipt schema leaves current unchanged"
else
	fail "unsupported receipt schema changed current"
fi
cp "$BASE/receipt.good" "$PREFIX/install-receipt"

# Lane persists by default, and an explicit lane wins.
LANE_HOME=$BASE/lane-home
LANE_PREFIX=$BASE/lane-prefix
mkdir -p "$LANE_HOME"
env HOME="$LANE_HOME" "$INSTALL" --lane staging --prefix "$LANE_PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" >/dev/null
env HOME="$LANE_HOME" "$INSTALL" --prefix "$LANE_PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" >/dev/null
if grep -F 'lane=staging' "$LANE_PREFIX/install-receipt" >/dev/null; then
	pass "plain upgrade preserves staging lane"
else
	fail "plain upgrade lost staging lane"
fi
env HOME="$LANE_HOME" "$INSTALL" --lane dev --prefix "$LANE_PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" >/dev/null
if grep -F 'lane=dev' "$LANE_PREFIX/install-receipt" >/dev/null; then
	pass "explicit lane overrides receipt"
else
	fail "explicit lane did not override receipt"
fi

# A positively identified receipt-less tree is adopted. Its unknowable lane is
# retained as unknown rather than guessed to be release.
ADOPT_HOME=$BASE/adopt-home
ADOPT_PREFIX=$BASE/adopt-prefix
mkdir -p "$ADOPT_HOME"
env HOME="$ADOPT_HOME" "$INSTALL" --prefix "$ADOPT_PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" >/dev/null
rm -f "$ADOPT_PREFIX/install-receipt"

# Today's receipt-less installs also carry the five-field pre-epoch release
# record. Positive identification accepts that legacy shape for adoption, but
# does not invent an epoch or lane for it.
_adopt_current=$ADOPT_PREFIX/$(readlink "$ADOPT_PREFIX/current")
sed '/^upgrade_epoch=/d; /^retention_window=/d' "$_adopt_current/.release" >"$BASE/adopt-legacy.release"
mv "$BASE/adopt-legacy.release" "$_adopt_current/.release"
ADOPT_UPGRADE_REL=$BASE/adopt-upgrade.release
make_release "$ADOPT_UPGRADE_REL" 1.0.23 "$TARGET"
if env HOME="$ADOPT_HOME" "$INSTALL" --upgrade --prefix "$ADOPT_PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$ADOPT_UPGRADE_REL" >/dev/null \
	&& grep -F 'lane=unknown' "$ADOPT_PREFIX/install-receipt" >/dev/null \
	&& grep -F 'journal_version=1.0.23' "$ADOPT_PREFIX/install-receipt" >/dev/null; then
	pass "receipt-less owned tree is adopted with unknown lane"
else
	fail "receipt-less owned tree adoption"
fi

UNKNOWN_PREFIX=$BASE/unknown-prefix
mkdir -p "$UNKNOWN_PREFIX/versions"
printf 'owner data\n' >"$UNKNOWN_PREFIX/sentinel"
expect_refuse route-unknown unknown-tree-not-adopted \
	env HOME="$BASE/unknown-home" "$INSTALL" --upgrade --prefix "$UNKNOWN_PREFIX" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL"
if [ "$(cat "$UNKNOWN_PREFIX/sentinel")" = 'owner data' ] && [ ! -e "$UNKNOWN_PREFIX/current" ]; then
	pass "unknown tree refusal mutates nothing"
else
	fail "unknown tree refusal mutated the prefix"
fi

expect_refuse upgrade-not-installed upgrade-requires-existing-route \
	env HOME="$BASE/fresh-upgrade-home" "$INSTALL" --upgrade --prefix "$BASE/fresh-upgrade-prefix" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL"

V1_HOME=$BASE/v1-home
mkdir -p "$V1_HOME/.local/bin"
printf '%s\n' '#!/usr/bin/python3' 'from solstone.think.sol_cli import journal_main' >"$V1_HOME/.local/bin/journal"
chmod 755 "$V1_HOME/.local/bin/journal"
expect_refuse v1-handoff v1-remains-setup-owned \
	env HOME="$V1_HOME" "$INSTALL" --upgrade --prefix "$BASE/v1-prefix" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL"

PACKAGE_BIN=$BASE/package-bin
mkdir -p "$PACKAGE_BIN"
printf '%s\n' '#!/bin/sh' "printf '%s' 'install ok installed'" >"$PACKAGE_BIN/dpkg-query"
chmod 755 "$PACKAGE_BIN/dpkg-query"
expect_refuse 'sudo apt upgrade solstone-journal' package-route-names-apt \
	env PATH="$PACKAGE_BIN:$PATH" HOME="$BASE/package-home" "$INSTALL" --upgrade --prefix "$BASE/package-prefix" --archive "$ARCHIVE" --sha256 "$SHA" --release "$REL"
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
NO_PATH_ARGS=$BASE/nopath-setup-args
if env HOME="$NO_PATH_HOME" SOLSTONE_SETUP_ARGS_LOG="$NO_PATH_ARGS" \
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
	if grep -F -- '--skip-path' "$NO_PATH_ARGS" >/dev/null \
		&& grep -F -- '--installer-transaction' "$NO_PATH_ARGS" >/dev/null; then
		pass "no-path is carried through setup"
	else
		fail "no-path setup arguments: $(cat "$NO_PATH_ARGS" 2>/dev/null)"
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
if [ -x "$PREFIX/current/bin/journal" ] && grep -F '# fixture-build: ok' "$PREFIX/current/bin/journal" >/dev/null; then
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
printf '%s\n' '#!/bin/sh' '# fixture-build: other' 'exit 0' >"$STAGE2/bin/journal"
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
if grep -F '# fixture-build: other' "$PREFIX/current/bin/journal" >/dev/null 2>&1; then
	pass "same-version respin flips current to the new build"
else
	fail "same-version respin flips current to the new build"
fi
if [ -x "$PREFIX/$_prev_current/bin/journal" ] && grep -F '# fixture-build: ok' "$PREFIX/$_prev_current/bin/journal" >/dev/null; then
	pass "same-version respin keeps the prior build's version directory"
else
	fail "same-version respin keeps the prior build's version directory"
fi

# The installer flips `current` before invoking the newly installed setup so
# setup observes the stable versioned-prefix identity. Exit 1 or 2 is setup's
# explicit pre-mutation refusal contract in installer-transaction mode, so
# only those statuses can safely restore the
# previous current target. Any other failure may follow a partial wrapper or
# service repoint and must leave current on the candidate for a safe retry.
FAIL_STAGE=$BASE/setup-fail-stage
FAIL_ARCHIVE=$BASE/setup-fail.tar.gz
FAIL_SHA=$BASE/setup-fail.sha256
FAIL_REL=$BASE/setup-fail.release
mkdir -p "$FAIL_STAGE/bin"
printf '%s\n' '#!/bin/sh' 'exit 2' >"$FAIL_STAGE/bin/journal"
chmod 755 "$FAIL_STAGE/bin/journal"
tar -C "$FAIL_STAGE" -czf "$FAIL_ARCHIVE" bin
sha_sidecar "$FAIL_ARCHIVE" "$FAIL_SHA"
make_release "$FAIL_REL" 1.0.23 "$TARGET"
_before_failed_setup=$(readlink "$PREFIX/current")
cp "$PREFIX/install-receipt" "$BASE/receipt-before-setup-failure"
expect_refuse setup-failed setup-failure-is-named \
	env HOME="$HOME" "$INSTALL" --prefix "$PREFIX" --archive "$FAIL_ARCHIVE" --sha256 "$FAIL_SHA" --release "$FAIL_REL"
if [ "$(readlink "$PREFIX/current")" = "$_before_failed_setup" ]; then
	pass "setup failure restores previous current"
else
	fail "setup failure left current on the failed candidate"
fi
if cmp -s "$PREFIX/install-receipt" "$BASE/receipt-before-setup-failure"; then
	pass "setup failure preserves previous receipt"
else
	fail "setup failure rewrote the receipt"
fi
if find "$PREFIX" -maxdepth 1 -name '.install-receipt-*' | grep . >/dev/null; then
	fail "setup refusal left a staged receipt"
else
	pass "setup refusal removes its staged receipt"
fi

PARTIAL_FAIL_STAGE=$BASE/setup-partial-fail-stage
PARTIAL_FAIL_ARCHIVE=$BASE/setup-partial-fail.tar.gz
PARTIAL_FAIL_SHA=$BASE/setup-partial-fail.sha256
PARTIAL_FAIL_REL=$BASE/setup-partial-fail.release
mkdir -p "$PARTIAL_FAIL_STAGE/bin"
printf '%s\n' '#!/bin/sh' '[ "${SOLSTONE_SETUP_RETRY:-0}" -eq 1 ] && exit 0' 'exit 3' >"$PARTIAL_FAIL_STAGE/bin/journal"
chmod 755 "$PARTIAL_FAIL_STAGE/bin/journal"
tar -C "$PARTIAL_FAIL_STAGE" -czf "$PARTIAL_FAIL_ARCHIVE" bin
sha_sidecar "$PARTIAL_FAIL_ARCHIVE" "$PARTIAL_FAIL_SHA"
make_release "$PARTIAL_FAIL_REL" 1.0.24 "$TARGET"
_before_partial_failure=$(readlink "$PREFIX/current")
expect_refuse setup-failed setup-partial-failure-is-named \
	env HOME="$HOME" "$INSTALL" --prefix "$PREFIX" --archive "$PARTIAL_FAIL_ARCHIVE" --sha256 "$PARTIAL_FAIL_SHA" --release "$PARTIAL_FAIL_REL"
if [ "$(readlink "$PREFIX/current")" != "$_before_partial_failure" ] \
	&& grep -F 'exit 3' "$PREFIX/current/bin/journal" >/dev/null; then
	pass "post-mutation setup failure leaves current on the retryable candidate"
else
	fail "post-mutation setup failure restored an unsafe mixed installation"
fi
if grep -F 'journal_version=1.0.24' "$PREFIX/install-receipt" >/dev/null \
	&& grep -F 'lane=release' "$PREFIX/install-receipt" >/dev/null \
	&& grep -F 'setup_status=pending' "$PREFIX/install-receipt" >/dev/null; then
	pass "post-mutation setup failure publishes a retryable pending receipt"
else
	fail "post-mutation setup failure receipt: $(cat "$PREFIX/install-receipt" 2>/dev/null)"
fi
if find "$PREFIX" -maxdepth 1 -name '.install-receipt-*' | grep . >/dev/null; then
	fail "post-mutation setup failure left a staged receipt"
else
	pass "post-mutation setup failure removes its staged receipt"
fi
if env HOME="$HOME" SOLSTONE_SETUP_RETRY=1 "$INSTALL" --prefix "$PREFIX" \
	--archive "$PARTIAL_FAIL_ARCHIVE" --sha256 "$PARTIAL_FAIL_SHA" --release "$PARTIAL_FAIL_REL" >/dev/null \
	&& grep -F 'setup_status=complete' "$PREFIX/install-receipt" >/dev/null \
	&& ! grep -F 'setup_status=pending' "$PREFIX/install-receipt" >/dev/null; then
	pass "rerunning the same installer command completes pending setup"
else
	fail "same-command setup retry did not complete"
fi

# Downgrades are only permitted to a retained directory in the signed epoch.
# Four distinct fixtures make the three-directory boundary observable.
DOWNGRADE_HOME=$BASE/downgrade-home
DOWNGRADE_PREFIX=$BASE/downgrade-prefix
mkdir -p "$DOWNGRADE_HOME"

# Component comparison must remain exact beyond awk/IEEE-754 integer precision.
HUGE_HOME=$BASE/huge-version-home
HUGE_PREFIX=$BASE/huge-version-prefix
mkdir -p "$HUGE_HOME"
for _huge_version in 2.0.9007199254740993 2.0.9007199254740992; do
	_huge_stage=$BASE/huge-${_huge_version}-stage
	_huge_archive=$BASE/huge-${_huge_version}.tar.gz
	_huge_sha=$BASE/huge-${_huge_version}.sha256
	_huge_release=$BASE/huge-${_huge_version}.release
	make_tree_tar "$_huge_archive" "$_huge_stage"
	sha_sidecar "$_huge_archive" "$_huge_sha"
	make_release "$_huge_release" "$_huge_version" "$TARGET"
done
env HOME="$HUGE_HOME" "$INSTALL" --prefix "$HUGE_PREFIX" \
	--archive "$BASE/huge-2.0.9007199254740993.tar.gz" \
	--sha256 "$BASE/huge-2.0.9007199254740993.sha256" \
	--release "$BASE/huge-2.0.9007199254740993.release" >/dev/null
_huge_current=$(readlink "$HUGE_PREFIX/current")
expect_refuse downgrade-window huge-semver-downgrade-is-exact \
	env HOME="$HUGE_HOME" "$INSTALL" --prefix "$HUGE_PREFIX" \
	--archive "$BASE/huge-2.0.9007199254740992.tar.gz" \
	--sha256 "$BASE/huge-2.0.9007199254740992.sha256" \
	--release "$BASE/huge-2.0.9007199254740992.release"
if [ "$(readlink "$HUGE_PREFIX/current")" = "$_huge_current" ]; then
	pass "huge semver downgrade leaves current unchanged"
else
	fail "huge semver downgrade changed current"
fi

_patch=0
while [ "$_patch" -le 3 ]; do
	_version=2.0.${_patch}
	_stage=$BASE/downgrade-stage-${_patch}
	_archive=$BASE/downgrade-${_patch}.tar.gz
	_sha=$BASE/downgrade-${_patch}.sha256
	_release=$BASE/downgrade-${_patch}.release
	mkdir -p "$_stage/bin"
	printf '%s\n' '#!/bin/sh' "# fixture-version: $_version" 'exit 0' >"$_stage/bin/journal"
	chmod 755 "$_stage/bin/journal"
	tar -C "$_stage" -czf "$_archive" bin
	sha_sidecar "$_archive" "$_sha"
	make_release "$_release" "$_version" "$TARGET"
	env HOME="$DOWNGRADE_HOME" "$INSTALL" --prefix "$DOWNGRADE_PREFIX" \
		--archive "$_archive" --sha256 "$_sha" --release "$_release" >/dev/null
	# Deterministic order for the explicit retention-window checks.
	touch -t "20260903010${_patch}.00" "$DOWNGRADE_PREFIX/$(readlink "$DOWNGRADE_PREFIX/current")"
	_patch=$((_patch + 1))
done

if env HOME="$DOWNGRADE_HOME" "$INSTALL" --prefix "$DOWNGRADE_PREFIX" \
	--archive "$BASE/downgrade-2.tar.gz" --sha256 "$BASE/downgrade-2.sha256" \
	--release "$BASE/downgrade-2.release" >/dev/null \
	&& grep -F '# fixture-version: 2.0.2' "$DOWNGRADE_PREFIX/current/bin/journal" >/dev/null; then
	pass "downgrade inside epoch and three-directory window"
else
	fail "downgrade inside epoch and window"
fi

_before_outside_window=$(readlink "$DOWNGRADE_PREFIX/current")
expect_refuse downgrade-window downgrade-outside-window \
	env HOME="$DOWNGRADE_HOME" "$INSTALL" --prefix "$DOWNGRADE_PREFIX" \
	--archive "$BASE/downgrade-0.tar.gz" --sha256 "$BASE/downgrade-0.sha256" \
	--release "$BASE/downgrade-0.release"
if [ "$(readlink "$DOWNGRADE_PREFIX/current")" = "$_before_outside_window" ]; then
	pass "outside-window downgrade mutates no current target"
else
	fail "outside-window downgrade changed current"
fi

EPOCH_STAGE=$BASE/downgrade-epoch-stage
EPOCH_ARCHIVE=$BASE/downgrade-epoch.tar.gz
EPOCH_SHA=$BASE/downgrade-epoch.sha256
EPOCH_REL=$BASE/downgrade-epoch.release
mkdir -p "$EPOCH_STAGE/bin"
printf '%s\n' '#!/bin/sh' '# fixture-version: 1.9.9-foreign-epoch' 'exit 0' >"$EPOCH_STAGE/bin/journal"
chmod 755 "$EPOCH_STAGE/bin/journal"
tar -C "$EPOCH_STAGE" -czf "$EPOCH_ARCHIVE" bin
sha_sidecar "$EPOCH_ARCHIVE" "$EPOCH_SHA"
make_release "$EPOCH_REL" 1.9.9 "$TARGET"
sed 's/^upgrade_epoch=journal-v2$/upgrade_epoch=journal-v3/' "$EPOCH_REL" >"$BASE/downgrade-epoch-other.release"
expect_refuse downgrade-epoch downgrade-crosses-epoch \
	env HOME="$DOWNGRADE_HOME" "$INSTALL" --prefix "$DOWNGRADE_PREFIX" \
	--archive "$EPOCH_ARCHIVE" --sha256 "$EPOCH_SHA" \
	--release "$BASE/downgrade-epoch-other.release"

if env HOME="$DOWNGRADE_HOME" "$INSTALL_SOURCE" --prefix "$DOWNGRADE_PREFIX" --prune \
	>"$BASE/prune.out" 2>&1; then
	_version_dir_count=$(find "$DOWNGRADE_PREFIX/versions" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
	if [ "$_version_dir_count" -eq 3 ] && [ -d "$DOWNGRADE_PREFIX/$_before_outside_window" ]; then
		pass "explicit prune keeps three directories including current"
	else
		fail "explicit prune retained $_version_dir_count directories or removed current"
	fi
else
	fail "explicit prune: $(cat "$BASE/prune.out")"
fi

	expect_refuse prune-unsafe prune-rejects-lane \
	env HOME="$DOWNGRADE_HOME" "$INSTALL_SOURCE" --prefix "$DOWNGRADE_PREFIX" --prune --lane release
expect_refuse prune-unsafe prune-rejects-origin \
	env HOME="$DOWNGRADE_HOME" "$INSTALL_SOURCE" --prefix "$DOWNGRADE_PREFIX" --prune --origin https://updates.solstone.app
expect_refuse prune-unsafe prune-rejects-signature-flag \
	env HOME="$DOWNGRADE_HOME" "$INSTALL_SOURCE" --prefix "$DOWNGRADE_PREFIX" --prune --skip-signature
expect_refuse prune-unsafe prune-rejects-path-flag \
	env HOME="$DOWNGRADE_HOME" "$INSTALL_SOURCE" --prefix "$DOWNGRADE_PREFIX" --prune --no-path

rm -f "$DOWNGRADE_PREFIX/install-receipt"
if env HOME="$DOWNGRADE_HOME" "$INSTALL_SOURCE" --prefix "$DOWNGRADE_PREFIX" --prune \
	>"$BASE/prune-receiptless.out" 2>&1; then
	pass "receipt-less verified tree can be pruned without a lane"
else
	fail "receipt-less prune: $(cat "$BASE/prune-receiptless.out")"
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

# A response truncated anywhere before the last `main "$@"` line may parse or
# fail to parse, but it must not execute an install action. Exercise cuts before
# the function, inside it, and immediately before its EOF call.
_main_line=$(awk '/^main\(\) \{/ { print NR; exit }' "$INSTALL_SOURCE")
_call_line=$(awk '/^main "\$@"$/ { print NR; exit }' "$INSTALL_SOURCE")
if [ -z "$_main_line" ] || [ -z "$_call_line" ] || [ "$_call_line" -le "$_main_line" ]; then
	fail "installer main-at-EOF structure"
else
	_cut_index=0
	for _cut_line in "$((_main_line - 1))" "$((_main_line + 8))" "$((_call_line - 1))"; do
		_cut_index=$((_cut_index + 1))
		_cut_script=$BASE/install-truncated-${_cut_index}.sh
		_cut_home=$BASE/truncated-home-${_cut_index}
		_cut_prefix=$BASE/truncated-prefix-${_cut_index}
		head -n "$_cut_line" "$INSTALL_SOURCE" >"$_cut_script"
		chmod 755 "$_cut_script"
		mkdir -p "$_cut_home"
		printf 'profile sentinel\n' >"$_cut_home/.profile"
		env HOME="$_cut_home" "$_cut_script" --prefix "$_cut_prefix" \
			--archive "$ARCHIVE" --sha256 "$SHA" --release "$REL" >/dev/null 2>&1 || true
		if [ ! -e "$_cut_prefix" ] && [ "$(cat "$_cut_home/.profile")" = "profile sentinel" ]; then
			pass "truncated installer cut ${_cut_index} executes no action"
		else
			fail "truncated installer cut ${_cut_index} mutated install or profile"
		fi
	done
fi

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
