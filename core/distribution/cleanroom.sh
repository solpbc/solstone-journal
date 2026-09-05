#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Live distribution oracle. Every subject is preloaded by immutable digest and
# runs with --pull=never and --network=none. The same file is mounted inside the
# subjects so the operator and subject contracts cannot drift apart.

set -eu

refuse() {
	printf 'cleanroom: %s\n' "$*" >&2
	exit 2
}

one_artifact() {
	suffix=$1
	set -- /artifacts/*"$suffix"
	if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
		refuse "expected exactly one *${suffix} artifact"
	fi
	printf '%s\n' "$1"
}

scan_zero() {
	matches=$(/scan-python.sh /)
	[ -z "$matches" ] || {
		printf '%s\n' "$matches" >&2
		refuse "Python runtime found in zero-Python subject"
	}
}

scan_control() {
	matches=$(/scan-python.sh /)
	printf '%s\n' "$matches"
	[ -n "$matches" ] || refuse "Python control produced no findings"
	printf '%s\n' "$matches" | grep -F 'executable /usr/local/bin/python3.12' >/dev/null \
		|| refuse "Python control missed its known executable"
}

assert_launchers() {
	for launcher in journal solstone; do
		path=$(command -v "$launcher") || refuse "launcher missing: $launcher"
		case $path in
		*site-packages* | *dist-packages* | *.venv/* | *'/venv/'*)
			refuse "launcher resolved through Python layout: $path"
			;;
		esac
	done
}

SUPERVISOR_PID=
SERVER_PID=
cleanup_inside() {
	for pid in ${SUPERVISOR_PID:-} ${SERVER_PID:-}; do
		kill "$pid" 2>/dev/null || true
		wait "$pid" 2>/dev/null || true
	done
}

start_supervisor() {
	with_convey=$1
	SUPERVISOR_PID=
	if [ "$with_convey" = yes ]; then
		solstone-core supervisor --journal "$SOLSTONE_JOURNAL" --no-spl --no-daily 5015 \
			>"$SOLSTONE_JOURNAL/supervisor.log" 2>&1 &
	else
		solstone-core supervisor --journal "$SOLSTONE_JOURNAL" --no-convey --no-spl --no-daily 5015 \
			>"$SOLSTONE_JOURNAL/supervisor.log" 2>&1 &
	fi
	SUPERVISOR_PID=$!
	attempt=0
	while [ ! -S "$SOLSTONE_JOURNAL/health/callosum.sock" ]; do
		attempt=$((attempt + 1))
		if ! kill -0 "$SUPERVISOR_PID" 2>/dev/null || [ "$attempt" -ge 30 ]; then
			cat "$SOLSTONE_JOURNAL/supervisor.log" >&2 || true
			refuse "supervisor did not become ready"
		fi
		sleep 1
	done
	if [ "$with_convey" = yes ]; then
		attempt=0
		while [ "$(cat "$SOLSTONE_JOURNAL/health/convey.port" 2>/dev/null || true)" != 5015 ]; do
			attempt=$((attempt + 1))
			if ! kill -0 "$SUPERVISOR_PID" 2>/dev/null || [ "$attempt" -ge 30 ]; then
				cat "$SOLSTONE_JOURNAL/supervisor.log" >&2 || true
				refuse "convey did not become ready on port 5015"
			fi
			sleep 1
		done
	fi
}

stop_supervisor() {
	[ -n "${SUPERVISOR_PID:-}" ] || return 0
	kill "$SUPERVISOR_PID" 2>/dev/null || true
	wait "$SUPERVISOR_PID" 2>/dev/null || true
	SUPERVISOR_PID=
}

fresh_journal_loop() {
	rung=$1
	journal=/journal
	segment=$journal/chronicle/20990101/default/120000_001
	mkdir -p "$journal/config" "$journal/streams" "$segment/talents"
	printf '%s\n' '{"setup":{"completed_at":1}}' >"$journal/config/journal.json"
	printf '%s\n' '{"name":"default","kind":"default","host":null,"platform":null,"created_at":0,"last_day":"20990101","last_segment":"120000_001","seq":1}' >"$journal/streams/default.json"
	printf '%s\n' '{"stream":"default","seq":1,"prev_segment":null}' >"$segment/stream.json"
	printf '%s\n' \
		'{"raw":"synthetic.flac","model":"fixture","duration":1}' \
		'{"start":"00:00:00","source":"mic","speaker":1,"text":"CLEANROOM_SENTINEL_260817","description":"public synthetic cleanroom fixture"}' \
		>"$segment/audio.jsonl"
	printf '%s\n' '# Audio Transcript' '' 'CLEANROOM_SENTINEL_260817' >"$segment/talents/audio.md"
	export SOLSTONE_JOURNAL=$journal
	export SOL_SKIP_SUPERVISOR_CHECK=1
	if journal indexer -q CLEANROOM_SENTINEL_260817 --day 20990101 --stream default --limit 10 \
		>"$journal/initial-query.out" 2>"$journal/initial-query.err"; then
		refuse "$rung initial query unexpectedly succeeded"
	fi
	grep -F 'journal index is absent' "$journal/initial-query.err" >/dev/null \
		|| refuse "$rung did not prove an initially absent index"
	start_supervisor yes
	journal segment verify 20990101/default/120000_001 --json >"$journal/verify.json"
	! grep -F '"passed": false' "$journal/verify.json" >/dev/null \
		|| refuse "$rung segment verification failed"
	journal indexer --rescan
	journal indexer -q CLEANROOM_SENTINEL_260817 --day 20990101 --stream default --limit 10 \
		>"$journal/query.out"
	grep -F 'Total: 1 chunks' "$journal/query.out" >/dev/null \
		|| refuse "$rung query did not return exactly one chunk"
	grep -F 'CLEANROOM_SENTINEL_260817' "$journal/query.out" >/dev/null \
		|| refuse "$rung query lost its sentinel"
	sol call transcripts read 20990101 --segment 120000_001 --stream default --raw \
		>"$journal/read.out"
	grep -F 'CLEANROOM_SENTINEL_260817' "$journal/read.out" >/dev/null \
		|| refuse "$rung transcript read lost its sentinel"
	stop_supervisor
	scan_zero
	printf 'rung=%s ok\n' "$rung"
}

install_tar() {
	archive=$(one_artifact .tar.gz)
	mkdir /tree
	tar -xzf "$archive" -C /tree
	PATH=/tree/bin:/usr/sbin:/usr/bin:/sbin:/bin
	export PATH
}

install_deb() {
	package=$(one_artifact .deb)
	dpkg -i "$package"
	PATH=/usr/bin:/usr/sbin:/bin:/sbin
	export PATH
}

install_rpm() {
	package=$(one_artifact .rpm)
	rpm -i --replacepkgs "$package"
	PATH=/usr/bin:/usr/sbin:/bin:/sbin
	export PATH
}

start_static_server() {
	root=$1
	: >"$root/server.addr"
	/producer cleanroom-serve "$root" >"$root/server.addr" 2>"$root/server.err" &
	SERVER_PID=$!
	attempt=0
	while [ ! -s "$root/server.addr" ]; do
		attempt=$((attempt + 1))
		if ! kill -0 "$SERVER_PID" 2>/dev/null || [ "$attempt" -ge 30 ]; then
			cat "$root/server.err" >&2 || true
			refuse "loopback origin did not become ready"
		fi
		sleep 1
	done
}

stop_server() {
	[ -n "${SERVER_PID:-}" ] || return 0
	kill "$SERVER_PID" 2>/dev/null || true
	wait "$SERVER_PID" 2>/dev/null || true
	SERVER_PID=
}

bootstrap_install() {
	mkdir /origin /home/clean
	cp /install.sh /origin/install.sh
	cp /artifacts/* /origin/
	start_static_server /origin
	origin=http://$(cat /origin/server.addr)
	version=$(awk -F= '$1 == "version" {print $2}' "$(one_artifact .release)")
	[ -n "$version" ] || refuse "bootstrap release has no version"
	HOME=/home/clean
	export HOME
	curl --fail --silent --show-error "$origin/install.sh" \
		| sh -s -- --version "$version" --origin "$origin"
	login_paths=$(env -i HOME="$HOME" USER=clean LOGNAME=clean TERM=dumb \
		sh -l -c 'command -v journal; command -v solstone')
	printf '%s\n' "$login_paths"
	[ "$(printf '%s\n' "$login_paths" | wc -l | tr -d ' ')" -eq 2 ] \
		|| refuse "fresh login did not resolve all launchers"
	! printf '%s\n' "$login_paths" | grep -qx '.*/sol' \
		|| refuse "fresh login still resolved sol"
	PATH=$HOME/.local/solstone-journal/current/bin:/usr/bin:/usr/sbin:/bin:/sbin
	export PATH
	stop_server
}

