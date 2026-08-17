#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Live macOS distribution oracle. The sibling of cleanroom.sh, and deliberately
# not a port of it: there is no container to preload by digest, so the SUBJECT
# IS THE HOST and the roles run directly on whichever Mac you point this at.
#
# Two hosts, two answers, and both are required:
#   a Mac with no interpreter present -> `scan` must find nothing
#   a Mac with Python present         -> `scan` must find it
# A zero from the first is a claim about this script before it is a claim about
# the host, so it means nothing until the second one has fired.
#
# 🔴 THE GATEKEEPER OBLIGATION lives in the `gatekeeper` role, and it has two
# halves that a single check cannot cover:
#   1. every signed native EXECUTABLE starts under Gatekeeper, quarantined
#   2. every shipped LOADED PAYLOAD actually loads into one of them
# Those are disjoint sets. The retired Python `warm` verb held the second one
# for site-packages; nothing else does now, so this role does.
#
# ROLES:
#   scan        python census (zero on a clean Mac, positive on the control)
#   tar         extract the tarball, prove the launchers and the journal loop
#   pkg         install the signed package, prove staple + spctl
#   bootstrap   install.sh end to end, prove a FRESH LOGIN SHELL finds journal
#   gatekeeper  both halves above, each with a negative control
#   talent      a talent runs from the extracted tree
#   speakers    the real speaker models run from the extracted tree

set -eu

PRODUCT=solstone-journal
SIGNER="Developer ID Application: sol pbc (7QCG8V4M6H)"
TEAM_ID=7QCG8V4M6H

refuse() {
	printf 'macos-rung: %s\n' "$*" >&2
	exit 2
}

note() {
	printf 'macos-rung: %s\n' "$*" >&2
}

ROOT=$(CDPATH='' cd -- "$(dirname "$0")/../.." && pwd)
SCAN_SH=$ROOT/core/distribution/scan-python.sh
INSTALL_SH=$ROOT/core/distribution/install.sh
ARTIFACTS=${SOLSTONE_MACOS_ARTIFACTS:-/var/tmp/solstone-distribution-out/macos-arm64}
WORK=${SOLSTONE_MACOS_WORK:-/var/tmp/solstone-macos-rung}
TREE=$WORK/tree
JOURNAL=$WORK/journal

one_artifact() {
	suffix=$1
	set -- "$ARTIFACTS"/*"$suffix"
	if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
		refuse "expected exactly one *${suffix} artifact under $ARTIFACTS"
	fi
	printf '%s\n' "$1"
}

reset_work() {
	rm -rf "$WORK"
	mkdir -p "$WORK"
}

# --- python census ----------------------------------------------------------

scan_zero() {
	matches=$(sh "$SCAN_SH" / || true)
	[ -z "$matches" ] || {
		printf '%s\n' "$matches" >&2
		refuse "Python runtime found on a host declared interpreter-free"
	}
	printf 'scan=zero ok\n'
}

scan_control() {
	matches=$(sh "$SCAN_SH" / || true)
	printf '%s\n' "$matches"
	[ -n "$matches" ] || refuse "Python control produced no findings: the census cannot see a positive, so its zero elsewhere means nothing"
	printf '%s\n' "$matches" | grep -E '^executable ' >/dev/null \
		|| refuse "Python control found no executable interpreter"
	printf 'scan=control ok\n'
}

# --- installation -----------------------------------------------------------

install_tar() {
	archive=$(one_artifact .tar.gz)
	mkdir -p "$TREE"
	tar -xzf "$archive" -C "$TREE"
	PATH=$TREE/bin:/usr/bin:/bin:/usr/sbin:/sbin
	export PATH
}

assert_launchers() {
	for launcher in journal sol solstone; do
		path=$(command -v "$launcher") || refuse "launcher missing: $launcher"
		case $path in
		*site-packages* | *.venv/* | *'/venv/'* | *Python.framework*)
			refuse "launcher resolved through Python layout: $path"
			;;
		esac
	done
	printf 'launchers ok\n'
}

# --- Gatekeeper -------------------------------------------------------------

# Every Mach-O in the tree, found by magic rather than by name. The executables
# carry no extension and the payload arrives under two different dylib names, so
# a name-keyed census is structurally unable to enumerate this tree.
macho_members() {
	find "$TREE" -type f -perm -u+r -print | while IFS= read -r path; do
		magic=$(LC_ALL=C od -An -N4 -t x1 "$path" 2>/dev/null | tr -d ' \n')
		case $magic in
		cffaedfe | cafebabe | cafebabf) printf '%s\n' "$path" ;;
		esac
	done | LC_ALL=C sort
}

macho_executables() {
	macho_members | while IFS= read -r path; do
		[ -x "$path" ] && printf '%s\n' "$path"
	done
}

macho_payloads() {
	macho_members | while IFS= read -r path; do
		case ${path##*/} in
		*.dylib | *.so | *.so.*) printf '%s\n' "$path" ;;
		esac
	done
}

