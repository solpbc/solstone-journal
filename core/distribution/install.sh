#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# POSIX bootstrap: detect → fetch → verify → extract → flip current → profile.
# Named refusals are the contract surface; keep them in sync with
# solstone-core-distribution ArchiveEscape plus the install-only names below.
#
# ARCHIVE_REFUSALS:
#   archive-absolute-path
#   archive-parent-traversal
#   archive-symlink-escape
#   archive-hardlink-escape
#   archive-symlink-then-child
# INSTALL_REFUSALS:
#   unsupported-platform
#   origin-refused
#   fetcher-missing
#   tmpdir-unusable
#   digest-mismatch
#   release-invalid
#   version-mismatch
#   lane-invalid
#   latest-invalid

set -eu

PRODUCT=solstone-journal
ORIGIN_HOST=updates.solstone.app
MAX_HOPS=5
PROFILE_BEGIN="# BEGIN solstone-journal PATH"
PROFILE_END="# END solstone-journal PATH"

refuse() {
	_name=$1
	shift
	if [ "$#" -gt 0 ]; then
		printf '%s\n' "${_name}: $*" >&2
	else
		printf '%s\n' "${_name}" >&2
	fi
	exit 2
}

usage() {
	printf '%s\n' "usage: install.sh [--prefix DIR] [--version VER] [--lane LANE] [--origin URL] [--archive FILE] [--sha256 FILE] [--release FILE] [--no-path]"
}

PREFIX=
VERSION=
LANE=release
ORIGIN=
ARCHIVE=
SHA256_FILE=
RELEASE_FILE=
NO_PATH=0
while [ "$#" -gt 0 ]; do
	case $1 in
	--prefix)
		PREFIX=$2
		shift 2
		;;
	--version)
		VERSION=$2
		shift 2
		;;
	--lane)
		LANE=$2
		shift 2
		;;
	--origin)
		ORIGIN=$2
		shift 2
		;;
	--archive)
		ARCHIVE=$2
		shift 2
		;;
	--sha256)
		SHA256_FILE=$2
		shift 2
		;;
	--release)
		RELEASE_FILE=$2
		shift 2
		;;
	--no-path)
		NO_PATH=1
		shift
		;;
	--help | -h)
		usage
		exit 0
		;;
	*)
		refuse release-invalid "unknown argument $1"
		;;
	esac
done

case $LANE in
release | staging | dev) ;;
*) refuse lane-invalid "$LANE" ;;
esac

HOME=${HOME:-}
if [ -z "$HOME" ]; then
	refuse unsupported-platform "HOME is unset"
fi
if [ -z "$PREFIX" ]; then
	PREFIX=$HOME/.local/solstone-journal
fi
if [ -z "$ORIGIN" ]; then
	ORIGIN=https://${ORIGIN_HOST}
fi

detect_target() {
	_os=${SOLSTONE_UNAME_S:-$(uname -s)}
	_arch=${SOLSTONE_UNAME_M:-$(uname -m)}
	_os_lc=$(printf '%s' "$_os" | tr '[:upper:]' '[:lower:]')
	_arch_lc=$(printf '%s' "$_arch" | tr '[:upper:]' '[:lower:]')
	case ${_os_lc} in
	linux)
		case ${_arch_lc} in
		x86_64 | amd64) TARGET=linux-x86_64 ;;
		aarch64 | arm64) TARGET=linux-aarch64 ;;
		*) refuse unsupported-platform "arch=${_arch}" ;;
		esac
		;;
	darwin)
		# Intel Macs are deliberately not a target: the journal runtime is
		# Apple Silicon only. Refusing by name beats installing a tree whose
		# binaries cannot execute.
		case ${_arch_lc} in
		arm64 | aarch64) TARGET=macos-arm64 ;;
		*) refuse unsupported-platform "arch=${_arch}" ;;
		esac
		;;
	*) refuse unsupported-platform "os=${_os}" ;;
	esac
}

