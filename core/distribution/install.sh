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
#   verifier-missing
#   signature-invalid
#   receipt-invalid
#   receipt-schema-unsupported
#   route-busy
#   route-unknown
#   package-route
#   v1-handoff
#   upgrade-not-installed
#   lane-unknown
#   setup-failed
#   downgrade-epoch
#   downgrade-window
#   version-order-unknown
#   retention-policy-unsupported
#   prune-unsafe

set -eu

PRODUCT=solstone-journal
ORIGIN_HOST=updates.solstone.app
MAX_HOPS=5
PROFILE_BEGIN="# BEGIN solstone-journal PATH"
PROFILE_END="# END solstone-journal PATH"
MINISIGN_KEY_ID=B44073BF49E0D944
MINISIGN_PUBLIC_KEY=RWRE2eBJv3NAtN0mF5+kqygYyP/ocYNw1Ng9yJhAKgyTflNV9NabMMjq
RECEIPT_SCHEMA_VERSION=1
SUPPORTED_UPGRADE_EPOCH=journal-v2
SUPPORTED_RETENTION_WINDOW=3

refuse() {
	_name=$1
	shift
	if [ "$#" -gt 0 ]; then
		_detail=$*
		[ -z "${REFUSAL_HINT:-}" ] || _detail="${_detail}; ${REFUSAL_HINT}"
		printf '%s\n' "${_name}: ${_detail}" >&2
	elif [ -n "${REFUSAL_HINT:-}" ]; then
		printf '%s\n' "${_name}: ${REFUSAL_HINT}" >&2
	else
		printf '%s\n' "${_name}" >&2
	fi
	exit 2
}

usage() {
	printf '%s\n' "usage: install.sh [--prefix DIR] [--version VER] [--lane LANE] [--origin URL] [--archive FILE] [--sha256 FILE] [--release FILE] [--manifest FILE] [--minisig FILE] [--skip-signature] [--upgrade] [--prune] [--no-path]"
}

PREFIX=
VERSION=
LANE=release
LANE_EXPLICIT=0
ORIGIN=
ORIGIN_EXPLICIT=0
ARCHIVE=
SHA256_FILE=
RELEASE_FILE=
MANIFEST_FILE=
MINISIG_FILE=
SKIP_SIGNATURE=0
UPGRADE=0
PRUNE=0
NO_PATH=0
WORK=
PARTIAL=
RECEIPT_PARTIAL=
ROUTE_LOCK=
ROUTE_LOCK_TOKEN=
ROUTE_LOCK_HELD=0
PREFIX_CREATED=0
REFUSAL_HINT=
SETUP_TRANSACTION_ACTIVE=0
DEST_CREATED=0
DEST_MOVE_PENDING=0
DEST_OWNER_FILE=
DEST_NESTED_PARTIAL=
DEST_NESTED_MARKER=

cleanup() {
	if [ -n "$RECEIPT_PARTIAL" ]; then
		_receipt_partial=$RECEIPT_PARTIAL
		RECEIPT_PARTIAL=
		rm -f -- "$_receipt_partial"
	fi
	if [ -n "$PARTIAL" ]; then
		_partial=$PARTIAL
		PARTIAL=
		rm -rf -- "$_partial"
	fi
	if [ -n "$WORK" ]; then
		_work=$WORK
		WORK=
		rm -rf -- "$_work"
	fi
	if [ "$ROUTE_LOCK_HELD" -eq 1 ]; then
		rm -f -- "$ROUTE_LOCK/pid" "$ROUTE_LOCK/owner"
		rmdir -- "$ROUTE_LOCK" 2>/dev/null || true
		ROUTE_LOCK_HELD=0
	fi
	if [ "$DEST_MOVE_PENDING" -eq 1 ]; then
		if destination_is_transaction_owned; then
			DEST_CREATED=1
		elif path_is_transaction_owned "$DEST_NESTED_PARTIAL" "$DEST_NESTED_MARKER"; then
			rm -rf -- "$DEST_NESTED_PARTIAL" || true
		fi
	fi
	DEST_MOVE_PENDING=0
	if [ "$DEST_CREATED" -eq 1 ]; then
		if ! current_selects_destination; then
			if destination_is_transaction_owned; then
				rm -rf -- "$DEST" || true
			fi
		elif destination_is_transaction_owned; then
			rm -f -- "$DEST_OWNER_FILE" || true
		fi
		DEST_CREATED=0
	fi
	DEST_NESTED_PARTIAL=
	DEST_NESTED_MARKER=
	if [ "$PREFIX_CREATED" -eq 1 ]; then
		rmdir -- "$PREFIX/versions" 2>/dev/null || true
		rmdir -- "$PREFIX" 2>/dev/null || true
		PREFIX_CREATED=0
	fi
}

handle_signal() {
	_status=$1
	trap - 0 1 2 15
	if [ "$SETUP_TRANSACTION_ACTIVE" -eq 1 ]; then
		publish_signal_receipt || true
	fi
	cleanup
	exit "$_status"
}

publish_signal_receipt() {
	[ -n "$RECEIPT_PARTIAL" ] && [ -f "$RECEIPT_PARTIAL" ] && [ ! -L "$RECEIPT_PARTIAL" ] \
		|| return 1
	current_selects_destination || return 1
	grep -Fqx 'setup_status=pending' "$RECEIPT_PARTIAL" \
		|| grep -Fqx 'setup_status=complete' "$RECEIPT_PARTIAL" \
		|| return 1
	mv -f "$RECEIPT_PARTIAL" "$PREFIX/install-receipt" || return 1
	RECEIPT_PARTIAL=
	return 0
}