talent_rung() {
	install_tar
	assert_launchers
	journal=/journal
	day=$(date +%Y%m%d)
	weekday=$(date +%A)
	segment=$journal/chronicle/$day/default/031700_697
	target_segment=$journal/chronicle/20990101/default/120000_001
	mkdir -p "$journal/config" "$segment" "$target_segment/talents"
	printf '%s\n' '{"stream":"default","seq":1,"prev_segment":null}' >"$segment/stream.json"
	printf '%s\n' '{"stream":"default","seq":1,"prev_segment":null}' \
		>"$target_segment/stream.json"
	printf '%s\n' '# Audio Transcript' '' \
		'No future scheduled activity appears in this synthetic cleanroom segment.' \
		>"$target_segment/talents/audio.md"
	printf '%s\n' \
		'{"start":"00:00:00","source":"mic","speaker":1,"text":"No future scheduled activity appears in this synthetic cleanroom segment."}' \
		>"$target_segment/capture_audio.jsonl"
	printf '%s (%s):\n  03:17 - 03:28 (11m)' "$day" "$weekday" >"$journal/expected-fragment"
	: >"$journal/generation-evidence"
	: >"$journal/generation.addr"
	/producer cleanroom-generate-serve "$journal/generation-evidence" "$journal/expected-fragment" \
		>"$journal/generation.addr" 2>"$journal/generation.err" &
	SERVER_PID=$!
	attempt=0
	while [ ! -s "$journal/generation.addr" ]; do
		attempt=$((attempt + 1))
		if ! kill -0 "$SERVER_PID" 2>/dev/null || [ "$attempt" -ge 30 ]; then
			cat "$journal/generation.err" >&2 || true
			refuse "generation fixture did not become ready"
		fi
		sleep 1
	done
	endpoint=http://$(cat "$journal/generation.addr")
	printf '%s\n' "{\"setup\":{\"completed_at\":1},\"providers\":{\"active\":{\"provider\":\"local\"},\"local\":{\"endpoint_url\":\"$endpoint\",\"served_model_id\":\"cleanroom\"}}}" \
		>"$journal/config/journal.json"
	export SOLSTONE_JOURNAL=$journal
	unset SOL_SKIP_SUPERVISOR_CHECK || true
	[ ! -e "$journal/chronicle/20990101/talents/daily_schedule.json" ] \
		|| refuse "daily_schedule output was pre-seeded"
	[ ! -e "$journal/config/schedules.json" ] || refuse "schedule metadata was pre-seeded"
	start_supervisor yes
	if ! journal think --day 20990101 --refresh >"$journal/think.out" 2>"$journal/think.err"; then
		cat "$journal/think.out" "$journal/think.err" "$journal/supervisor.log" \
			"$journal/generation.err" "$journal/generation-evidence" >&2 || true
		refuse "journal think failed"
	fi
	stop_supervisor
	stop_server
	[ -f "$journal/chronicle/20990101/talents/daily_schedule.json" ] \
		|| refuse "daily_schedule output missing"
	daily=$(tr -d '[:space:]' <"$journal/chronicle/20990101/talents/daily_schedule.json")
	[ "$daily" = '{"primary":"03:00","fallback":"04:00"}' ] \
		|| refuse "daily_schedule output mismatch: $daily"
	grep -F '"daily_time": "03:00"' "$journal/config/schedules.json" >/dev/null \
		|| grep -F '"daily_time":"03:00"' "$journal/config/schedules.json" >/dev/null \
		|| refuse "daily schedule metadata missing"
	! grep -F 'fallback' "$journal/config/schedules.json" >/dev/null \
		|| refuse "fallback leaked into schedule metadata"
	sorted=$(LC_ALL=C sort "$journal/generation-evidence")
	[ "$sorted" = "daily_schedule
morning_briefing
schedule" ] || {
		cat "$journal/generation-evidence" >&2
		refuse "generation fixture saw an incomplete or unexpected daily batch"
	}
	set -- "$journal"/talents/daily_schedule/*.jsonl
	if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
		refuse "daily_schedule terminal event log missing"
	fi
	grep -F '"event":"finish"' "$1" >/dev/null \
		|| refuse "daily_schedule did not reach a terminal finish event"
	grep -F '"name":"daily_schedule"' "$1" >/dev/null \
		|| refuse "daily_schedule terminal event was misattributed"
	scan_zero
	printf 'rung=talent ok\n'
}

speaker_request() {
	output=$1
	interval=$2
	cat <<EOF
{"schema":"solstone-speaker-analyze-request-v1","sample_rate_hz":16000,"full_audio_f32le_path":"/work/audio.f32","reduced_audio_f32le_path":null,"models":{"pyannote_segmentation_onnx_path":"/tree/lib/solstone_journal_models/assets/pyannote-segmentation-3.0.onnx","wespeaker_onnx_path":"/tree/lib/solstone_journal_models/assets/wespeaker-resnet34-256.onnx"},"output_payload_f32le_path":"$output","interval_embedding_payload_f32le_path":"$interval","statement_embedding":{"spans":[{"statement_id":1,"start_s":0.0,"end_s":0.5}]},"diarization":{"spans":[{"statement_id":1,"start_s":0.0,"end_s":0.5}]}}
EOF
}

speakers_rung() {
	install_tar
	mkdir /work
	dd if=/dev/zero of=/work/audio.f32 bs=32000 count=1 2>/dev/null
	speaker_request /work/statements.f32 /work/intervals.f32 \
		| /tree/bin/solstone-core-speakers-analyze > /work/response.json
	grep -F '"schema":"solstone-speaker-analyze-response-v1"' /work/response.json >/dev/null
	grep -F '"shape":[1,256]' /work/response.json >/dev/null
	grep -F '"byte_count":1024' /work/response.json >/dev/null
	grep -F '"speaker_evidence":"none"' /work/response.json >/dev/null
	grep -F '"intervals":null' /work/response.json >/dev/null
	[ "$(wc -c </work/statements.f32 | tr -d ' ')" -eq 1024 ] \
		|| refuse "speaker embedding payload length mismatch"
	if LC_ALL=C od -An -t f4 /work/statements.f32 | grep -E -i 'nan|inf' >/dev/null; then
		refuse "speaker embedding payload contains a non-finite value"
	fi
	[ ! -e /work/intervals.f32 ] || refuse "declined diarization emitted interval payload"
	pyannote=/tree/lib/solstone_journal_models/assets/pyannote-segmentation-3.0.onnx
	mv "$pyannote" "$pyannote.missing"
	if speaker_request /work/missing.f32 /work/missing-intervals.f32 \
		| /tree/bin/solstone-core-speakers-analyze >/work/missing.out 2>/work/missing.err; then
		refuse "speakers rung survived a missing pyannote graph"
	fi
	grep -F 'pyannote-segmentation-3.0.onnx' /work/missing.err >/dev/null \
		|| refuse "pyannote negative did not name the missing graph"
	mv "$pyannote.missing" "$pyannote"
	wespeaker=/tree/lib/solstone_journal_models/assets/wespeaker-resnet34-256.onnx
	mv "$wespeaker" "$wespeaker.missing"
	if speaker_request /work/missing.f32 /work/missing-intervals.f32 \
		| /tree/bin/solstone-core-speakers-analyze >/work/missing.out 2>/work/missing.err; then
		refuse "speakers rung survived a missing wespeaker graph"
	fi
	grep -F 'wespeaker-resnet34-256.onnx' /work/missing.err >/dev/null \
		|| refuse "wespeaker negative did not name the missing graph"
	mv "$wespeaker.missing" "$wespeaker"
	runtime=/tree/lib/solstone-core-speakers-analyze/libonnxruntime.so.1
	mv "$runtime" "$runtime.missing"
	if speaker_request /work/missing.f32 /work/missing-intervals.f32 \
		| /tree/bin/solstone-core-speakers-analyze >/work/missing.out 2>/work/missing.err; then
		refuse "speakers rung survived a missing ONNX Runtime"
	fi
	grep -F 'libonnxruntime.so.1' /work/missing.err >/dev/null \
		|| refuse "runtime negative did not name libonnxruntime.so.1"
	mv "$runtime.missing" "$runtime"
	scan_zero
	printf 'rung=speakers ok\n'
}

pdf_rung() {
	install_tar
	assert_launchers
	[ -x /tree/bin/solstone-core-pdf ] || refuse "solstone-core-pdf missing from the extracted tree"
	[ -f /tree/lib/solstone-core-pdf/libpdfium.so ] \
		|| refuse "libpdfium.so missing from the extracted tree"
	journal=/journal
	mkdir -p "$journal/config" /work
	printf '%s\n' '{"setup":{"completed_at":1}}' >"$journal/config/journal.json"
	export SOLSTONE_JOURNAL=$journal
	export SOL_SKIP_SUPERVISOR_CHECK=1
	[ -f /fixture.pdf ] || refuse "real PDF fixture was not mounted"
	journal importer --source document --timestamp 20260311_120000 --dry-run /fixture.pdf \
		>"$journal/preview.out" 2>"$journal/preview.err" || {
		cat "$journal/preview.out" "$journal/preview.err" >&2 || true
		refuse "document preview failed"
	}
	grep -F '1 PDF documents, 2 total pages' "$journal/preview.out" >/dev/null \
		|| {
			cat "$journal/preview.out" "$journal/preview.err" >&2 || true
			refuse "document preview did not report the fixture PDF"
		}
	! grep -F 'worker spawn failed' "$journal/preview.out" "$journal/preview.err" >/dev/null \
		|| refuse "document preview still failed to spawn the PDF worker"
	/tree/bin/solstone-core-pdf extract /fixture.pdf > /work/extract.json
	grep -F '"schema":"sol-pdf/1"' /work/extract.json >/dev/null \
		|| refuse "PDF extract missing sol-pdf/1 schema"
	grep -F 'SOLPDF_SENTINEL_PAGE_2' /work/extract.json >/dev/null \
		|| refuse "PDF extract lost the fixture sentinel"
	grep -F '0a5c0aef0776024b3fa4ca8f29a7d12dbc9df56c2157c5ae6474dc6fa68479c6' /work/extract.json \
		>/dev/null || refuse "PDF extract digest mismatch"
	library=/tree/lib/solstone-core-pdf/libpdfium.so
	mv "$library" "$library.missing"
	if /tree/bin/solstone-core-pdf extract /fixture.pdf >/work/missing.out 2>/work/missing.err; then
		refuse "pdf rung survived a missing libpdfium.so"
	fi
	grep -E 'libpdfium|PDFium|pdfium' /work/missing.out /work/missing.err >/dev/null \
		|| {
			cat /work/missing.out /work/missing.err >&2 || true
			refuse "pdfium negative did not name libpdfium"
		}
	mv "$library.missing" "$library"
	scan_zero
	printf 'rung=pdf ok\n'
}

inside_main() {
	role=${1:-}
	trap cleanup_inside 0 1 2 15
	case $role in
	scan-control) scan_control ;;
	tar)
		scan_zero
		install_tar
		assert_launchers
		fresh_journal_loop tar
		;;
	deb)
		scan_zero
		install_deb
		assert_launchers
		fresh_journal_loop deb
		;;
	rpm)
		scan_zero
		install_rpm
		assert_launchers
		fresh_journal_loop rpm
		;;
	bootstrap)
		scan_zero
		bootstrap_install
		assert_launchers
		fresh_journal_loop bootstrap
		;;
	talent)
		scan_zero
		talent_rung
		;;
	speakers)
		scan_zero
		speakers_rung
		;;
	pdf)
		scan_zero
		pdf_rung
		;;
	*) refuse "unknown inside role: $role" ;;
	esac
}

ROOT=$(CDPATH='' cd -- "$(dirname "$0")/../.." && pwd)
SELF=$ROOT/core/distribution/cleanroom.sh
INVENTORY=$ROOT/core/distribution/inventory.toml
INSTALL_SH=$ROOT/core/distribution/install.sh
SCAN_SH=$ROOT/core/distribution/scan-python.sh
if [ -n "${SOLSTONE_DISTRIBUTION_BIN:-}" ]; then
	BIN=$SOLSTONE_DISTRIBUTION_BIN
	BIN_EXPLICIT=yes
else
	BIN=$ROOT/core/target/debug/solstone-distribution
	BIN_EXPLICIT=no
fi

if [ "${1:-}" = --inside ]; then
	shift
	inside_main "$@"
	exit 0
fi

subject_digest() {
	id=$1
	printf '%s\n' "$PLAN" | awk -v id="$id" '$1 == "SUBJECT" && $2 == id {print $4}'
}

subject_image() {
	id=$1
	printf '%s\n' "$PLAN" | awk -v id="$id" '$1 == "SUBJECT" && $2 == id {print $3}'
}

is_lower_hex() {
	value=$1
	want=$2
	[ "$(printf '%s' "$value" | wc -c | tr -d ' ')" -eq "$want" ] || return 1
	case $value in
	*[!0-9a-f]*) return 1 ;;
	esac
}

verify_artifact_set() {
	set -- "$ARTIFACTS"/*.release
	release=$1
	base=${release##*/}
	base=${base%.release}
	sha=$ARTIFACTS/$base.sha256
	manifest=$ARTIFACTS/$base.manifest.json
	[ "$(awk 'NF { count++ } END { print count + 0 }' "$sha")" -eq 3 ] \
		|| refuse "checksum sidecar must name exactly three archives"
	for suffix in .tar.gz .deb .rpm; do
		name=$base$suffix
		line=$(awk -v name="$name" '$2 == name && NF == 2 { print $1 }' "$sha")
		is_lower_hex "$line" 64 || refuse "checksum sidecar missing or invalid: $name"
		[ "$(awk -v name="$name" '$2 == name && NF == 2 { count++ } END { print count + 0 }' "$sha")" -eq 1 ] \
			|| refuse "checksum sidecar duplicates: $name"
		grep -F "\"$name\": \"$line\"" "$manifest" >/dev/null \
			|| refuse "manifest checksum mismatch: $name"
	done
	[ "$(grep -c '^    "' "$manifest")" -eq 3 ] \
		|| refuse "manifest must name exactly three archives"
	(cd "$ARTIFACTS" && sha256sum --strict -c "$base.sha256")
	product=$(awk -F= '$1 == "product" { print $2 }' "$release")
	version=$(awk -F= '$1 == "version" { print $2 }' "$release")
	target=$(awk -F= '$1 == "target" { print $2 }' "$release")
	commit=$(awk -F= '$1 == "commit" { print $2 }' "$release")
	lock=$(awk -F= '$1 == "lock_sha256" { print $2 }' "$release")
	epoch=$(awk -F= '$1 == "upgrade_epoch" { print $2 }' "$release")
	window=$(awk -F= '$1 == "retention_window" { print $2 }' "$release")
	min_bootstrap=$(awk -F= '$1 == "min_bootstrap_revision" { print $2 }' "$release")
	[ "$(awk 'NF { count++ } END { print count + 0 }' "$release")" -eq 8 ] \
		|| refuse "release sidecar must contain exactly eight fields"
	[ "$product" = solstone-journal ] || refuse "release product mismatch"
	[ "$target" = linux-x86_64 ] || refuse "release target mismatch"
	[ "$base" = "solstone-journal-$version-linux-x86_64" ] \
		|| refuse "release version does not match artifact basename"
	is_lower_hex "$commit" 40 || refuse "release commit is invalid"
	is_lower_hex "$lock" 64 || refuse "release lock digest is invalid"
	[ "$epoch" = journal-v2 ] || refuse "release upgrade epoch is invalid"
	[ "$window" = 3 ] || refuse "release retention window is invalid"
	[ "$min_bootstrap" = 1 ] || refuse "release minimum bootstrap revision is invalid"
	grep -F '  "product": "solstone-journal",' "$manifest" >/dev/null \
		|| refuse "manifest product mismatch"
	grep -F "  \"version\": \"$version\"," "$manifest" >/dev/null \
		|| refuse "manifest version mismatch"
	grep -F '  "target": "linux-x86_64",' "$manifest" >/dev/null \
		|| refuse "manifest target mismatch"
}