assert_signed_by_us() {
	path=$1
	report=$(/usr/bin/codesign -dv --verbose=4 "$path" 2>&1) \
		|| refuse "codesign could not read $path"
	printf '%s\n' "$report" | grep -Fq "Authority=$SIGNER" \
		|| refuse "not signed by us: $path"
	printf '%s\n' "$report" | grep -Fq "TeamIdentifier=$TEAM_ID" \
		|| refuse "wrong or absent team identifier: $path"
	printf '%s\n' "$report" | grep -Fq '(runtime)' \
		|| refuse "hardened runtime absent: $path"
	printf '%s\n' "$report" | grep -q '^Timestamp=' \
		|| refuse "trusted timestamp absent: $path"
}

quarantine_tree() {
	xattr -w -r com.apple.quarantine '0081;00000000;solstone-macos-rung;' "$TREE"
	# Prove the mark actually landed. curl and tar do not set it, so a rung
	# that skips this step evaluates an UNQUARANTINED tree and passes
	# identically on binaries Gatekeeper would have rejected.
	marked=$(xattr -p com.apple.quarantine "$TREE/bin/solstone-core" 2>/dev/null || true)
	[ -n "$marked" ] || refuse "quarantine attribute did not land; the Gatekeeper rung would be measuring nothing"
	printf 'quarantine ok (%s)\n' "$marked"
}

gatekeeper_rung() {
	install_tar
	quarantine_tree

	count=0
	for path in $(macho_executables); do
		count=$((count + 1))
		assert_signed_by_us "$path"
		spctl -a -vvv -t exec "$path" >"$WORK/spctl.out" 2>&1 \
			|| { cat "$WORK/spctl.out" >&2; refuse "Gatekeeper rejected $path"; }
		grep -Fq 'accepted' "$WORK/spctl.out" \
			|| { cat "$WORK/spctl.out" >&2; refuse "spctl did not accept $path"; }
		grep -Fq 'Notarized Developer ID' "$WORK/spctl.out" \
			|| { cat "$WORK/spctl.out" >&2; refuse "$path is signed but NOT notarized"; }
		# Signed and assessed is still not started. Run it.
		"$path" --version >/dev/null 2>&1 \
			|| "$path" --help >/dev/null 2>&1 \
			|| refuse "signed, notarized and would not start: $path"
	done
	[ "$count" -ge 8 ] || refuse "expected at least 8 executables in the tree, found $count"
	printf 'gatekeeper half 1: %s executables signed, notarized, accepted and started\n' "$count"

	payloads=0
	for path in $(macho_payloads); do
		payloads=$((payloads + 1))
		assert_signed_by_us "$path"
	done
	[ "$payloads" -ge 1 ] || refuse "no loaded payload found in the tree: half 2 has nothing to test"

	# Half 2 is not "the dylib is signed" — it is "the dylib LOADS". Under the
	# hardened runtime a binary refuses a library signed by another team, so
	# the only proof is a run that actually links it.
	speakers_probe "$WORK/gk-response.json" \
		|| refuse "the signed payload did not load into the signed helper"
	grep -Fq '"schema":"solstone-speaker-analyze-response-v1"' "$WORK/gk-response.json" \
		|| refuse "payload load produced no response"
	printf 'gatekeeper half 2: %s payload(s) signed and loaded by a hardened-runtime binary\n' "$payloads"

	gatekeeper_negatives
	printf 'rung=gatekeeper ok\n'
}