current_selects_destination() {
	[ -n "${CURRENT:-}" ] && [ -n "${DEST:-}" ] && [ -L "$CURRENT" ] || return 1
	_selected_current=$(readlink "$CURRENT") || return 1
	[ "$_selected_current" = "versions/${DEST##*/versions/}" ]
}

destination_is_transaction_owned() {
	path_is_transaction_owned "${DEST:-}" "$DEST_OWNER_FILE"
}

path_is_transaction_owned() {
	_owned_path=$1
	_owned_marker=$2
	[ -n "$_owned_path" ] && [ -n "$_owned_marker" ] \
		&& [ -d "$_owned_path" ] && [ ! -L "$_owned_path" ] \
		&& [ -f "$_owned_marker" ] && [ ! -L "$_owned_marker" ] \
		|| return 1
	[ "$(cat "$_owned_marker")" = "$ROUTE_LOCK_TOKEN" ]
}

parse_args() {
	while [ "$#" -gt 0 ]; do
		case $1 in
		--prefix)
			[ "$#" -ge 2 ] || refuse release-invalid "--prefix requires a value"
			PREFIX=$2
			shift 2
			;;
		--version)
			[ "$#" -ge 2 ] || refuse release-invalid "--version requires a value"
			VERSION=$2
			shift 2
			;;
		--lane)
			[ "$#" -ge 2 ] || refuse release-invalid "--lane requires a value"
			LANE=$2
			LANE_EXPLICIT=1
			shift 2
			;;
		--origin)
			[ "$#" -ge 2 ] || refuse release-invalid "--origin requires a value"
			ORIGIN=$2
			ORIGIN_EXPLICIT=1
			shift 2
			;;
		--archive)
			[ "$#" -ge 2 ] || refuse release-invalid "--archive requires a value"
			ARCHIVE=$2
			shift 2
			;;
		--sha256)
			[ "$#" -ge 2 ] || refuse release-invalid "--sha256 requires a value"
			SHA256_FILE=$2
			shift 2
			;;
		--release)
			[ "$#" -ge 2 ] || refuse release-invalid "--release requires a value"
			RELEASE_FILE=$2
			shift 2
			;;
		--manifest)
			[ "$#" -ge 2 ] || refuse release-invalid "--manifest requires a value"
			MANIFEST_FILE=$2
			shift 2
			;;
		--minisig)
			[ "$#" -ge 2 ] || refuse release-invalid "--minisig requires a value"
			MINISIG_FILE=$2
			shift 2
			;;
		--skip-signature)
			SKIP_SIGNATURE=1
			shift
			;;
		--upgrade)
			UPGRADE=1
			shift
			;;
		--prune)
			PRUNE=1
			shift
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
}

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

acquire_route_lock() {
	if [ -e "$PREFIX" ] || [ -L "$PREFIX" ]; then
		[ -d "$PREFIX" ] && [ ! -L "$PREFIX" ] \
			|| refuse route-unknown "the selected prefix is not a real directory; leave it untouched and choose another --prefix"
	else
		mkdir -p "$PREFIX" || refuse route-unknown "could not create the selected prefix; choose a writable --prefix"
		PREFIX_CREATED=1
	fi
	ROUTE_LOCK_TOKEN=$(LC_ALL=C od -An -tx1 -N16 /dev/urandom | tr -d ' \n') \
		|| refuse route-busy "could not generate a route-lock owner token; retry after checking /dev/urandom"
	is_hex "$ROUTE_LOCK_TOKEN" 32 \
		|| refuse route-busy "could not generate a valid route-lock owner token; retry"
	ROUTE_LOCK=$PREFIX/.solstone-route.lock
	_old_umask=$(umask)
	umask 077
	if ! mkdir "$ROUTE_LOCK" 2>/dev/null; then
		if route_lock_owner_is_stale; then
			rm -f -- "$ROUTE_LOCK/pid" "$ROUTE_LOCK/owner"
			rmdir -- "$ROUTE_LOCK" 2>/dev/null \
				|| {
					umask "$_old_umask"
					refuse route-busy "a stale route lock could not be removed; if no install.sh is running, remove $ROUTE_LOCK and retry"
				}
			mkdir "$ROUTE_LOCK" 2>/dev/null \
				|| {
					umask "$_old_umask"
					refuse route-busy "another install transaction acquired $ROUTE_LOCK; wait for it to finish, then retry"
				}
		else
			umask "$_old_umask"
			refuse route-busy "another install transaction may own $ROUTE_LOCK; wait for it to finish, or if no install.sh is running remove that exact lock directory and retry"
		fi
	fi
	ROUTE_LOCK_HELD=1
	printf 'solstone-route-lock-v1\n%s\n' "$ROUTE_LOCK_TOKEN" >"$ROUTE_LOCK/owner" \
		|| refuse route-busy "could not write route-lock ownership; retry"
	printf '%s\n' "$$" >"$ROUTE_LOCK/pid" \
		|| refuse route-busy "could not write route-lock process identity; retry"
	chmod 700 "$ROUTE_LOCK" || refuse route-busy "could not secure the route lock; retry"
	chmod 600 "$ROUTE_LOCK/owner" || refuse route-busy "could not secure route-lock ownership; retry"
	chmod 600 "$ROUTE_LOCK/pid" || refuse route-busy "could not secure route-lock process identity; retry"
	umask "$_old_umask"
}

