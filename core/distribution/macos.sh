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
# 🔴 A FRESH work root per run, and this is not tidiness — a reused path can be
# permanently poisoned. Measured 2026-08-17: one execution of a QUARANTINED
# binary in a headless session blocks forever waiting on a first-launch
# assessment that wants a GUI, and it leaves a stuck syspolicy record keyed to
# that PATH. `rm -rf` and a clean re-extract do NOT clear it: the same bytes
# from the same tarball timed out at 15s under the poisoned path and returned in
# 1s at `…/tree2` and at a different root, in one script, back to back.
# ⚠ A rung pinned to a fixed path therefore reports a healthy artifact as hung,
# for as long as that host lives.
WORK=${SOLSTONE_MACOS_WORK:-$(mktemp -d /var/tmp/solstone-macos-rung.XXXXXX)}
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
	printf 'work root: %s\n' "$WORK"
}

# --- python census ----------------------------------------------------------

# 🔴 A bare zero is NOT achievable on any Mac and must not be the criterion.
# `/usr/bin/python3` is present on every macOS install as a Command Line Tools
# shim — a real, executable file — so a census that keys on name and mode
# reports a Python runtime on a host that has never had one. The census
# classifies a candidate that cannot `import sys` as `shim`, and the zero this
# rung asserts is over the REAL interpreters.
#
# ✅ Shims are printed rather than dropped: an unreported exclusion is how a
# criterion quietly stops covering the thing it names.
# Every location an interpreter could actually be installed. A walk that did
# not reach ALL of these cannot certify an absence, whatever else it reached.
#
# ⚠ Some incompleteness is unavoidable on a real Mac — system caches and
# DirectoryServices are unreadable even to an admin — so a blanket refusal on
# any unreached path would make the zero unachievable and the criterion
# decorative. The rule is therefore specific: disclose everything unreached, and
# REFUSE if an unreached path is an ancestor of somewhere an interpreter lives.
CRITICAL_ROOTS='/usr/bin /usr/local /opt /Library/Frameworks /Library/Developer /Applications /Users /System/Library/Frameworks'