# The controls. Each removes exactly the property the corresponding half
# asserts and requires the same check to go red. A half that still passes here
# was measuring the harness.
gatekeeper_negatives() {
	broken=$WORK/broken
	rm -rf "$broken"
	cp -R "$TREE" "$broken"
	xattr -w -r com.apple.quarantine '0081;00000000;solstone-macos-rung;' "$broken"

	# Control for half 1: an ad-hoc signature is a VALID signature, which is
	# why `codesign --verify` cannot be the test. Gatekeeper must still refuse.
	adhoc=$broken/bin/solstone-core
	/usr/bin/codesign --force --sign - "$adhoc" >/dev/null 2>&1 \
		|| refuse "could not build the ad-hoc control"
	/usr/bin/codesign --verify --strict "$adhoc" >/dev/null 2>&1 \
		|| note "ad-hoc control did not even verify; the control is weaker than intended"
	if spctl -a -vvv -t exec "$adhoc" >/dev/null 2>&1; then
		refuse "CONTROL FAILED: Gatekeeper accepted an ad-hoc signed binary, so half 1 proves nothing"
	fi
	printf 'control 1 ok: Gatekeeper refuses an ad-hoc signed binary\n'

	# Control for half 2: strip the payload's signature and require the
	# hardened-runtime helper to refuse to start. This is the exact failure a
	# binaries-only signing census ships.
	for path in $(SOLSTONE_MACOS_TREE=$broken macho_members_in "$broken"); do
		case ${path##*/} in
		*.dylib) /usr/bin/codesign --remove-signature "$path" >/dev/null 2>&1 || true ;;
		esac
	done
	if TREE=$broken speakers_probe "$WORK/broken-response.json" 2>"$WORK/broken.err"; then
		refuse "CONTROL FAILED: the helper ran with an unsigned payload, so half 2 proves nothing"
	fi
	printf 'control 2 ok: a hardened-runtime binary refuses an unsigned payload\n'
	rm -rf "$broken"
}

macho_members_in() {
	root=$1
	find "$root" -type f -print | while IFS= read -r path; do
		magic=$(LC_ALL=C od -An -N4 -t x1 "$path" 2>/dev/null | tr -d ' \n')
		case $magic in
		cffaedfe | cafebabe | cafebabf) printf '%s\n' "$path" ;;
		esac
	done | LC_ALL=C sort
}

# --- speakers ---------------------------------------------------------------

speaker_request() {
	tree=$1
	output=$2
	interval=$3
	cat <<EOF
{"schema":"solstone-speaker-analyze-request-v1","sample_rate_hz":16000,"full_audio_f32le_path":"$WORK/audio.f32","reduced_audio_f32le_path":null,"models":{"pyannote_segmentation_onnx_path":"$tree/lib/solstone_journal_models/assets/pyannote-segmentation-3.0.onnx","wespeaker_onnx_path":"$tree/lib/solstone_journal_models/assets/wespeaker-resnet34-256.onnx"},"output_payload_f32le_path":"$output","interval_embedding_payload_f32le_path":"$interval","statement_embedding":{"spans":[{"statement_id":1,"start_s":0.0,"end_s":0.5}]},"diarization":{"spans":[{"statement_id":1,"start_s":0.0,"end_s":0.5}]}}
EOF
}

speakers_probe() {
	response=$1
	tree=${TREE}
	[ -f "$WORK/audio.f32" ] || dd if=/dev/zero of="$WORK/audio.f32" bs=32000 count=1 2>/dev/null
	speaker_request "$tree" "$WORK/statements.f32" "$WORK/intervals.f32" \
		| "$tree/bin/solstone-core-speakers-analyze" >"$response" 2>"$WORK/speakers.err"
}

speakers_rung() {
	install_tar
	rm -f "$WORK/statements.f32" "$WORK/intervals.f32"
	speakers_probe "$WORK/response.json" || {
		cat "$WORK/speakers.err" >&2
		refuse "speakers rung did not run"
	}
	grep -Fq '"schema":"solstone-speaker-analyze-response-v1"' "$WORK/response.json" \
		|| refuse "speakers response schema mismatch"
	grep -Fq '"shape":[1,256]' "$WORK/response.json" \
		|| refuse "speaker embedding shape mismatch"
	grep -Fq '"byte_count":1024' "$WORK/response.json" \
		|| refuse "speaker embedding byte count mismatch"
	[ "$(wc -c <"$WORK/statements.f32" | tr -d ' ')" -eq 1024 ] \
		|| refuse "speaker embedding payload length mismatch"
	if LC_ALL=C od -An -t f4 "$WORK/statements.f32" | grep -E -i 'nan|inf' >/dev/null; then
		refuse "speaker embedding payload contains a non-finite value"
	fi

	# Each model and the runtime removed in turn: the response above must
	# depend on all three, or it was reading something else.
	for asset in \
		"$TREE/lib/solstone_journal_models/assets/pyannote-segmentation-3.0.onnx" \
		"$TREE/lib/solstone_journal_models/assets/wespeaker-resnet34-256.onnx" \
		"$TREE/lib/solstone-core-speakers-analyze/libonnxruntime.1.25.0.dylib"; do
		[ -f "$asset" ] || refuse "expected tree member missing: $asset"
		mv "$asset" "$asset.missing"
		if speakers_probe "$WORK/missing.json" 2>"$WORK/missing.err"; then
			mv "$asset.missing" "$asset"
			refuse "speakers rung survived a missing $(basename "$asset")"
		fi
		mv "$asset.missing" "$asset"
	done
	printf 'rung=speakers ok\n'
}