route_lock_owner_is_stale() {
	[ -d "$ROUTE_LOCK" ] && [ ! -L "$ROUTE_LOCK" ] || return 1
	[ -f "$ROUTE_LOCK/owner" ] && [ ! -L "$ROUTE_LOCK/owner" ] || return 1
	[ -f "$ROUTE_LOCK/pid" ] && [ ! -L "$ROUTE_LOCK/pid" ] || return 1
	_owner_text=$(cat "$ROUTE_LOCK/owner") || return 1
	_owner_magic=${_owner_text%%
*}
	_owner_token=${_owner_text#*
}
	[ "$_owner_magic" = solstone-route-lock-v1 ] || return 1
	is_hex "$_owner_token" 32 || return 1
	_owner_pid=$(cat "$ROUTE_LOCK/pid") || return 1
	case $_owner_pid in
	'' | *[!0-9]*) return 1 ;;
	esac
	if kill -0 "$_owner_pid" 2>/dev/null \
		|| [ -d "/proc/$_owner_pid" ] \
		|| { command -v ps >/dev/null 2>&1 && ps -p "$_owner_pid" >/dev/null 2>&1; }; then
		return 1
	fi
	return 0
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

minisign_install_hint() {
	_id=
	_like=
	if [ -r /etc/os-release ]; then
		_id=$(awk -F= '$1 == "ID" {gsub(/^"|"$/, "", $2); print $2; exit}' /etc/os-release)
		_like=$(awk -F= '$1 == "ID_LIKE" {gsub(/^"|"$/, "", $2); print $2; exit}' /etc/os-release)
	fi
	case " ${_id} ${_like} " in
	*" debian "* | *" ubuntu "*) printf '%s' "sudo apt install minisign" ;;
	*" fedora "* | *" rhel "* | *" centos "*) printf '%s' "sudo dnf install minisign" ;;
	*) printf '%s' "install minisign, then run this command again" ;;
	esac
}

manifest_member_digest() {
	_manifest=$1
	_member=$2
	_digest=$(awk -v want="$_member" '
		BEGIN { RS="\""; count=0 }
		$0 == want {
			if ((getline separator) <= 0 || (getline value) <= 0 || separator !~ /^[[:space:]]*:[[:space:]]*$/) next
			count++
			found=value
		}
		END { if (count == 1) print found }
	' "$_manifest")
	is_hex "$_digest" 64 || refuse signature-invalid "signed manifest member ${_member}"
	printf '%s' "$_digest"
}

verify_signed_release_set() {
	_manifest=$1
	_signature=$2
	_archive_name=$3
	_sha_name=$4
	_release_name=$5
	if [ ! -f "$_manifest" ] || [ -L "$_manifest" ]; then
		refuse signature-invalid "manifest missing or not a regular file"
	fi
	if [ ! -f "$_signature" ] || [ -L "$_signature" ]; then
		refuse signature-invalid "signature missing or not a regular file"
	fi
	command -v minisign >/dev/null 2>&1 || refuse verifier-missing "$(minisign_install_hint)"
	_pin=$WORK/solstone-journal-release.pub
	printf '%s\n%s\n' "untrusted comment: minisign public key ${MINISIGN_KEY_ID}" "$MINISIGN_PUBLIC_KEY" >"$_pin"
	if ! minisign -Vm "$_manifest" -x "$_signature" -p "$_pin" >/dev/null 2>&1; then
		refuse signature-invalid "manifest signature"
	fi
	for _member in "$_archive_name:$WORK/tree.tar.gz" "$_sha_name:$WORK/tree.sha256" "$_release_name:$WORK/tree.release"; do
		_name=${_member%%:*}
		_path=${_member#*:}
		_expected=$(manifest_member_digest "$_manifest" "$_name")
		_actual=$(digest_file "$_path")
		[ "$_expected" = "$_actual" ] || refuse signature-invalid "signed manifest digest for ${_name}"
	done
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
	case $_lines in
	5 | 7 | 10) ;;
	*) refuse release-invalid "expected 5 legacy, 7 current, or 10 macos fields" ;;
	esac
	_product=
	_version=
	_target=
	_commit=
	_lock=
	_epoch=
	_window=
	_archive_prebuild=
	_archive_delivery=
	_archive_invocation=
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
		upgrade_epoch) _epoch=$_val ;;
		retention_window) _window=$_val ;;
		archive_prebuild_input_sha256) _archive_prebuild=$_val ;;
		archive_delivery_contract_sha256) _archive_delivery=$_val ;;
		archive_final_invocation_sha256) _archive_invocation=$_val ;;
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
	case $_version in
	[0-9]*.[0-9]*.[0-9]*) ;;
	*) refuse release-invalid "version must be numeric MAJOR.MINOR.PATCH" ;;
	esac
	[ "$(printf '%s' "$_version" | awk -F. 'NF == 3 && $1 ~ /^[0-9]+$/ && $2 ~ /^[0-9]+$/ && $3 ~ /^[0-9]+$/ { print "yes" }')" = yes ] \
		|| refuse release-invalid "version must be numeric MAJOR.MINOR.PATCH"
	is_hex "$_commit" 40 || refuse release-invalid "commit"
	is_hex "$_lock" 64 || refuse release-invalid "lock_sha256"
	if [ "$_lines" -eq 5 ]; then
		_epoch=unknown
		_window=unknown
	else
		[ -n "$_epoch" ] || refuse release-invalid "upgrade_epoch"
		case $_window in
		'' | *[!0-9]*) refuse release-invalid "retention_window" ;;
		esac
		[ "$_window" -gt 0 ] || refuse release-invalid "retention_window"
	fi
	[ "$_target" = "$_want_target" ] || refuse release-invalid "target"
	if [ "$_lines" -eq 10 ]; then
		[ "$_target" = macos-arm64 ] \
			|| refuse release-invalid "archive-chain fields are macos-only"
		is_hex "$_archive_prebuild" 64 || refuse release-invalid "archive_prebuild_input_sha256"
		is_hex "$_archive_delivery" 64 || refuse release-invalid "archive_delivery_contract_sha256"
		is_hex "$_archive_invocation" 64 || refuse release-invalid "archive_final_invocation_sha256"
	fi
	if [ -n "$_want_version" ] && [ "$_version" != "$_want_version" ]; then
		refuse version-mismatch "$_version"
	fi
	RELEASE_VERSION=$_version
	RELEASE_COMMIT=$_commit
	RELEASE_EPOCH=$_epoch
	RELEASE_RETENTION_WINDOW=$_window
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