origin_host() {
	_url=$1
	_rest=${_url#*://}
	_host=${_rest%%/*}
	_host=${_host%%@*}
	printf '%s' "${_host%%:*}"
}

origin_scheme() {
	_url=$1
	printf '%s' "${_url%%://*}"
}

check_origin_url() {
	_url=$1
	case $_url in
	*://*@*) refuse origin-refused "userinfo" ;;
	esac
	_scheme=$(origin_scheme "$_url")
	_host=$(origin_host "$_url")
	case ${_scheme}://${_host} in
	https://${ORIGIN_HOST}) return 0 ;;
	http://127.0.0.1 | https://127.0.0.1) return 0 ;;
	*) refuse origin-refused "${_scheme}://${_host}" ;;
	esac
}

hex_len() {
	printf '%s' "$1" | wc -c | tr -d ' '
}

is_hex() {
	_val=$1
	_len=$2
	[ "$(hex_len "$_val")" -eq "$_len" ] || return 1
	case $_val in
	*[!0-9a-f]*) return 1 ;;
	esac
	return 0
}

digest_file() {
	_path=$1
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum "$_path" | awk '{print $1}'
		return 0
	fi
	if command -v openssl >/dev/null 2>&1; then
		openssl dgst -sha256 "$_path" | awk '{print $NF}'
		return 0
	fi
	refuse fetcher-missing "sha256"
}

parse_sha256_file() {
	_path=$1
	_want=$2
	[ -n "$_want" ] || refuse digest-mismatch "sha256 sidecar"
	_line=$(awk -v w="$_want" '
		NF < 2 { next }
		{
			name = $2
			base = name
			sub(/^.*\//, "", base)
			if (name == w || base == w) {
				print
				exit
			}
		}
	' "$_path")
	[ -n "$_line" ] || refuse digest-mismatch "sha256 sidecar"
	_digest=${_line%% *}
	is_hex "$_digest" 64 || refuse digest-mismatch "sha256 sidecar"
	printf '%s' "$_digest"
}

fetch_url() {
	_url=$1
	_dest=$2
	_on_fail=${3:-origin-refused}
	check_origin_url "$_url"
	_hops=0
	_current=$_url
	while [ "$_hops" -le "$MAX_HOPS" ]; do
		check_origin_url "$_current"
		if command -v curl >/dev/null 2>&1; then
			_hdrs=$(mktemp "$WORK/solstone-install-headers-XXXXXX")
			_code=$(curl -sS --http1.1 -D "$_hdrs" -o "$_dest" -w '%{http_code}' "$_current" || true)
			_location=$(awk '/^[Ll][Oo][Cc][Aa][Tt][Ii][Oo][Nn]:/{sub(/\r$/,""); sub(/^[^:]*:[[:space:]]*/,""); print; exit}' "$_hdrs")
			rm -f "$_hdrs"
		elif command -v wget >/dev/null 2>&1; then
			_hdrs=$(mktemp "$WORK/solstone-install-headers-XXXXXX")
			if wget -qS -O "$_dest" "$_current" 2>"$_hdrs"; then
				_code=200
			else
				_code=$(awk '/^  HTTP\//{print $2; exit}' "$_hdrs")
			fi
			_location=$(awk '/^[[:space:]]*[Ll][Oo][Cc][Aa][Tt][Ii][Oo][Nn]:/{sub(/\r$/,""); sub(/^[[:space:]]*[^:]*:[[:space:]]*/,""); print; exit}' "$_hdrs")
			rm -f "$_hdrs"
		else
			refuse fetcher-missing
		fi
		case $_code in
		200)
			return 0
			;;
		301 | 302 | 303 | 307 | 308)
			[ -n "$_location" ] || refuse "$_on_fail" "redirect without Location"
			_hops=$((_hops + 1))
			[ "$_hops" -le "$MAX_HOPS" ] || refuse "$_on_fail" "too many redirects"
			case $_location in
			http://* | https://*) _current=$_location ;;
			/*)
				_scheme=$(origin_scheme "$_current")
				_host=$(origin_host "$_current")
				_current=${_scheme}://${_host}${_location}
				;;
			*) refuse "$_on_fail" "relative redirect" ;;
			esac
			;;
		*)
			refuse "$_on_fail" "http ${_code}"
			;;
		esac
	done
	refuse "$_on_fail" "too many redirects"
}

validate_release() {
	_text=$1
	_want_version=$2
	_want_target=$3
	_lines=$(printf '%s\n' "$_text" | awk 'NF{c++} END{print c+0}')
	[ "$_lines" -eq 5 ] || refuse release-invalid "expected 5 lines"
	_product=
	_version=
	_target=
	_commit=
	_lock=
	_oldifs=$IFS
	IFS=
	while read -r _line; do
		[ -n "$_line" ] || continue
		case $_line in
		*=*) ;;
		*)
			IFS=$_oldifs
			refuse release-invalid "not key=value"
			;;
		esac
		_key=${_line%%=*}
		_val=${_line#*=}
		case $_key in
		product) _product=$_val ;;
		version) _version=$_val ;;
		target) _target=$_val ;;
		commit) _commit=$_val ;;
		lock_sha256) _lock=$_val ;;
		*)
			IFS=$_oldifs
			refuse release-invalid "unexpected key ${_key}"
			;;
		esac
	done <<EOF
$_text
EOF
	IFS=$_oldifs
	[ "$_product" = "$PRODUCT" ] || refuse release-invalid "product"
	is_hex "$_commit" 40 || refuse release-invalid "commit"
	is_hex "$_lock" 64 || refuse release-invalid "lock_sha256"
	[ "$_target" = "$_want_target" ] || refuse release-invalid "target"
	if [ -n "$_want_version" ] && [ "$_version" != "$_want_version" ]; then
		refuse version-mismatch "$_version"
	fi
	RELEASE_VERSION=$_version
	RELEASE_TARGET=$_target
	RELEASE_COMMIT=$_commit
	RELEASE_LOCK=$_lock
}

member_has_dotdot() {
	_path=$1
	_oldifs=$IFS
	IFS=/
	# shellcheck disable=SC2086
	set -- $_path
	IFS=$_oldifs
	for _part in "$@"; do
		[ "$_part" = ".." ] && return 0
	done
	return 1
}

scan_archive() {
	_archive=$1
	_names=$(tar -tzf "$_archive") || refuse release-invalid "unreadable archive"
	_listing=$(tar -tvzf "$_archive") || refuse release-invalid "unreadable archive"
	_oldifs=$IFS
	IFS=
	while read -r _name; do
		[ -n "$_name" ] || continue
		_name=${_name%/}
		case $_name in
		/*) refuse archive-absolute-path "$_name" ;;
		esac
		member_has_dotdot "$_name" && refuse archive-parent-traversal "$_name"
	done <<EOF
$_names
EOF
	IFS=$_oldifs
	_symlinks=$(printf '%s\n' "$_listing" | awk '
		substr($0,1,1)=="l" {
			for (i=1;i<=NF;i++) if ($i=="->") { print $(i-1); break }
		}
	')
	_hardlinks=$(printf '%s\n' "$_listing" | awk 'substr($0,1,1)=="h" {print $NF}')
	if [ -n "$_hardlinks" ]; then
		refuse archive-hardlink-escape "$_hardlinks"
	fi
	IFS=
	while read -r _line; do
		[ -n "$_line" ] || continue
		case $(printf '%s' "$_line" | awk '{print substr($0,1,1)}') in
		l)
			_name=$(printf '%s' "$_line" | awk '{for (i=1;i<=NF;i++) if ($i=="->") {print $(i-1); exit}}')
			_target=$(printf '%s' "$_line" | awk '{for (i=1;i<=NF;i++) if ($i=="->") {print $(i+1); exit}}')
			case $_target in
			/*) refuse archive-symlink-escape "$_name" ;;
			esac
			member_has_dotdot "$_target" && refuse archive-symlink-escape "$_name"
			;;
		esac
	done <<EOF
$_listing
EOF
	IFS=$_oldifs
	IFS=
	while read -r _name; do
		[ -n "$_name" ] || continue
		_name=${_name%/}
		IFS=
		while read -r _link; do
			[ -n "$_link" ] || continue
			case $_name in
			"${_link}"/*) refuse archive-symlink-then-child "$_name" ;;
			esac
		done <<EOF2
$_symlinks
EOF2
	done <<EOF
$_names
EOF
	IFS=$_oldifs
}

flip_current() {
	_prefix=$1
	_dest=$2
	_current=${_prefix}/current
	_rel=versions/${_dest##*/versions/}
	# Not `ln -s "$_rel" "$_tmp"; mv -f "$_tmp" "$_current"`: once an install
	# already exists, `$_current` is a symlink that resolves to a directory,
	# and POSIX `mv` stats its destination -- following the symlink -- to
	# decide whether to move the source INTO that directory rather than
	# replace the link itself. On every upgrade over an existing install (not
	# just a same-version respin) that silently left `current` pointed at the
	# old build while reporting success. `ln -sfn` never dereferences
	# `$_current` to decide that, so it replaces the link itself -- the same
	# primitive this file's own failure-path already trusts to restore
	# `$OLD_CURRENT` below.
	ln -sfn "$_rel" "$_current"
}