run_subject() {
	id=$1
	role=$2
	digest=$(subject_digest "$id")
	image=$(subject_image "$id")
	[ -n "$digest" ] && [ -n "$image" ] || refuse "subject missing from inventory: $id"
	ref=$image@$digest
	# Inventory pins the registry manifest digest. Docker's image Id is a
	# different hash, so inspect-by-Id cannot see a pulled pin. RepoDigests
	# is the axis the pin lives on. Docker may store the pin as
	# `python@sha256:...` even when the inventory names
	# `python:3.12-slim-bookworm`, so match on the digest suffix.
	digests=$($RUNTIME image inspect --format '{{join .RepoDigests "\n"}}' "$ref") \
		|| refuse "subject is not preloaded: $id $ref"
	printf '%s\n' "$digests" | grep -F "@$digest" >/dev/null \
		|| refuse "subject digest mismatch: $id expected=$ref actual=$(printf '%s' "$digests" | tr '\n' ' ')"
	$RUNTIME run --rm --pull=never --network=none \
		-v "$ARTIFACTS:/artifacts:ro" \
		-v "$SELF:/cleanroom.sh:ro" \
		-v "$INSTALL_SH:/install.sh:ro" \
		-v "$SCAN_SH:/scan-python.sh:ro" \
		-v "$BIN:/producer:ro" \
		-v "$ROOT/core/fixtures/pdf_corpus/text.pdf:/fixture.pdf:ro" \
		"$ref" sh /cleanroom.sh --inside "$role"
	printf 'rung=%s subject=%s digest=%s\n' "$role" "$id" "$digest" >>"$RECEIPT_PARTIAL"
}