restore_previous_current() {
	if [ -n "$OLD_CURRENT" ]; then
		ln -sfn "$OLD_CURRENT" "$CURRENT" || return 1
	else
		rm -f -- "$CURRENT" || return 1
	fi
	if [ "$DEST_CREATED" -eq 1 ]; then
		destination_is_transaction_owned || return 1
		rm -rf -- "$DEST" || return 1
		[ ! -e "$DEST" ] && [ ! -L "$DEST" ] || return 1
		DEST_CREATED=0
	fi
	return 0
}

refuse_after_rollback_failure() {
	_reason=$1
	publish_signal_receipt || true
	SETUP_TRANSACTION_ACTIVE=0
	refuse setup-failed "${_reason}; the previous current target could not be restored; inspect ${CURRENT}, then rerun this same install.sh command"
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
	printf 'lane=%s\n' "$LANE"
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

package_upgrade_command() {
	if command -v dpkg-query >/dev/null 2>&1 \
		&& dpkg-query -W -f='${Status}' solstone-journal 2>/dev/null | grep -F 'install ok installed' >/dev/null 2>&1; then
		printf '%s' "sudo apt upgrade solstone-journal"
		return 0
	fi
	if command -v rpm >/dev/null 2>&1 && rpm -q solstone-journal >/dev/null 2>&1; then
		printf '%s' "sudo dnf upgrade solstone-journal"
		return 0
	fi
	return 1
}

positive_tree_release() {
	[ -d "$PREFIX/versions" ] && [ ! -L "$PREFIX/versions" ] || return 1
	[ -L "$PREFIX/current" ] || return 1
	_link=$(readlink "$PREFIX/current") || return 1
	case $_link in
	versions/*) _entry=${_link#versions/} ;;
	*) return 1 ;;
	esac
	case $_entry in
	"" | . | .. | */*) return 1 ;;
	esac
	_dest=$PREFIX/versions/$_entry
	[ -d "$_dest" ] && [ ! -L "$_dest" ] || return 1
	[ -f "$_dest/.release" ] && [ ! -L "$_dest/.release" ] || return 1
	[ -f "$_dest/.archive-sha256" ] && [ ! -L "$_dest/.archive-sha256" ] || return 1
	_recorded_digest=$(cat "$_dest/.archive-sha256")
	is_hex "$_recorded_digest" 64 || return 1
	_digest12=$(printf '%s' "$_recorded_digest" | cut -c1-12)
	case $_entry in
	*-"$_digest12") _installed_version=${_entry%"-$_digest12"} ;;
	*) return 1 ;;
	esac
	REFUSAL_HINT="leave the existing tree untouched and run journal setup; if it remains refused, choose a different --prefix"
	validate_release "$(cat "$_dest/.release")" "$_installed_version" "$TARGET"
	REFUSAL_HINT=
	INSTALLED_VERSION=$RELEASE_VERSION
	INSTALLED_DEST=$_dest
	INSTALLED_EPOCH=$RELEASE_EPOCH
	INSTALLED_RETENTION_WINDOW=$RELEASE_RETENTION_WINDOW
	return 0
}

version_is_older() {
	_left=$1
	_right=$2
	awk -v left="$_left" -v right="$_right" '
		function compare_component(a, b, i, ai, bi) {
			sub(/^0+/, "", a)
			sub(/^0+/, "", b)
			if (a == "") a = "0"
			if (b == "") b = "0"
			if (length(a) < length(b)) return -1
			if (length(a) > length(b)) return 1
			for (i = 1; i <= length(a); i++) {
				ai = index("0123456789", substr(a, i, 1))
				bi = index("0123456789", substr(b, i, 1))
				if (ai < bi) return -1
				if (ai > bi) return 1
			}
			return 0
		}
		BEGIN {
			if (split(left, l, ".") != 3 || split(right, r, ".") != 3) exit 2
			for (i = 1; i <= 3; i++) {
				if (l[i] !~ /^[0-9]+$/ || r[i] !~ /^[0-9]+$/) exit 2
				comparison = compare_component(l[i], r[i])
				if (comparison < 0) exit 0
				if (comparison > 0) exit 1
			}
			exit 1
		}' </dev/null
}