write_profile() {
	_prefix=$1
	if [ -n "${SOLSTONE_PROFILE:-}" ]; then
		write_one_profile "$_prefix" "$SOLSTONE_PROFILE"
		return 0
	fi
	write_one_profile "$_prefix" "$HOME/.profile"
	# macOS logs users into zsh, which reads .zprofile and never .profile. A
	# Linux-derived proof cannot see this: `sh -l` reads .profile on both
	# platforms and reports success while a real owner's shell has no journal
	# on PATH. Both files carry the same marked block, so re-running is
	# idempotent on either.
	case ${TARGET:-} in
	macos-*) write_one_profile "$_prefix" "$HOME/.zprofile" ;;
	esac
}

write_one_profile() {
	_prefix=$1
	_profile=$2
	_dir=$(dirname "$_profile")
	mkdir -p "$_dir"
	_tmp=$(mktemp "$WORK/solstone-install-profile-XXXXXX")
	if [ -f "$_profile" ]; then
		awk -v begin="$PROFILE_BEGIN" -v end="$PROFILE_END" '
			$0 == begin {skip=1; next}
			$0 == end {skip=0; next}
			skip != 1 {print}
		' "$_profile" >"$_tmp"
	else
		: >"$_tmp"
	fi
	{
		cat "$_tmp"
		printf '%s\n' "$PROFILE_BEGIN"
		printf 'PATH="%s/current/bin${PATH:+:$PATH}"\n' "$_prefix"
		printf '%s\n' "export PATH"
		printf '%s\n' "$PROFILE_END"
	} >"${_tmp}.out"
	if [ -f "$_profile" ]; then
		cat "${_tmp}.out" >"$_profile"
	else
		cp "${_tmp}.out" "$_profile"
	fi
	rm -f "$_tmp" "${_tmp}.out"
}