# --- talent -----------------------------------------------------------------

talent_rung() {
	install_tar
	assert_launchers
	rm -rf "$JOURNAL"
	day=$(date +%Y%m%d)
	segment=$JOURNAL/chronicle/$day/default/031700_697
	target_segment=$JOURNAL/chronicle/20990101/default/120000_001
	mkdir -p "$JOURNAL/config" "$segment" "$target_segment/talents"
	printf '%s\n' '{"stream":"default","seq":1,"prev_segment":null}' >"$segment/stream.json"
	printf '%s\n' '{"stream":"default","seq":1,"prev_segment":null}' >"$target_segment/stream.json"
	printf '%s\n' '# Audio Transcript' '' \
		'No future scheduled activity appears in this synthetic macOS rung segment.' \
		>"$target_segment/talents/audio.md"
	printf '%s\n' \
		'{"start":"00:00:00","source":"mic","speaker":1,"text":"No future scheduled activity appears in this synthetic macOS rung segment."}' \
		>"$target_segment/capture_audio.jsonl"
	weekday=$(date +%A)
	printf '%s (%s):\n  03:17 - 03:28 (11m)' "$day" "$weekday" >"$JOURNAL/expected-fragment"
	: >"$JOURNAL/generation-evidence"
	: >"$JOURNAL/generation.addr"
	"$PRODUCER" cleanroom-generate-serve "$JOURNAL/generation-evidence" "$JOURNAL/expected-fragment" \
		>"$JOURNAL/generation.addr" 2>"$JOURNAL/generation.err" &
	SERVER_PID=$!
	attempt=0
	while [ ! -s "$JOURNAL/generation.addr" ]; do
		attempt=$((attempt + 1))
		if ! kill -0 "$SERVER_PID" 2>/dev/null || [ "$attempt" -ge 30 ]; then
			cat "$JOURNAL/generation.err" >&2 || true
			refuse "generation fixture did not become ready"
		fi
		sleep 1
	done
	endpoint=http://$(cat "$JOURNAL/generation.addr")
	printf '%s\n' "{\"setup\":{\"completed_at\":1},\"providers\":{\"active\":{\"provider\":\"local\"},\"local\":{\"endpoint_url\":\"$endpoint\",\"served_model_id\":\"macos-rung\"}}}" \
		>"$JOURNAL/config/journal.json"
	export SOLSTONE_JOURNAL=$JOURNAL
	unset SOL_SKIP_SUPERVISOR_CHECK || true
	[ ! -e "$JOURNAL/chronicle/20990101/talents/daily_schedule.json" ] \
		|| refuse "daily_schedule output was pre-seeded"
	solstone-core supervisor --journal "$JOURNAL" --no-spl --no-daily 5015 \
		>"$JOURNAL/supervisor.log" 2>&1 &
	SUPERVISOR_PID=$!
	attempt=0
	while [ ! -S "$JOURNAL/health/callosum.sock" ]; do
		attempt=$((attempt + 1))
		if ! kill -0 "$SUPERVISOR_PID" 2>/dev/null || [ "$attempt" -ge 30 ]; then
			cat "$JOURNAL/supervisor.log" >&2 || true
			refuse "supervisor did not become ready"
		fi
		sleep 1
	done
	if ! journal think --day 20990101 --refresh >"$JOURNAL/think.out" 2>"$JOURNAL/think.err"; then
		cat "$JOURNAL/think.out" "$JOURNAL/think.err" "$JOURNAL/supervisor.log" \
			"$JOURNAL/generation.err" >&2 || true
		kill "$SUPERVISOR_PID" 2>/dev/null || true
		kill "$SERVER_PID" 2>/dev/null || true
		refuse "journal think failed from the extracted tree"
	fi
	kill "$SUPERVISOR_PID" 2>/dev/null || true
	kill "$SERVER_PID" 2>/dev/null || true
	wait "$SUPERVISOR_PID" 2>/dev/null || true
	wait "$SERVER_PID" 2>/dev/null || true
	[ -f "$JOURNAL/chronicle/20990101/talents/daily_schedule.json" ] \
		|| refuse "daily_schedule output missing"
	daily=$(tr -d '[:space:]' <"$JOURNAL/chronicle/20990101/talents/daily_schedule.json")
	[ "$daily" = '{"primary":"03:00","fallback":"04:00"}' ] \
		|| refuse "daily_schedule output mismatch: $daily"
	set -- "$JOURNAL"/talents/daily_schedule/*.jsonl
	if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
		refuse "daily_schedule terminal event log missing"
	fi
	grep -Fq '"event":"finish"' "$1" \
		|| refuse "daily_schedule did not reach a terminal finish event"
	printf 'rung=talent ok\n'
}

# --- pkg + bootstrap --------------------------------------------------------

pkg_rung() {
	package=$(one_artifact .pkg)
	# A stapled ticket is what makes the package installable with no network.
	# `staple validate` reads the ticket back off the file, which `staple` on
	# its own does not prove after a copy.
	xcrun stapler validate "$package" >"$WORK/staple.out" 2>&1 \
		|| { cat "$WORK/staple.out" >&2; refuse "package carries no stapled ticket"; }
	spctl -a -vvv -t install "$package" >"$WORK/spctl-pkg.out" 2>&1 \
		|| { cat "$WORK/spctl-pkg.out" >&2; refuse "Gatekeeper rejected the package"; }
	grep -Fq 'accepted' "$WORK/spctl-pkg.out" || refuse "spctl did not accept the package"
	grep -Fq 'Notarized Developer ID' "$WORK/spctl-pkg.out" \
		|| refuse "the package is signed but NOT notarized"
	pkgutil --check-signature "$package" >"$WORK/pkgsig.out" 2>&1 \
		|| { cat "$WORK/pkgsig.out" >&2; refuse "pkgutil refused the package signature"; }
	grep -Fq 'Developer ID Installer: sol pbc' "$WORK/pkgsig.out" \
		|| refuse "the package is not signed with the Developer ID Installer identity"
	printf 'rung=pkg ok\n'
}

bootstrap_rung() {
	archive=$(one_artifact .tar.gz)
	sha=$(one_artifact .sha256)
	release=$(one_artifact .release)
	prefix=$WORK/prefix
	home=$WORK/home
	rm -rf "$prefix" "$home"
	mkdir -p "$prefix" "$home"
	HOME=$home sh "$INSTALL_SH" --prefix "$prefix" \
		--archive "$archive" --sha256 "$sha" --release "$release" \
		|| refuse "bootstrap install failed"
	[ -L "$prefix/current" ] || refuse "bootstrap did not flip current"

	# A fresh LOGIN shell, in both shells a Mac actually gives people. zsh is
	# the macOS default and never reads .profile, so proving only `sh -l` here
	# would certify a PATH no owner has.
	for shell in /bin/sh /bin/zsh; do
		[ -x "$shell" ] || continue
		resolved=$(env -i HOME="$home" PATH=/usr/bin:/bin TERM=dumb \
			"$shell" -l -c 'command -v journal; command -v sol; command -v solstone' 2>/dev/null || true)
		lines=$(printf '%s\n' "$resolved" | grep -c . || true)
		[ "$lines" -eq 3 ] \
			|| refuse "fresh $shell login shell resolved $lines of 3 launchers"
	done
	printf 'rung=bootstrap ok\n'
}

# --- entry ------------------------------------------------------------------

PRODUCER=${SOLSTONE_DISTRIBUTION_BIN:-$ROOT/core/target/release/solstone-distribution}

role=${1:-}
case $role in
scan) reset_work; scan_zero ;;
scan-control) reset_work; scan_control ;;
tar) reset_work; install_tar; assert_launchers ;;
pkg) reset_work; pkg_rung ;;
bootstrap) reset_work; bootstrap_rung ;;
gatekeeper) reset_work; gatekeeper_rung ;;
talent) reset_work; talent_rung ;;
speakers) reset_work; speakers_rung ;;
*)
	printf 'usage: macos.sh <scan|scan-control|tar|pkg|bootstrap|gatekeeper|talent|speakers>\n' >&2
	exit 2
	;;
esac