validate_installed_destination() {
	_dest=$1
	_digest=$2
	_release_text=$3
	if [ ! -d "$_dest" ] || [ -L "$_dest" ]; then
		refuse route-unknown "the selected version path is not a real directory; leave it untouched and run journal setup"
	fi
	if [ ! -f "$_dest/.release" ] || [ -L "$_dest/.release" ] \
		|| [ ! -f "$_dest/.archive-sha256" ] || [ -L "$_dest/.archive-sha256" ]; then
		refuse route-unknown "the selected version has incomplete provenance; leave it untouched and run journal setup"
	fi
	[ "$(cat "$_dest/.archive-sha256")" = "$_digest" ] \
		|| refuse digest-mismatch "installed destination provenance"
	[ "$(cat "$_dest/.release")" = "$_release_text" ] \
		|| refuse release-invalid "installed destination provenance"
}

within_retention_window() {
	_target=$1
	_current=$2
	_window=$3
	[ "$_target" = "$_current" ] && return 0
	_kept=1
	_oldifs=$IFS
	IFS='
'
	for _candidate in $(ls -1dt -- "$PREFIX"/versions/* 2>/dev/null); do
		[ "$_candidate" = "$_current" ] && continue
		_kept=$((_kept + 1))
		if [ "$_candidate" = "$_target" ] && [ "$_kept" -le "$_window" ]; then
			IFS=$_oldifs
			return 0
		fi
		if [ "$_kept" -ge "$_window" ]; then
			IFS=$_oldifs
			return 1
		fi
	done
	IFS=$_oldifs
	return 1
}

validate_prunable_version_dir() {
	_dir=$1
	[ -d "$_dir" ] && [ ! -L "$_dir" ] \
		|| refuse prune-unsafe "not a real version directory: $_dir"
	_name=${_dir##*/}
	case $_name in
	"" | . | .. | */*) refuse prune-unsafe "unsafe version directory name" ;;
	esac
	[ -f "$_dir/.release" ] && [ ! -L "$_dir/.release" ] \
		|| refuse prune-unsafe "missing release provenance: $_name"
	[ -f "$_dir/.archive-sha256" ] && [ ! -L "$_dir/.archive-sha256" ] \
		|| refuse prune-unsafe "missing archive provenance: $_name"
	_digest=$(cat "$_dir/.archive-sha256")
	is_hex "$_digest" 64 || refuse prune-unsafe "invalid archive provenance: $_name"
	_digest12=$(printf '%s' "$_digest" | cut -c1-12)
	case $_name in
	*-"$_digest12") _version=${_name%"-$_digest12"} ;;
	*) refuse prune-unsafe "version directory does not match its archive provenance: $_name" ;;
	esac
	validate_release "$(cat "$_dir/.release")" "$_version" "$TARGET"
	[ "$RELEASE_EPOCH" = "$INSTALLED_EPOCH" ] \
		|| refuse prune-unsafe "mixed upgrade epochs under versions/: $_name"
}

prune_versions() {
	[ "$ROUTE" = tree ] || refuse prune-unsafe "only a positively identified tree install can be pruned"
	[ "$INSTALLED_EPOCH" = "$SUPPORTED_UPGRADE_EPOCH" ] \
		|| refuse retention-policy-unsupported "the current release has no supported retention policy"
	[ "$INSTALLED_RETENTION_WINDOW" = "$SUPPORTED_RETENTION_WINDOW" ] \
		|| refuse retention-policy-unsupported "retention_window=${INSTALLED_RETENTION_WINDOW}"
	for _candidate in "$PREFIX"/versions/*; do
		validate_prunable_version_dir "$_candidate"
	done
	_kept=1
	_oldifs=$IFS
	IFS='
'
	for _candidate in $(ls -1dt -- "$PREFIX"/versions/* 2>/dev/null); do
		[ "$_candidate" = "$INSTALLED_DEST" ] && continue
		if [ "$_kept" -lt "$SUPPORTED_RETENTION_WINDOW" ]; then
			_kept=$((_kept + 1))
			continue
		fi
		# Every candidate was positively identified above, and the current
		# directory is excluded by exact path before this destructive step.
		_live_link=$(readlink "$PREFIX/current") \
			|| refuse prune-unsafe "current changed while pruning; retry"
		_live_current=$PREFIX/$_live_link
		[ "$_candidate" != "$_live_current" ] \
			|| refuse prune-unsafe "current changed while pruning; retry"
		rm -rf -- "$_candidate"
	done
	IFS=$_oldifs
	_count=$(find "$PREFIX/versions" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
	printf 'retained %s version directories; current -> %s\n' \
		"$_count" "$(readlink "$PREFIX/current")"
}

read_receipt() {
	_receipt=$1
	if [ ! -f "$_receipt" ] || [ -L "$_receipt" ]; then
		refuse receipt-invalid "not a regular file: $_receipt"
	fi
	_size=$(wc -c <"$_receipt" | tr -d ' ')
	[ "$_size" -le 4096 ] || refuse receipt-invalid "exceeds 4096 bytes"
	RECEIPT_SCHEMA=
	RECEIPT_LANE=
	RECEIPT_ROUTE=
	_seen_schema=0
	_seen_lane=0
	_seen_route=0
	while IFS= read -r _line || [ -n "$_line" ]; do
		case $_line in
		*=*) ;;
		*) refuse receipt-invalid "not key=value" ;;
		esac
		_key=${_line%%=*}
		_value=${_line#*=}
		case $_key in
		schema_version)
			[ "$_seen_schema" -eq 0 ] || refuse receipt-invalid "duplicate schema_version"
			RECEIPT_SCHEMA=$_value
			_seen_schema=1
			;;
		lane)
			[ "$_seen_lane" -eq 0 ] || refuse receipt-invalid "duplicate lane"
			RECEIPT_LANE=$_value
			_seen_lane=1
			;;
		route)
			[ "$_seen_route" -eq 0 ] || refuse receipt-invalid "duplicate route"
			RECEIPT_ROUTE=$_value
			_seen_route=1
			;;
		*) : ;; # Forward-compatible: unknown keys are dispatch hints, not authority.
		esac
	done <"$_receipt"
	[ "$_seen_schema" -eq 1 ] || refuse receipt-invalid "schema_version missing"
	[ "$RECEIPT_SCHEMA" = "$RECEIPT_SCHEMA_VERSION" ] \
		|| refuse receipt-schema-unsupported "schema_version=${RECEIPT_SCHEMA}"
	case $RECEIPT_LANE in
	release | staging | dev | unknown) ;;
	*) refuse receipt-invalid "lane=${RECEIPT_LANE}" ;;
	esac
	[ "$RECEIPT_ROUTE" = tree ] || refuse receipt-invalid "route=${RECEIPT_ROUTE}"
}

detect_existing_route() {
	ROUTE=fresh
	_package_command=
	if _package_command=$(package_upgrade_command); then
		_package_present=1
	else
		_package_present=0
	fi
	_tree_shape=0
	if [ -e "$PREFIX/current" ] || [ -L "$PREFIX/current" ] || [ -e "$PREFIX/versions" ] || [ -e "$PREFIX/install-receipt" ] || [ -L "$PREFIX/install-receipt" ]; then
		_tree_shape=1
	fi
	if [ "$_tree_shape" -eq 1 ]; then
		positive_tree_release || refuse route-unknown "existing tree is not a verified solstone-journal install; leave it untouched and run journal setup"
		[ "$_package_present" -eq 0 ] || refuse route-unknown "both tree and package routes are present; leave both untouched and choose one"
		ROUTE=tree
		if [ -e "$PREFIX/install-receipt" ] || [ -L "$PREFIX/install-receipt" ]; then
			read_receipt "$PREFIX/install-receipt"
			if [ "$LANE_EXPLICIT" -eq 0 ]; then
				LANE=$RECEIPT_LANE
			fi
		else
			if [ "$LANE_EXPLICIT" -eq 0 ]; then
				LANE=unknown
			fi
		fi
	elif [ "$_package_present" -eq 1 ]; then
		ROUTE=package
		refuse package-route "this install is owned by the package database; run: ${_package_command}"
	elif [ -f "$HOME/.local/bin/journal" ] \
		&& grep -E 'python|solstone\.think\.sol_cli' "$HOME/.local/bin/journal" >/dev/null 2>&1; then
		ROUTE=v1
		refuse v1-handoff "the Python install remains owned by journal setup; run: journal setup"
	elif [ "$UPGRADE" -eq 1 ]; then
		refuse upgrade-not-installed "no installed journal route was detected; run the documented install command without --upgrade"
	fi
	case $LANE in
	release | staging | dev | unknown) ;;
	*) refuse lane-invalid "$LANE" ;;
	esac
	[ "$PRUNE" -eq 0 ] || return 0
	if [ "$LANE" = unknown ] && [ -z "$ARCHIVE" ]; then
		refuse lane-unknown "the adopted tree has no lane receipt; rerun with --lane release, staging, or dev"
	fi
}

run_setup() {
	_dest=$1
	[ -x "$_dest/bin/journal" ] || return 1
	if [ "$NO_PATH" -eq 1 ]; then
		"$_dest/bin/journal" setup --yes --skip-path --installer-transaction
	else
		"$_dest/bin/journal" setup --yes --installer-transaction
	fi
}

stage_receipt() {
	_prefix=$1
	_setup_status=${2:-pending}
	_status=verified
	[ "$SKIP_SIGNATURE" -eq 0 ] || _status=skipped
	_origin=${ORIGIN%/}
	[ -n "$ARCHIVE" ] && _origin=local
	_line_count=$(printf '%s' "${VERSION}${LANE}${_origin}${TARGET}${RELEASE_COMMIT}" | wc -l | tr -d ' ')
	[ "$_line_count" -eq 0 ] || refuse receipt-invalid "receipt fields contain a newline"
	RECEIPT_PARTIAL=$(mktemp "$_prefix/.install-receipt-XXXXXX") \
		|| refuse receipt-invalid "cannot stage receipt"
	{
		printf 'schema_version=%s\n' "$RECEIPT_SCHEMA_VERSION"
		printf 'journal_version=%s\n' "$VERSION"
		printf 'lane=%s\n' "$LANE"
		printf 'origin=%s\n' "$_origin"
		printf 'architecture=%s\n' "$TARGET"
		printf 'installer_revision=%s\n' "$RELEASE_COMMIT"
		printf 'route=tree\n'
		printf 'signature_verification=%s\n' "$_status"
		printf 'setup_status=%s\n' "$_setup_status"
	} >"$RECEIPT_PARTIAL" || refuse receipt-invalid "could not stage receipt"
}

mark_receipt_complete() {
	_prefix=$1
	[ -n "$RECEIPT_PARTIAL" ] && [ -f "$RECEIPT_PARTIAL" ] && [ ! -L "$RECEIPT_PARTIAL" ] \
		|| return 1
	_complete_receipt=$(mktemp "$_prefix/.install-receipt-complete-XXXXXX") || return 1
	if ! sed 's/^setup_status=pending$/setup_status=complete/' \
		"$RECEIPT_PARTIAL" >"$_complete_receipt" \
		|| ! grep -Fqx 'setup_status=complete' "$_complete_receipt"; then
		rm -f -- "$_complete_receipt"
		return 1
	fi
	if ! mv -f "$_complete_receipt" "$RECEIPT_PARTIAL"; then
		rm -f -- "$_complete_receipt"
		return 1
	fi
	return 0
}

publish_receipt() {
	_prefix=$1
	[ -n "$RECEIPT_PARTIAL" ] && [ -f "$RECEIPT_PARTIAL" ] && [ ! -L "$RECEIPT_PARTIAL" ] \
		|| refuse receipt-invalid "staged receipt is unavailable"
	mv -f "$RECEIPT_PARTIAL" "$_prefix/install-receipt" \
		|| refuse receipt-invalid "could not publish receipt"
	RECEIPT_PARTIAL=
}

main() {
	parse_args "$@"
	detect_target
	trap cleanup 0
	trap 'handle_signal 129' 1
	trap 'handle_signal 130' 2
	trap 'handle_signal 143' 15
	acquire_route_lock
	detect_existing_route
	if [ "$PRUNE" -eq 1 ]; then
		[ -z "$ARCHIVE" ] && [ -z "$SHA256_FILE" ] && [ -z "$RELEASE_FILE" ] \
			&& [ -z "$MANIFEST_FILE" ] && [ -z "$MINISIG_FILE" ] && [ -z "$VERSION" ] \
			&& [ "$UPGRADE" -eq 0 ] && [ "$LANE_EXPLICIT" -eq 0 ] \
			&& [ "$ORIGIN_EXPLICIT" -eq 0 ] && [ "$SKIP_SIGNATURE" -eq 0 ] \
			&& [ "$NO_PATH" -eq 0 ] \
			|| refuse prune-unsafe "--prune must be used by itself (plus --prefix if needed)"
		prune_versions
		exit 0
	fi

TMP_ROOT=${TMPDIR:-/var/tmp}
if [ ! -d "$TMP_ROOT" ] || [ ! -w "$TMP_ROOT" ]; then
	refuse tmpdir-unusable "$TMP_ROOT"
fi
WORK=$(mktemp -d "$TMP_ROOT/solstone-install-work-XXXXXX") || refuse tmpdir-unusable "$TMP_ROOT"

if [ -n "$ARCHIVE" ]; then
	[ -f "$ARCHIVE" ] || refuse digest-mismatch "archive missing"
	[ -n "$SHA256_FILE" ] || refuse digest-mismatch "sha256 sidecar missing"
	[ -n "$RELEASE_FILE" ] || refuse release-invalid "release sidecar missing"
	cp "$ARCHIVE" "$WORK/tree.tar.gz"
	cp "$SHA256_FILE" "$WORK/tree.sha256"
	cp "$RELEASE_FILE" "$WORK/tree.release"
	_archive_name=${ARCHIVE##*/}
	_sha_name=${SHA256_FILE##*/}
	_release_name=${RELEASE_FILE##*/}
	if [ "$SKIP_SIGNATURE" -eq 0 ]; then
		[ -n "$MANIFEST_FILE" ] || refuse signature-invalid "manifest sidecar missing"
		[ -n "$MINISIG_FILE" ] || refuse signature-invalid "minisign sidecar missing"
		cp "$MANIFEST_FILE" "$WORK/tree.manifest.json"
		cp "$MINISIG_FILE" "$WORK/tree.manifest.json.minisig"
	fi
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
	_sha_name=${_base}.sha256
	_release_name=${_base}.release
	if [ "$SKIP_SIGNATURE" -eq 0 ]; then
		fetch_url "${_object_base}/${_base}.manifest.json" "$WORK/tree.manifest.json"
		fetch_url "${_object_base}/${_base}.manifest.json.minisig" "$WORK/tree.manifest.json.minisig"
	fi
fi

if [ "$SKIP_SIGNATURE" -eq 0 ]; then
	verify_signed_release_set \
		"$WORK/tree.manifest.json" \
		"$WORK/tree.manifest.json.minisig" \
		"$_archive_name" \
		"$_sha_name" \
		"$_release_name"
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

if [ -e "$DEST" ] || [ -L "$DEST" ]; then
	validate_installed_destination "$DEST" "$ACTUAL" "$RELEASE_TEXT"
fi

DOWNGRADE=0
if [ "$ROUTE" = tree ] && [ "$VERSION" != "$INSTALLED_VERSION" ]; then
	if version_is_older "$VERSION" "$INSTALLED_VERSION"; then
		DOWNGRADE=1
	else
		_version_order=$?
		[ "$_version_order" -eq 1 ] \
			|| refuse version-order-unknown "cannot compare ${VERSION} with ${INSTALLED_VERSION}; reinstall the current version"
	fi
fi
if [ "$DOWNGRADE" -eq 1 ]; then
	[ "$INSTALLED_EPOCH" = "$SUPPORTED_UPGRADE_EPOCH" ] \
		&& [ "$RELEASE_EPOCH" = "$INSTALLED_EPOCH" ] \
		|| refuse downgrade-epoch "install the current version or another ${SUPPORTED_UPGRADE_EPOCH} release"
	[ "$INSTALLED_RETENTION_WINDOW" = "$SUPPORTED_RETENTION_WINDOW" ] \
		&& [ "$RELEASE_RETENTION_WINDOW" = "$SUPPORTED_RETENTION_WINDOW" ] \
		|| refuse retention-policy-unsupported "install the current version"
	[ -d "$DEST" ] && [ ! -L "$DEST" ] \
		|| refuse downgrade-window "that build is not retained; reinstall the current version"
	within_retention_window "$DEST" "$INSTALLED_DEST" "$SUPPORTED_RETENTION_WINDOW" \
		|| refuse downgrade-window "that build is outside the three-directory window; reinstall the current version"
fi

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
		stage_receipt "$PREFIX"
		SETUP_TRANSACTION_ACTIVE=1
		if run_setup "$DEST"; then
			:
		else
			_setup_status=$?
			case $_setup_status in
			1 | 2)
				SETUP_TRANSACTION_ACTIVE=0
				;;
			*)
				publish_receipt "$PREFIX"
				SETUP_TRANSACTION_ACTIVE=0
				;;
			esac
			refuse setup-failed "rerun this same install.sh command; setup status was ${_setup_status}"
		fi
		if ! mark_receipt_complete "$PREFIX"; then
			publish_receipt "$PREFIX"
			SETUP_TRANSACTION_ACTIVE=0
			refuse receipt-invalid "setup completed but its receipt remains pending; rerun this same install.sh command"
		fi
		publish_receipt "$PREFIX"
		SETUP_TRANSACTION_ACTIVE=0
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
	PARTIAL=
	refuse release-invalid "extract failed"