report_success() {
	_prefix=$1
	printf 'installed %s %s at %s\n' "$PRODUCT" "$VERSION" "$_prefix"
	printf 'current -> %s\n' "$(readlink "$_prefix/current")"
	if [ "$NO_PATH" -eq 1 ]; then
		printf 'PATH not updated (--no-path)\n'
	elif [ -n "${SOLSTONE_PROFILE:-}" ]; then
		printf 'PATH updated in %s\n' "$SOLSTONE_PROFILE"
		printf 'open a new terminal, or: . %s\n' "$SOLSTONE_PROFILE"
	else
		case ${TARGET:-} in
		macos-*)
			printf 'PATH updated in ~/.zprofile and ~/.profile\n'
			printf 'open a new terminal, or: . ~/.zprofile\n'
			;;
		*)
			printf 'PATH updated in ~/.profile\n'
			printf 'open a new terminal, or: . ~/.profile\n'
			;;
		esac
	fi
	printf 'then: journal --version\n'
}

detect_target

TMP_ROOT=${TMPDIR:-/var/tmp}
if [ ! -d "$TMP_ROOT" ] || [ ! -w "$TMP_ROOT" ]; then
	refuse tmpdir-unusable "$TMP_ROOT"
fi
WORK=$(mktemp -d "$TMP_ROOT/solstone-install-work-XXXXXX") || refuse tmpdir-unusable "$TMP_ROOT"
cleanup() {
	rm -rf "$WORK"
}
trap cleanup 0 1 2 15

