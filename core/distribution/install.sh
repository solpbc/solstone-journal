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
#   digest-mismatch
#   release-invalid
#   version-mismatch
#   conflicting-digest

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
	printf '%s\n' "usage: install.sh [--prefix DIR] [--version VER] [--origin URL] [--archive FILE] [--sha256 FILE] [--release FILE]"
}

PREFIX=
VERSION=
ORIGIN=
ARCHIVE=
SHA256_FILE=
RELEASE_FILE=
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
	--help | -h)
		usage
		exit 0
		;;
	*)
		refuse release-invalid "unknown argument $1"
		;;
	esac
done

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
	linux) ;;
	*) refuse unsupported-platform "os=${_os}" ;;
	esac
	case ${_arch_lc} in
	x86_64 | amd64) TARGET=linux-x86_64 ;;
	aarch64 | arm64) TARGET=linux-aarch64 ;;
	*) refuse unsupported-platform "arch=${_arch}" ;;
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
	_line=$(awk 'NF {print; exit}' "$_path")
	_digest=${_line%% *}
	is_hex "$_digest" 64 || refuse digest-mismatch "sha256 sidecar"
	printf '%s' "$_digest"
}

fetch_url() {
	_url=$1
	_dest=$2
	check_origin_url "$_url"
	_hops=0
	_current=$_url
	while [ "$_hops" -le "$MAX_HOPS" ]; do
		check_origin_url "$_current"
		if command -v curl >/dev/null 2>&1; then
			_hdrs=$(mktemp)
			_code=$(curl -sS --http1.1 -D "$_hdrs" -o "$_dest" -w '%{http_code}' "$_current" || true)
			_location=$(awk 'BEGIN{IGNORECASE=1} /^Location:/{sub(/\r$/,""); sub(/^Location:[[:space:]]+/,""); print; exit}' "$_hdrs")
			rm -f "$_hdrs"
		elif command -v wget >/dev/null 2>&1; then
			_hdrs=$(mktemp)
			if wget -qS -O "$_dest" "$_current" 2>"$_hdrs"; then
				_code=200
			else
				_code=$(awk '/^  HTTP\//{print $2; exit}' "$_hdrs")
			fi
			_location=$(awk 'BEGIN{IGNORECASE=1} /^  Location:/{sub(/\r$/,""); sub(/^  Location:[[:space:]]+/,""); print; exit}' "$_hdrs")
			rm -f "$_hdrs"
		else
			refuse fetcher-missing
		fi
		case $_code in
		200)
			return 0
			;;
		301 | 302 | 303 | 307 | 308)
			[ -n "$_location" ] || refuse origin-refused "redirect without Location"
			_hops=$((_hops + 1))
			[ "$_hops" -le "$MAX_HOPS" ] || refuse origin-refused "too many redirects"
			case $_location in
			http://* | https://*) _current=$_location ;;
			/*)
				_scheme=$(origin_scheme "$_current")
				_host=$(origin_host "$_current")
				_current=${_scheme}://${_host}${_location}
				;;
			*) refuse origin-refused "relative redirect" ;;
			esac
			;;
		*)
			refuse origin-refused "http ${_code}"
			;;
		esac
	done
	refuse origin-refused "too many redirects"
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
	_tmp=${_prefix}/.current.new
	_rel=versions/${_dest##*/versions/}
	ln -s "$_rel" "$_tmp"
	mv -f "$_tmp" "$_current"
}

write_profile() {
	_prefix=$1
	_profile=${SOLSTONE_PROFILE:-$HOME/.profile}
	_dir=$(dirname "$_profile")
	mkdir -p "$_dir"
	_tmp=$(mktemp)
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

detect_target

WORK=$(mktemp -d)
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
else
	[ -n "$VERSION" ] || refuse release-invalid "version required to fetch"
	_base=${PRODUCT}-${VERSION}-${TARGET}
	_origin=${ORIGIN%/}
	check_origin_url "${_origin}/${_base}.tar.gz"
	if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then
		refuse fetcher-missing
	fi
	fetch_url "${_origin}/${_base}.tar.gz" "$WORK/tree.tar.gz"
	fetch_url "${_origin}/${_base}.tar.gz.sha256" "$WORK/tree.sha256"
	fetch_url "${_origin}/${_base}.release" "$WORK/tree.release"
fi

EXPECTED=$(parse_sha256_file "$WORK/tree.sha256")
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

if [ -e "$DEST" ]; then
	_existing=
	for _cand in "$PREFIX"/versions/"${VERSION}"-*; do
		[ -e "$_cand" ] || continue
		if [ "$_cand" != "$DEST" ]; then
			refuse conflicting-digest "$_cand"
		fi
		_existing=$_cand
	done
	if [ -n "$_existing" ] && [ -L "$CURRENT" ]; then
		_now=$(readlink "$CURRENT")
		_want=versions/${VERSION}-${DIGEST12}
		if [ "$_now" = "$_want" ]; then
			# Validated no-op: re-read release, do not rewrite current.
			validate_release "$(cat "$DEST/.release")" "$VERSION" "$TARGET"
			write_profile "$PREFIX"
			exit 0
		fi
	fi
else
	for _cand in "$PREFIX"/versions/"${VERSION}"-*; do
		[ -e "$_cand" ] || continue
		refuse conflicting-digest "$_cand"
	done
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
write_profile "$PREFIX"
exit 0