host_main() {
	out=${1:-${SOLSTONE_DISTRIBUTION_OUT:-/var/tmp/solstone-distribution-out}}
	case $out in
	*/linux-x86_64) ARTIFACTS=$out ;;
	*) ARTIFACTS=$out/linux-x86_64 ;;
	esac
	[ -d "$ARTIFACTS" ] || refuse "artifact directory missing: $ARTIFACTS"
	for suffix in .tar.gz .deb .rpm .sha256 .manifest.json .release; do
		set -- "$ARTIFACTS"/*"$suffix"
		if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
			refuse "expected exactly one *${suffix} in $ARTIFACTS"
		fi
	done
	if [ "$BIN_EXPLICIT" = no ]; then
		cargo build --manifest-path "$ROOT/core/Cargo.toml" -p solstone-core-distribution \
			--bin solstone-distribution --locked --offline
	else
		[ -x "$BIN" ] || refuse "producer binary is not executable: $BIN"
	fi
	PLAN=$($BIN cleanroom-plan "$INVENTORY")
	printf '%s\n' "$PLAN" | grep -F 'SUBJECT python-3.12-control ' >/dev/null \
		|| refuse "Python control is absent from the cleanroom plan"
	RUNTIME=${SOLSTONE_CONTAINER_RUNTIME:-docker}
	builder=${SOLSTONE_CLEANROOM_BUILDER_ID:-}
	case $builder in
	sha256:*) ;;
	*) refuse "SOLSTONE_CLEANROOM_BUILDER_ID must be an immutable sha256 image ID" ;;
	esac
	is_lower_hex "${builder#sha256:}" 64 \
		|| refuse "SOLSTONE_CLEANROOM_BUILDER_ID must be an immutable sha256 image ID"
	verify_artifact_set
	receipt=${SOLSTONE_CLEANROOM_RECEIPT:-/var/tmp/solstone-distribution-cleanroom.receipt}
	RECEIPT_PARTIAL=${receipt}.partial.$$
	trap 'rm -f "$RECEIPT_PARTIAL"' 0 1 2 15
	{
		printf 'schema=solstone-distribution-cleanroom-v1\n'
		printf 'commit=%s\n' "$commit"
		printf 'lock_sha256=%s\n' "$lock"
		printf 'builder_image=%s\n' "$builder"
	} >"$RECEIPT_PARTIAL"
	run_subject python-3.12-control scan-control
	run_subject debian-bookworm-no-python tar
	run_subject debian-bookworm-no-python deb
	run_subject fedora-42-no-python rpm
	run_subject fedora-42-no-python bootstrap
	run_subject debian-bookworm-no-python talent
	run_subject debian-bookworm-no-python speakers
	run_subject debian-bookworm-no-python pdf
	if [ -e "$receipt" ]; then
		cmp -s "$RECEIPT_PARTIAL" "$receipt" || refuse "receipt conflict: $receipt"
		rm -f "$RECEIPT_PARTIAL"
	else
		mv "$RECEIPT_PARTIAL" "$receipt"
	fi
	trap - 0 1 2 15
	printf 'cleanroom=ok receipt=%s\n' "$receipt"
}

self_test() {
	test_root=/var/tmp/solstone-cleanroom-self-test-$$
	trap 'find "$test_root" -depth -delete 2>/dev/null || true' 0 1 2 15
	mkdir -p "$test_root/artifacts/linux-x86_64"
	base=solstone-journal-1.0.22-linux-x86_64
	artifact_dir=$test_root/artifacts/linux-x86_64
	for suffix in .tar.gz .deb .rpm; do
		: >"$artifact_dir/$base$suffix"
	done
	(cd "$artifact_dir" && sha256sum "$base.tar.gz" "$base.deb" "$base.rpm" >"$base.sha256")
	digest=$(sha256sum "$artifact_dir/$base.tar.gz" | awk '{ print $1 }')
	printf '%s\n' \
		'{' \
		'  "product": "solstone-journal",' \
		'  "version": "1.0.22",' \
		'  "target": "linux-x86_64",' \
		'  "files": {' \
		"    \"$base.tar.gz\": \"$digest\"," \
		"    \"$base.deb\": \"$digest\"," \
		"    \"$base.rpm\": \"$digest\"" \
		'  }' \
		'}' >"$artifact_dir/$base.manifest.json"
	printf '%s\n' 'product=solstone-journal' 'version=1.0.22' 'target=linux-x86_64' \
		'commit=0000000000000000000000000000000000000001' \
		'lock_sha256=0000000000000000000000000000000000000000000000000000000000000001' \
		'upgrade_epoch=journal-v2' \
		'retention_window=3' \
		'min_bootstrap_revision=1' \
		>"$artifact_dir/$base.release"
	cat >"$test_root/runtime" <<'EOF'
#!/bin/sh
set -eu
case "$1 $2" in
'image inspect')
	shift 4
	printf '%s\n' "$1"
	;;
'run --rm')
	shift 2
	seen_pull=no
	seen_network=no
	role=
	for arg in "$@"; do
		[ "$arg" = --pull=never ] && seen_pull=yes
		[ "$arg" = --network=none ] && seen_network=yes
		role=$arg
	done
	[ "$seen_pull" = yes ] && [ "$seen_network" = yes ] || exit 91
	printf '%s\n' "$role" >>"$FAKE_LOG"
	[ "${FAKE_FAIL_ROLE:-}" != "$role" ] || exit 41
	;;
*) exit 92 ;;
esac
EOF
	chmod +x "$test_root/runtime"
	FAKE_LOG=$test_root/runtime.log
	export FAKE_LOG
	SOLSTONE_CONTAINER_RUNTIME=$test_root/runtime \
	SOLSTONE_CLEANROOM_BUILDER_ID=sha256:1111111111111111111111111111111111111111111111111111111111111111 \
	SOLSTONE_CLEANROOM_RECEIPT=$test_root/receipt \
		"$SELF" "$test_root/artifacts" >/dev/null
	[ "$(cat "$FAKE_LOG")" = "scan-control
tar
deb
rpm
bootstrap
talent
speakers
pdf" ] || refuse "fake runtime did not see the exact rung order"
	: >"$FAKE_LOG"
	rm -f "$test_root/receipt"
	if FAKE_FAIL_ROLE=deb SOLSTONE_CONTAINER_RUNTIME=$test_root/runtime \
		SOLSTONE_CLEANROOM_BUILDER_ID=sha256:1111111111111111111111111111111111111111111111111111111111111111 \
		SOLSTONE_CLEANROOM_RECEIPT=$test_root/receipt \
		"$SELF" "$test_root/artifacts" >/dev/null 2>&1; then
		refuse "fake runtime failure was accepted"
	fi
	[ ! -e "$test_root/receipt" ] || refuse "failed run published a receipt"
	printf 'cleanroom-self-test=ok\n'
}

case ${1:-} in
--self-test) self_test ;;
*) host_main "$@" ;;
esac