if [ -n "$ARCHIVE" ]; then
	[ -f "$ARCHIVE" ] || refuse digest-mismatch "archive missing"
	[ -n "$SHA256_FILE" ] || refuse digest-mismatch "sha256 sidecar missing"
	[ -n "$RELEASE_FILE" ] || refuse release-invalid "release sidecar missing"
	cp "$ARCHIVE" "$WORK/tree.tar.gz"
	cp "$SHA256_FILE" "$WORK/tree.sha256"
	cp "$RELEASE_FILE" "$WORK/tree.release"
	_archive_name=${ARCHIVE##*/}
else
	_origin=${ORIGIN%/}
	if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then
		refuse fetcher-missing
	fi
	if [ -z "$VERSION" ]; then
		_latest=$(mktemp "$WORK/solstone-install-latest-XXXXXX")
		fetch_url "${_origin}/solstone-journal/${LANE}/latest" "$_latest" latest-invalid
		_nlines=$(wc -l <"$_latest" | tr -d ' ')
		[ "$_nlines" -eq 1 ] || refuse latest-invalid
		_line=$(cat "$_latest")
		case $_line in
		version=*) ;;
		*) refuse latest-invalid ;;
		esac
		_token=${_line#version=}
		case $_token in
		"" | . | .. | */*) refuse latest-invalid "$_token" ;;
		esac
		VERSION=$_token
	fi
	_base=${PRODUCT}-${VERSION}-${TARGET}
	_object_base="${_origin}/solstone-journal/${LANE}/${VERSION}"
	check_origin_url "${_object_base}/${_base}.tar.gz"
	fetch_url "${_object_base}/${_base}.tar.gz" "$WORK/tree.tar.gz"
	fetch_url "${_object_base}/${_base}.sha256" "$WORK/tree.sha256"
	fetch_url "${_object_base}/${_base}.release" "$WORK/tree.release"
	_archive_name=${_base}.tar.gz
fi

EXPECTED=$(parse_sha256_file "$WORK/tree.sha256" "$_archive_name")
ACTUAL=$(digest_file "$WORK/tree.tar.gz")
[ "$EXPECTED" = "$ACTUAL" ] || refuse digest-mismatch
DIGEST12=$(printf '%s' "$ACTUAL" | cut -c1-12)

RELEASE_TEXT=$(cat "$WORK/tree.release")
validate_release "$RELEASE_TEXT" "$VERSION" "$TARGET"
VERSION=$RELEASE_VERSION

scan_archive "$WORK/tree.tar.gz"

mkdir -p "$PREFIX/versions"
DEST=$PREFIX/versions/${VERSION}-${DIGEST12}
CURRENT=$PREFIX/current

# A version directory is named `${VERSION}-${DIGEST12}`, so it is already
# content-addressed: a rebuilt archive for the same version that carries
# different bytes lands at a different, brand-new DEST rather than colliding
# with one that exists. Two builds of one version living side by side under
# `versions/` is therefore not a conflict to refuse -- it is exactly what a
# respin before release looks like, and the documented upgrade route (this
# script, then `journal setup`) depends on being able to install it. Refusing
# it here is what made a legitimate newer build of an already-installed
# version un-installable; the digest/release-record checks above this block
# are what still catch a genuinely bad or foreign artifact, and neither one
# is touched by removing this.
#
# The only case handled specially is a true no-op: this exact digest is
# already installed AND `current` already points at it, so nothing on disk
# needs to change.
if [ -e "$DEST" ] && [ -L "$CURRENT" ]; then
	_now=$(readlink "$CURRENT")
	_want=versions/${VERSION}-${DIGEST12}
	if [ "$_now" = "$_want" ]; then
		# Validated no-op: re-read release, do not rewrite current.
		validate_release "$(cat "$DEST/.release")" "$VERSION" "$TARGET"
		if [ "$NO_PATH" -eq 0 ]; then
			write_profile "$PREFIX"
		fi
		report_success "$PREFIX"
		exit 0
	fi
fi

PARTIAL=$PREFIX/versions/.partial-${VERSION}-${DIGEST12}
rm -rf "$PARTIAL"
mkdir -p "$PARTIAL"
if ! tar -xzf "$WORK/tree.tar.gz" -C "$PARTIAL"; then
	rm -rf "$PARTIAL"
	refuse release-invalid "extract failed"
fi
printf '%s\n' "$RELEASE_TEXT" >"$PARTIAL/.release"
printf '%s\n' "$ACTUAL" >"$PARTIAL/.archive-sha256"

if [ -e "$DEST" ]; then
	rm -rf "$PARTIAL"
else
	mv "$PARTIAL" "$DEST"
fi

OLD_CURRENT=
if [ -L "$CURRENT" ]; then
	OLD_CURRENT=$(readlink "$CURRENT")
fi
if ! flip_current "$PREFIX" "$DEST"; then
	if [ -n "$OLD_CURRENT" ]; then
		ln -sfn "$OLD_CURRENT" "$CURRENT"
	fi
	refuse release-invalid "current flip failed"
fi
if [ "$NO_PATH" -eq 0 ]; then
	write_profile "$PREFIX"
fi
report_success "$PREFIX"
exit 0