scan_zero() {
	matches=$(sh "$SCAN_SH" / || true)
	incomplete=$(printf '%s\n' "$matches" | grep '^scan-incomplete ' || true)
	blind=
	if [ -n "$incomplete" ]; then
		printf 'disclosed unreached paths:\n%s\n' "$incomplete"
		for critical in $CRITICAL_ROOTS; do
			printf '%s\n' "$incomplete" | while IFS= read -r line; do
				path=${line#scan-incomplete }
				[ -n "$path" ] || continue
				case "$critical/" in
				"$path"/*) printf '%s covers %s\n' "$path" "$critical" ;;
				esac
			done
		done >"$WORK/blind.out"
		blind=$(cat "$WORK/blind.out" 2>/dev/null || true)
	fi
	[ -z "$blind" ] || {
		printf '%s\n' "$blind" >&2
		refuse "the census never reached a location an interpreter lives in; its zero is a claim about the walk, not about the host"
	}
	shims=$(printf '%s\n' "$matches" | grep '^shim ' || true)
	real=$(printf '%s\n' "$matches" | grep -vE '^(shim|scan-incomplete) ' | grep . || true)
	[ -z "$shims" ] || {
		printf 'disclosed non-findings (present, not an interpreter):\n%s\n' "$shims"
	}
	[ -z "$real" ] || {
		printf '%s\n' "$real" >&2
		refuse "Python runtime found on a host declared interpreter-free"
	}
	printf 'scan=zero ok (0 interpreters, %s shim(s) disclosed)\n' \
		"$(printf '%s\n' "$shims" | grep -c . || true)"
}

scan_control() {
	matches=$(sh "$SCAN_SH" / || true)
	printf '%s\n' "$matches"
	[ -n "$matches" ] || refuse "Python control produced no findings: the census cannot see a positive, so its zero elsewhere means nothing"
	printf '%s\n' "$matches" | grep -E '^executable ' >/dev/null \
		|| refuse "Python control found no RUNNING interpreter; a census that only sees shims cannot certify an absence"
	printf 'scan=control ok (%s interpreter(s), %s shim(s), %s unwalked root(s))\n' \
		"$(printf '%s\n' "$matches" | grep -c '^executable ' || true)" \
		"$(printf '%s\n' "$matches" | grep -c '^shim ' || true)" \
		"$(printf '%s\n' "$matches" | grep -c '^scan-incomplete ' || true)"
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
	for launcher in journal solstone; do
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

# ⛔ Split executables from payloads by Mach-O FILETYPE, never by the +x bit or
# by extension. Both shipped dylibs are staged mode 0755, so an `-x` test counts
# them as executables (measured: 10 where the inventory admits 8) and an
# extension test misses any payload that does not end in `.dylib`. The filetype
# is a property of the bytes and it is the same discriminator the producer's
# Rust census uses, so the two agree by construction rather than by convention.
macho_filetype() {
	LC_ALL=C od -An -j12 -N4 -t u4 "$1" 2>/dev/null | tr -d ' \n'
}

macho_executables() {
	macho_members | while IFS= read -r path; do
		[ "$(macho_filetype "$path")" = "2" ] && printf '%s\n' "$path"
	done
}

macho_payloads() {
	macho_members | while IFS= read -r path; do
		[ "$(macho_filetype "$path")" = "6" ] && printf '%s\n' "$path"
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

# Notarization, asserted two independent ways.
#
# ⛔ `spctl -t exec` is the WRONG instrument here and its rejection is not about
# our signature: for a bare CLI Mach-O it answers *"the code is valid but does
# not seem to be an app"* while printing our own `origin=`. `-t open` with the
# primary-signature context is the assessment that applies to a plain file.
# `codesign -R="notarized"` is the second, and it names the property directly
# rather than inferring it from an assessment verdict.
# 🔴 The instrument self-check, and it is not optional.
#
# `spctl` returns "accepted" for EVERYTHING when assessments are turned off, so
# every Gatekeeper verdict on such a host is vacuous — including the ad-hoc
# control, which is the only thing making the rung falsifiable. Measured
# 2026-08-17: a stock CI macOS VM image ships `assessments disabled`, and on it
# the ad-hoc control passed while the build host rejected the identical file.
# ⛔ Never read an `spctl` green without having read this first.
assert_gatekeeper_enabled() {
	status=$(spctl --status 2>&1 || true)
	case $status in
	*enabled*) printf 'gatekeeper assessments: enabled\n' ;;
	*)
		printf '%s\n' "$status" >&2
		refuse "Gatekeeper assessments are not enabled on this host; every spctl verdict here is vacuous and the ad-hoc control cannot fail"
		;;
	esac
}

assert_notarized() {
	path=$1
	out=$(spctl -a -vvv -t open --context context:primary-signature "$path" 2>&1) || {
		printf '%s\n' "$out" >&2
		refuse "Gatekeeper did not accept $path"
	}
	printf '%s\n' "$out" | grep -Fq 'Notarized Developer ID' \
		|| { printf '%s\n' "$out" >&2; refuse "$path is signed but NOT notarized"; }
	/usr/bin/codesign -vvv -R='notarized' --check-notarization "$path" >"$WORK/notarized.out" 2>&1 \
		|| { cat "$WORK/notarized.out" >&2; refuse "$path failed the notarized code requirement"; }
}

# ⛔ `timeout` is NOT a base-macOS tool — it arrives with GNU coreutils, so a
# rung that uses it passes on a developer's Mac and dies on a clean one with
# `command not found`. Bound the run by hand instead.
#
# ⚠ And stdin must be closed: three of the nine binaries speak a JSON request
# protocol on stdin, so an unbounded probe with an open stdin hangs forever and
# reads as a wedge rather than as a passing binary.
# ⛔ Do NOT poll `kill -0 "$pid"` to decide whether a background child is still
# running. A child that has already exited stays a ZOMBIE until it is reaped, and
# `kill -0` on a zombie SUCCEEDS — so the loop runs to its limit and reports a
# timeout on a command that finished in one second. That is exactly what this
# rung did on its first run: it declared nine signed, notarized binaries hung
# while each of them was returning promptly, which reads as a product failure and
# is a harness failure. The sentinel file is written by the thing that did the
# work, so it is the only honest signal.
run_bounded() {
	path=$1
	out=$2
	limit=$3
	: >"$out"
	rm -f "$out.rc"
	( "$path" --version </dev/null >"$out" 2>&1; printf '%s\n' "$?" >"$out.rc" ) &
	probe_pid=$!
	waited=0
	while [ ! -f "$out.rc" ]; do
		if [ "$waited" -ge "$limit" ]; then
			kill "$probe_pid" 2>/dev/null || true
			return 1
		fi
		sleep 1
		waited=$((waited + 1))
	done
	return 0
}

# Liveness for a binary that may legitimately refuse `--version`.
#
# ⚠ `depict`, `solstone-retention` and `speakers-analyze` answer a JSON request
# protocol and reject `--version` with a TYPED error and a nonzero status — so
# exit 0 is the wrong test, and asserting it would fail three shipped binaries
# that are working correctly. What proves the process started is that IT wrote
# something recognisably its own. A Gatekeeper kill produces no output at all.
assert_starts() {
	path=$1
	if ! run_bounded "$path" "$WORK/start.out" 20; then
		refuse "signed, notarized, and did not return within 20s: $path"
	fi
	# Non-empty output is the positive: a process that the loader refused
	# produces none at all. ⛔ Do NOT additionally require the output to
	# *look* like ours — `solstone-retention` answers `{"ok":false,"error":
	# "unknown verb --version"}`, which names neither the binary nor the
	# product, and an allow-list of expected shapes fails three working
	# binaries while proving nothing extra.
	[ -s "$WORK/start.out" ] \
		|| refuse "signed, notarized, and produced no output at all: $path"
	# The failures this rung exists to catch all announce themselves, and each
	# of them can occur on a binary whose signature is perfectly valid.
	if grep -Eq 'dyld\[|Library not loaded|code signature|Killed: 9|Trace/BPT|Operation not permitted|invalid active developer path' "$WORK/start.out"; then
		cat "$WORK/start.out" >&2
		refuse "loader or signature failure on start: $path"
	fi
}

# 🔴 The extracted tree carries NO quarantine, and that is a fact about the
# owner's real path rather than an oversight to correct.
#
# Measured 2026-08-17 on pro5e: a fresh `tar -xzf` of our tarball has no
# `com.apple.quarantine` xattr at all, and `curl` does not set one either — only
# quarantine-aware launchers (browsers, Mail, Messages) do. ⛔ So the `.tar.gz`
# path is **not adjudicated by Gatekeeper on first launch**; the signature and
# notarization are what a later check validates, and the `.pkg` is the container
# where Gatekeeper actually decides. Asserting the absence here keeps the rest of
# this rung honest about which question it is answering.
#
# ⚠ And do NOT "strengthen" this by marking the tree and executing it. Measured
# the same day: a quarantined binary run from a headless shell BLOCKS FOREVER —
# `solstone-core --version` returned in 1s unquarantined and had not returned
# after four minutes quarantined, for a 7.9 MB binary as well as a 95 MB one.
# The first-launch assessment wants a GUI session that an ssh/tmux context does
# not have, so a rung built on it measures the harness and hangs the host.
assert_tar_sets_no_quarantine() {
	if xattr -p com.apple.quarantine "$TREE/bin/solstone-core" >/dev/null 2>&1; then
		refuse "the extracted tree carries a quarantine attribute; this rung's premise no longer holds and its conclusions do not follow"
	fi
	printf 'tar sets no quarantine (verified absent)\n'
}

gatekeeper_rung() {
	assert_gatekeeper_enabled
	install_tar
	assert_tar_sets_no_quarantine

	count=0
	for path in $(macho_executables); do
		count=$((count + 1))
		assert_signed_by_us "$path"
		assert_notarized "$path"
		# Signed and assessed is still not started. Run it.
		assert_starts "$path"
	done
	[ "$count" -eq 9 ] || refuse "expected exactly 9 executables in the tree, found $count"
	printf 'gatekeeper half 1: %s executables signed, notarized, accepted and started\n' "$count"

	payloads=0
	for path in $(macho_payloads); do
		payloads=$((payloads + 1))
		assert_signed_by_us "$path"
		assert_notarized "$path"
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

	# Control for half 1: an ad-hoc signature is a VALID signature, which is
	# why `codesign --verify` cannot be the test. Both notarization instruments
	# must refuse it, and `--verify` must still pass — that contrast is the
	# whole point.
	adhoc=$broken/bin/solstone-core
	/usr/bin/codesign --force --sign - "$adhoc" >/dev/null 2>&1 \
		|| refuse "could not build the ad-hoc control"
	/usr/bin/codesign --verify --strict "$adhoc" >/dev/null 2>&1 \
		|| note "ad-hoc control did not even verify; the control is weaker than intended"
	if spctl -a -vvv -t open --context context:primary-signature "$adhoc" >/dev/null 2>&1; then
		refuse "CONTROL FAILED: Gatekeeper accepted an ad-hoc signed binary, so half 1 proves nothing"
	fi
	if /usr/bin/codesign -vvv -R='notarized' --check-notarization "$adhoc" >/dev/null 2>&1; then
		refuse "CONTROL FAILED: an ad-hoc binary satisfied the notarized requirement, so half 1 proves nothing"
	fi
	printf 'control 1 ok: both notarization instruments refuse an ad-hoc signed binary that codesign --verify accepts\n'

	# Control for half 2: strip the payload's signature and require the
	# hardened-runtime helper to refuse to start. This is the exact failure a
	# binaries-only signing census ships.
	stripped=0
	for path in $(macho_members_in "$broken"); do
		if [ "$(macho_filetype "$path")" = "6" ]; then
			/usr/bin/codesign --remove-signature "$path" >/dev/null 2>&1 || true
			stripped=$((stripped + 1))
		fi
	done
	[ "$stripped" -ge 1 ] || refuse "CONTROL FAILED: no payload was stripped, so control 2 changed nothing"
	for path in $(macho_members_in "$broken"); do
		if [ "$(macho_filetype "$path")" = "6" ]; then
			/usr/bin/codesign -dv --verbose=2 "$path" 2>&1 | grep -Fq 'not signed at all' \
				|| refuse "CONTROL FAILED: $path still carries a signature after --remove-signature"
		fi
	done
	saved_tree=$TREE
	TREE=$broken
	if speakers_probe "$WORK/broken-response.json" 2>"$WORK/broken.err"; then
		lv=unenforced
	else
		lv=enforced
	fi
	TREE=$saved_tree

	# 🔴 REPORT THE HOST'S VERDICT; do not assume it.
	#
	# Whether a hardened-runtime process refuses an UNSIGNED dylib is a property
	# of the macOS version, not of our artifact. Measured 2026-08-17 with the
	# same tarball, the same stripped payloads and the same helper:
	#   macOS 26.5  -> the helper REFUSED to start   (library validation enforced)
	#   macOS 15.7.7 -> the helper RAN and answered  (not enforced)
	# ⛔ So a rung that hard-fails when the control does not fire calls a correct
	# artifact broken on the older OS, and a rung that silently passes claims a
	# proof it did not get. It records which, and the artifact names the host
	# that enforced it.
	if [ "$lv" = enforced ]; then
		printf 'control 2 ok: %s payload(s) stripped, and a hardened-runtime binary refused to start (library validation ENFORCED on %s)\n' \
			"$stripped" "$(sw_vers -productVersion)"
	else
		printf 'control 2 NOT AVAILABLE: %s payload(s) stripped and the helper still ran (library validation NOT enforced on %s). Half 2 stands on the signed-and-notarized census here; its NECESSITY is proven only on a host that enforces.\n' \
			"$stripped" "$(sw_vers -productVersion)"
	fi
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
	# ⛔ NOT 5015. The Docker cleanroom owns its whole network namespace; this
	# rung runs on a real Mac where the founder's own journal is live on the
	# default port, and binding it would both fail the rung (convey exits 75)
	# and disturb a running install. Measured: the first run of this rung died
	# with `convey exited during startup (exit 75)` for exactly that reason.
	port=${SOLSTONE_MACOS_CONVEY_PORT:-51015}
	solstone-core supervisor --journal "$JOURNAL" --no-spl --no-daily "$port" \
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
	attempt=0
	while [ "$(cat "$JOURNAL/health/convey.port" 2>/dev/null || true)" != "$port" ]; do
		attempt=$((attempt + 1))
		if ! kill -0 "$SUPERVISOR_PID" 2>/dev/null || [ "$attempt" -ge 30 ]; then
			cat "$JOURNAL/supervisor.log" >&2 || true
			refuse "convey did not become ready on port $port"
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
	assert_gatekeeper_enabled
	package=$(one_artifact .pkg)
	# ⚠ `xcrun stapler` is Command Line Tools, not base macOS — so it is
	# available on the BUILD host and absent on a genuinely clean Mac, where
	# `xcrun` answers *"No developer tools were found"*. That is a fact about
	# the CHECK, not about the package: an owner's Mac validates a stapled
	# ticket through Gatekeeper without ever running `stapler`. Skip it with a
	# disclosure rather than failing a clean host, and never let the skip pass
	# silently.
	if xcrun --find stapler >/dev/null 2>&1; then
		xcrun stapler validate "$package" >"$WORK/staple.out" 2>&1 \
			|| { cat "$WORK/staple.out" >&2; refuse "package carries no stapled ticket"; }
		printf 'stapled ticket: validated\n'
	else
		printf 'stapled ticket: NOT CHECKED HERE (xcrun/stapler needs Command Line Tools; validated on the build host)\n'
	fi
	spctl -a -vvv -t install "$package" >"$WORK/spctl-pkg.out" 2>&1 \
		|| { cat "$WORK/spctl-pkg.out" >&2; refuse "Gatekeeper rejected the package"; }
	grep -Fq 'accepted' "$WORK/spctl-pkg.out" || refuse "spctl did not accept the package"
	grep -Fq 'Notarized Developer ID' "$WORK/spctl-pkg.out" \
		|| refuse "the package is signed but NOT notarized"
	pkgutil --check-signature "$package" >"$WORK/pkgsig.out" 2>&1 \
		|| { cat "$WORK/pkgsig.out" >&2; refuse "pkgutil refused the package signature"; }
	grep -Fq 'Developer ID Installer: sol pbc' "$WORK/pkgsig.out" \
		|| refuse "the package is not signed with the Developer ID Installer identity"

	# 🔴 "The tree installs" is a done condition, so install it for real.
	# Everything above grades the package as a FILE; this is the only step that
	# grades it as an INSTALL.
	if [ "${SOLSTONE_MACOS_INSTALL_PKG:-}" = "1" ]; then
		sudo installer -pkg "$package" -target / >"$WORK/installer.out" 2>&1 \
			|| { cat "$WORK/installer.out" >&2; refuse "installer refused the package"; }
		for launcher in journal solstone; do
			[ -x "/usr/local/bin/$launcher" ] \
				|| refuse "installed package did not place /usr/local/bin/$launcher"
		done
		run_bounded /usr/local/bin/solstone-core "$WORK/installed.out" 20 \
			|| refuse "the installed solstone-core did not return"
		[ -s "$WORK/installed.out" ] || refuse "the installed solstone-core produced no output"
		printf 'installed from the package: %s\n' "$(head -1 "$WORK/installed.out")"
	else
		printf 'package install: NOT RUN (set SOLSTONE_MACOS_INSTALL_PKG=1 on a disposable host)\n'
	fi
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
			"$shell" -l -c 'command -v journal; command -v solstone' 2>/dev/null || true)
		lines=$(printf '%s\n' "$resolved" | grep -c . || true)
		[ "$lines" -eq 2 ] \
			|| refuse "fresh $shell login shell resolved $lines of 2 launchers"
		! printf '%s\n' "$resolved" | grep -qx '.*/sol' \
			|| refuse "fresh $shell login still resolved sol"
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