fi
printf '%s\n' "$RELEASE_TEXT" >"$PARTIAL/.release"
printf '%s\n' "$ACTUAL" >"$PARTIAL/.archive-sha256"

if [ -e "$DEST" ]; then
	rm -rf "$PARTIAL"
else
	DEST_OWNER_FILE=$DEST/.install-transaction-${ROUTE_LOCK_TOKEN}
	printf '%s\n' "$ROUTE_LOCK_TOKEN" >"$PARTIAL/.install-transaction-${ROUTE_LOCK_TOKEN}"
	_partial_name=${PARTIAL##*/}
	DEST_NESTED_PARTIAL=$DEST/$_partial_name
	DEST_NESTED_MARKER=$DEST_NESTED_PARTIAL/.install-transaction-${ROUTE_LOCK_TOKEN}
	DEST_MOVE_PENDING=1
	if ! mv "$PARTIAL" "$DEST"; then
		if destination_is_transaction_owned; then
			DEST_CREATED=1
		elif path_is_transaction_owned "$DEST_NESTED_PARTIAL" "$DEST_NESTED_MARKER"; then
			rm -rf -- "$DEST_NESTED_PARTIAL"
		fi
		DEST_MOVE_PENDING=0
		refuse route-busy "the selected version path changed during installation; retry"
	fi
	if ! destination_is_transaction_owned; then
		if path_is_transaction_owned "$DEST_NESTED_PARTIAL" "$DEST_NESTED_MARKER"; then
			rm -rf -- "$DEST_NESTED_PARTIAL"
		fi
		PARTIAL=
		DEST_MOVE_PENDING=0
		refuse route-busy "the selected version path changed during installation; leave it untouched and retry"
	fi
	DEST_CREATED=1
	DEST_MOVE_PENDING=0
fi
PARTIAL=

stage_receipt "$PREFIX"
OLD_CURRENT=
if [ -L "$CURRENT" ]; then
	OLD_CURRENT=$(readlink "$CURRENT")
fi
SETUP_TRANSACTION_ACTIVE=1
if ! flip_current "$PREFIX" "$DEST"; then
	if ! restore_previous_current; then
		refuse_after_rollback_failure "the current flip failed"
	fi
	SETUP_TRANSACTION_ACTIVE=0
	refuse release-invalid "current flip failed"
fi
if run_setup "$DEST"; then
	:
else
	_setup_status=$?
	case $_setup_status in
	1 | 2)
		if ! restore_previous_current; then
			refuse_after_rollback_failure "setup refused before mutation"
		fi
		SETUP_TRANSACTION_ACTIVE=0
		refuse setup-failed "setup refused before mutation and the previous current target was restored; correct the refusal, then rerun this same install.sh command"
		;;
	esac
	publish_receipt "$PREFIX"
	SETUP_TRANSACTION_ACTIVE=0
	refuse setup-failed "current remains on the candidate and its receipt marks setup pending; rerun this same install.sh command"
fi
if ! mark_receipt_complete "$PREFIX"; then
	publish_receipt "$PREFIX"
	SETUP_TRANSACTION_ACTIVE=0
	refuse receipt-invalid "setup completed but its receipt remains pending; rerun this same install.sh command"
fi
publish_receipt "$PREFIX"
SETUP_TRANSACTION_ACTIVE=0
if [ "$NO_PATH" -eq 0 ]; then
	write_profile "$PREFIX"
fi
report_success "$PREFIX"
exit 0
}

main "$@"
