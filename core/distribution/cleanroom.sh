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
	matches=$(/qualification-tools/scan-python.sh /)
	[ -z "$matches" ] || {
		printf '%s\n' "$matches" >&2
		refuse "Python runtime found in zero-Python subject"
	}
}

scan_control() {
	matches=$(/qualification-tools/scan-python.sh /)
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

setup_fixture_journal() {
	journal=$1
	mkdir -p /home/clean "$journal"
	HOME=/home/clean
	export HOME
	journal setup -y --journal "$journal" --accept-existing-journal \
		--skip-models --skip-brain --skip-skills --skip-service \
		--skip-wrapper --skip-path \
		>"$journal/setup.out" 2>"$journal/setup.err" || {
		cat "$journal/setup.out" "$journal/setup.err" >&2 || true
		refuse "fixture setup failed"
	}
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
	setup_fixture_journal "$journal"
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
	solstone call journal read \
		--path chronicle/20990101/default/120000_001/talents/audio.md --max 16384 \
		>"$journal/read.out"
	grep -F 'CLEANROOM_SENTINEL_260817' "$journal/read.out" >/dev/null \
		|| refuse "$rung transcript read lost its sentinel"
	stop_supervisor
	grep -F 'supervisor: built-in schedules registered:' "$journal/supervisor.log" >/dev/null \
		|| refuse "$rung supervisor did not register built-in schedules"
	grep -F 'supervisor: maintenance schedules reconciled' "$journal/supervisor.log" >/dev/null \
		|| refuse "$rung supervisor did not reconcile maintenance schedules"
	scan_zero
	printf 'rung=%s coverage=runtime-loop,supervisor-schedule-registration ok\n' "$rung"
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
	# This immutable, networkless subject intentionally carries no package
	# dependency closure. The systemd package gate owns dependency resolution;
	# this rung installs the exact payload and proves its Python-free runtime.
	dpkg -i --force-depends "$package"
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
	/qualification-tools/solstone-distribution cleanroom-serve "$root" \
		>"$root/server.addr" 2>"$root/server.err" &
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
	version=$(awk -F= '$1 == "version" {print $2}' "$(one_artifact .release)")
	[ -n "$version" ] || refuse "bootstrap release has no version"
	expected_commit=$(awk -F= '$1 == "commit" {print $2}' "$(one_artifact .release)")
	[ "$(printf '%s' "$expected_commit" | wc -c | tr -d ' ')" -eq 40 ] \
		|| refuse "bootstrap release has no valid commit"
	case $expected_commit in
	*[!0-9a-f]*) refuse "bootstrap release has no valid commit" ;;
	esac
	negative_root=/origin-negative/solstone-journal/release/$version
	mkdir -p "$negative_root" /home/negative
	cp /qualification-tools/install.sh /origin-negative/install.sh
	cp /artifacts/* "$negative_root/"
	printf '\n' >>"$negative_root/solstone-journal-$version-linux-x86_64.manifest.json"
	start_static_server /origin-negative
	negative_origin=http://$(cat /origin-negative/server.addr)
	HOME=/home/negative
	PATH=/qualification-tools:/usr/bin:/usr/sbin:/bin:/sbin
	LD_LIBRARY_PATH=/qualification-tools
	export HOME PATH LD_LIBRARY_PATH
	if sh /origin-negative/install.sh --version "$version" --origin "$negative_origin" \
		>/origin-negative/install.out 2>/origin-negative/install.err; then
		refuse "tampered signed manifest was accepted"
	else
		negative_status=$?
	fi
	[ "$negative_status" -eq 2 ] \
		|| refuse "tampered signed manifest returned unexpected status: $negative_status"
	[ "$(awk 'NF { count++ } END { print count + 0 }' /origin-negative/install.err)" -eq 1 ] \
		|| refuse "tampered signed manifest did not produce one bounded refusal"
	grep -Fqx 'signature-invalid: manifest signature' /origin-negative/install.err \
		|| refuse "tampered signed manifest produced the wrong refusal"
	[ ! -e "$HOME/.local/solstone-journal/current" ] \
		|| refuse "tampered signed manifest selected a current version"
	[ ! -e "$HOME/.local/solstone-journal/install-receipt" ] \
		|| refuse "tampered signed manifest published an install receipt"
	stop_server

	release_root=/origin/solstone-journal/release/$version
	mkdir -p "$release_root" /home/clean
	cp /qualification-tools/install.sh /origin/install.sh
	cp /artifacts/* "$release_root/"
	start_static_server /origin
	origin=http://$(cat /origin/server.addr)
	HOME=/home/clean
	PATH=/qualification-tools:/usr/bin:/usr/sbin:/bin:/sbin
	LD_LIBRARY_PATH=/qualification-tools
	export HOME
	export PATH LD_LIBRARY_PATH
	curl --fail --silent --show-error "$origin/install.sh" -o /origin/downloaded-install.sh
	[ "$(sha256sum /qualification-tools/install.sh | awk '{print $1}')" = \
		"$(sha256sum /origin/downloaded-install.sh | awk '{print $1}')" ] \
		|| refuse "bootstrap did not fetch the exact installer"
	if sh /origin/downloaded-install.sh --version "$version" --origin "$origin" \
		>/origin/install.out 2>/origin/install.err; then
		refuse "networkless bootstrap unexpectedly completed full setup"
	else
		status=$?
	fi
	[ "$status" -eq 2 ] || {
		cat /origin/install.out /origin/install.err >&2 || true
		refuse "networkless bootstrap returned unexpected status: $status"
	}
	[ "$(awk 'NF { count++ } END { print count + 0 }' /origin/install.err)" -eq 3 ] \
		|| {
			cat /origin/install.err >&2
			refuse "networkless bootstrap refusal was not the exact three-line setup boundary"
		}
	grep -E '^journal setup: install_models failed: download origin unavailable at updates\.solstone\.app:' \
		/origin/install.err >/dev/null || {
			cat /origin/install.err >&2
			refuse "networkless bootstrap did not refuse at pinned model acquisition"
		}
	grep -Fqx 'journal setup: service failed: error: service command spawn failed: No such file or directory (os error 2)' \
		/origin/install.err || {
			cat /origin/install.err >&2
			refuse "networkless bootstrap did not refuse at service setup"
		}
	grep -Fqx 'setup-failed: current remains on the candidate and its receipt marks setup pending; rerun this same install.sh command' \
		/origin/install.err || {
			cat /origin/install.err >&2
			refuse "networkless bootstrap did not retain the exact pending-setup refusal"
		}
	receipt=$HOME/.local/solstone-journal/install-receipt
	grep -Fqx "installer_revision=$expected_commit" "$receipt" \
		|| refuse "networkless bootstrap receipt has the wrong installer revision"
	grep -Fqx 'signature_verification=verified' "$receipt" \
		|| refuse "networkless bootstrap did not verify the signed release"
	grep -Fqx 'setup_status=pending' "$receipt" \
		|| refuse "networkless bootstrap did not retain pending setup state"
	[ -L "$HOME/.local/solstone-journal/current" ] \
		|| refuse "networkless bootstrap did not retain the verified candidate"
	unset LD_LIBRARY_PATH
	PATH=$HOME/.local/solstone-journal/current/bin:/usr/bin:/usr/sbin:/bin:/sbin
	export PATH
	stop_server
	printf 'rung=bootstrap signature-negative=ok expected-refusal=setup-failed\n'
}

talent_direct_rung() {
	install_tar
	assert_launchers
	journal=/journal-direct
	day=$(date +%Y%m%d)
	weekday=$(date +%A)
	segment=$journal/chronicle/$day/default/031700_697
	target_segment=$journal/chronicle/20990101/default/120000_001
	mkdir -p "$journal/config" "$segment/talents" "$target_segment/talents"
	printf '%s\n' '{"stream":"default","seq":1,"prev_segment":null}' >"$segment/stream.json"
	printf '%s\n' '{"stream":"default","seq":1,"prev_segment":null}' \
		>"$target_segment/stream.json"
	printf '%s\n' '# Audio Transcript' '' \
		'No future scheduled activity appears in this synthetic cleanroom segment.' \
		>"$target_segment/talents/audio.md"
	printf '%s\n' \
		'{"start":"00:00:00","source":"mic","speaker":1,"text":"No future scheduled activity appears in this synthetic cleanroom segment."}' \
		>"$target_segment/capture_audio.jsonl"
	printf '%s\n' \
		'{"density":"active","content_type":"work","activity_summary":"Held synthetic cleanroom activity.","facets":[]}' \
		>"$segment/talents/sense.json"
	printf '%s\n' \
		'{"density":"idle","content_type":"idle","activity_summary":"","facets":[]}' \
		>"$target_segment/talents/sense.json"
	printf '%s (%s):\n  03:17 - 03:28 (11m)' "$day" "$weekday" >"$journal/expected-fragment"
	: >"$journal/generation-evidence"
	: >"$journal/generation.addr"
	/qualification-tools/solstone-distribution cleanroom-generate-serve \
		"$journal/generation-evidence" "$journal/expected-fragment" \
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
	setup_fixture_journal "$journal"
	printf '%s\n' "{\"setup\":{\"completed_at\":1},\"providers\":{\"active\":{\"provider\":\"local\"},\"local\":{\"endpoint_url\":\"$endpoint\",\"served_model_id\":\"cleanroom\"}}}" \
		>"$journal/config/journal.json"
	export SOLSTONE_JOURNAL=$journal
	[ ! -e "$journal/chronicle/20990101/talents/daily_schedule.json" ] \
		|| refuse "daily_schedule output was pre-seeded"
	[ ! -e "$journal/config/schedules.json" ] || refuse "schedule metadata was pre-seeded"
	printf '%s\n' '{"name":"daily_schedule","day":"20990101"}' \
		>"$journal/talent-request.json"
	if ! /tree/bin/solstone-core __talent-worker \
		<"$journal/talent-request.json" >"$journal/talent.out" 2>"$journal/talent.err"; then
		cat "$journal/talent.out" "$journal/talent.err" \
			"$journal/generation.err" "$journal/generation-evidence" >&2 || true
		refuse "direct talent worker failed"
	fi
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
	[ "$sorted" = "daily_schedule" ] || {
		cat "$journal/generation-evidence" >&2
		refuse "generation fixture saw an unexpected direct-worker batch"
	}
	grep -F '"event":"finish"' "$journal/talent.out" >/dev/null \
		|| refuse "direct talent worker did not report finish"
	grep -F '"name":"daily_schedule"' "$journal/talent.out" >/dev/null \
		|| refuse "direct talent worker finish was misattributed"
	cat "$journal/talent.out"
	scan_zero
	printf 'rung=talent coverage=direct-worker ok\n'
}

talent_orchestration_rung() {
	assert_launchers
	journal=/journal-orchestration
	day=$(date +%Y%m%d)
	target_day=$(date -d yesterday +%Y%m%d)
	weekday=$(date +%A)
	segment=$journal/chronicle/$day/default/031700_697
	target_segment=$journal/chronicle/$target_day/default/120000_001
	mkdir -p "$journal/config" "$segment/talents" "$target_segment/talents"
	printf '%s\n' '{"stream":"default","seq":1,"prev_segment":null}' >"$segment/stream.json"
	printf '%s\n' '{"stream":"default","seq":1,"prev_segment":null}' \
		>"$target_segment/stream.json"
	printf '%s\n' '# Audio Transcript' '' \
		'No future scheduled activity appears in this synthetic cleanroom segment.' \
		>"$target_segment/talents/audio.md"
	printf '%s\n' \
		'{"start":"00:00:00","source":"mic","speaker":1,"text":"No future scheduled activity appears in this synthetic cleanroom segment."}' \
		>"$target_segment/capture_audio.jsonl"
	printf '%s\n' \
		'{"density":"active","content_type":"work","activity_summary":"Held synthetic cleanroom activity.","facets":[]}' \
		>"$segment/talents/sense.json"
	printf '%s\n' \
		'{"density":"active","content_type":"work","activity_summary":"Held synthetic cleanroom activity.","facets":[]}' \
		>"$target_segment/talents/sense.json"
	# The retained sense output is a negative control: --mark-updated forces
	# production orchestration to replace it through the live fixture.
	printf '%s (%s):\n  03:17 - 03:28 (11m)' "$day" "$weekday" >"$journal/expected-fragment"
	: >"$journal/generation-evidence"
	: >"$journal/generation.addr"
	/qualification-tools/solstone-distribution cleanroom-generate-serve \
		"$journal/generation-evidence" "$journal/expected-fragment" \
		>"$journal/generation.addr" 2>"$journal/generation.err" &
	SERVER_PID=$!
	attempt=0
	while [ ! -s "$journal/generation.addr" ]; do
		attempt=$((attempt + 1))
		if ! kill -0 "$SERVER_PID" 2>/dev/null || [ "$attempt" -ge 30 ]; then
			cat "$journal/generation.err" >&2 || true
			refuse "orchestration generation fixture did not become ready"
		fi
		sleep 1
	done
	endpoint=http://$(cat "$journal/generation.addr")
	setup_fixture_journal "$journal"
	printf '%s\n' "{\"setup\":{\"completed_at\":1},\"providers\":{\"active\":{\"provider\":\"local\"},\"local\":{\"endpoint_url\":\"$endpoint\",\"served_model_id\":\"cleanroom\"}}}" \
		>"$journal/config/journal.json"
	export SOLSTONE_JOURNAL=$journal
	export SOL_SKIP_SUPERVISOR_CHECK=1
	[ ! -e "$journal/chronicle/$target_day/talents/daily_schedule.json" ] \
		|| refuse "orchestration daily_schedule output was pre-seeded"
	[ ! -e "$journal/config/schedules.json" ] \
		|| refuse "orchestration schedule metadata was pre-seeded"
	start_supervisor yes
	if ! journal reprocess "$target_day" --mark-updated >"$journal/reprocess.out" \
		2>"$journal/reprocess.err"; then
		cat "$journal/reprocess.out" "$journal/reprocess.err" \
			"$journal/supervisor.log" >&2 || true
		refuse "supervisor daily reprocess request failed"
	fi
	cat "$journal/reprocess.out"
	grep -Fqx "reprocess (mark-updated) submitted for $target_day" \
		"$journal/reprocess.out" || {
			cat "$journal/reprocess.out" "$journal/reprocess.err" >&2 || true
			refuse "supervisor daily reprocess request was not accepted"
		}
	attempt=0
	while :; do
		set -- "$journal"/talents/daily_schedule/*.jsonl
		if [ "$#" -eq 1 ] && [ -f "$1" ] \
			&& grep -F '"event":"finish"' "$1" >/dev/null 2>&1; then
			break
		fi
		attempt=$((attempt + 1))
		if ! kill -0 "$SUPERVISOR_PID" 2>/dev/null || [ "$attempt" -ge 120 ]; then
			cat "$journal/supervisor.log" "$journal/generation.err" \
				"$journal/generation-evidence" >&2 || true
			refuse "supervisor daily catchup did not reach a terminal event"
		fi
		sleep 1
	done
	stop_supervisor
	stop_server
	[ -f "$journal/chronicle/$target_day/talents/daily_schedule.json" ] \
		|| refuse "orchestration daily_schedule output missing"
	daily=$(tr -d '[:space:]' <"$journal/chronicle/$target_day/talents/daily_schedule.json")
	[ "$daily" = '{"primary":"03:00","fallback":"04:00"}' ] \
		|| refuse "orchestration daily_schedule output mismatch: $daily"
	grep -F '"daily_time": "03:00"' "$journal/config/schedules.json" >/dev/null \
		|| grep -F '"daily_time":"03:00"' "$journal/config/schedules.json" >/dev/null \
		|| refuse "orchestration daily schedule metadata missing"
	! grep -F 'fallback' "$journal/config/schedules.json" >/dev/null \
		|| refuse "orchestration fallback leaked into schedule metadata"
	sorted=$(LC_ALL=C sort "$journal/generation-evidence")
	[ "$sorted" = "daily_schedule
morning_briefing
schedule
sense" ] || {
		cat "$journal/generation-evidence" >&2
		refuse "generation fixture saw an incomplete or unexpected production daily batch"
	}
	set -- "$journal"/talents/daily_schedule/*.jsonl
	if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
		refuse "orchestration daily_schedule terminal event log missing"
	fi
	grep -F '"event":"finish"' "$1" >/dev/null \
		|| refuse "orchestration daily_schedule did not persist a terminal finish event"
	grep -F '"name":"daily_schedule"' "$1" >/dev/null \
		|| refuse "orchestration daily_schedule terminal event was misattributed"
	scan_zero
	printf 'rung=talent coverage=supervisor-catchup-orchestration,daily-batch,persisted-terminal-event ok\n'
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
	setup_fixture_journal "$journal"
	printf '%s\n' '{"setup":{"completed_at":1}}' >"$journal/config/journal.json"
	export SOLSTONE_JOURNAL=$journal
	export SOL_SKIP_SUPERVISOR_CHECK=1
	[ -f /qualification-tools/text.pdf ] || refuse "real PDF fixture was not mounted"
	journal importer --source document --timestamp 20260311_120000 --dry-run \
		/qualification-tools/text.pdf \
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
	/tree/bin/solstone-core-pdf extract /qualification-tools/text.pdf > /work/extract.json
	grep -F '"schema":"sol-pdf/1"' /work/extract.json >/dev/null \
		|| refuse "PDF extract missing sol-pdf/1 schema"
	grep -F 'SOLPDF_SENTINEL_PAGE_2' /work/extract.json >/dev/null \
		|| refuse "PDF extract lost the fixture sentinel"
	grep -F '0a5c0aef0776024b3fa4ca8f29a7d12dbc9df56c2157c5ae6474dc6fa68479c6' /work/extract.json \
		>/dev/null || refuse "PDF extract digest mismatch"
	library=/tree/lib/solstone-core-pdf/libpdfium.so
	mv "$library" "$library.missing"
	if /tree/bin/solstone-core-pdf extract /qualification-tools/text.pdf \
		>/work/missing.out 2>/work/missing.err; then
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
		talent_direct_rung
		talent_orchestration_rung
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
SCAN_SH=$ROOT/core/distribution/scan-python.sh
PDF_FIXTURE=$ROOT/core/fixtures/pdf_corpus/text.pdf
EXPECTED_MINISIGN_SHA256=4cad7876a506ce2d53dee69a2316c6cf1ede6260ce22f647e4decfaf346b2878
EXPECTED_MINISIGN_LIB_SHA256=b3e7f801a4bf685528ecfda3c58b7ed2e4d23d17f97056310c89ddd85bad02ed
TOOL_SNAPSHOT=
ARTIFACT_SNAPSHOT=
RECEIPT_PARTIAL=
SAVED_BIN=
SAVED_SELF=
SAVED_INVENTORY=
SAVED_SCAN_SH=
SAVED_PDF_FIXTURE=
SAVED_ARTIFACTS=
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

cleanup_host() {
	[ -z "${RECEIPT_PARTIAL:-}" ] || rm -f "$RECEIPT_PARTIAL"
	[ -z "${SAVED_BIN:-}" ] || BIN=$SAVED_BIN
	[ -z "${SAVED_SELF:-}" ] || SELF=$SAVED_SELF
	[ -z "${SAVED_INVENTORY:-}" ] || INVENTORY=$SAVED_INVENTORY
	[ -z "${SAVED_SCAN_SH:-}" ] || SCAN_SH=$SAVED_SCAN_SH
	[ -z "${SAVED_PDF_FIXTURE:-}" ] || PDF_FIXTURE=$SAVED_PDF_FIXTURE
	[ -z "${SAVED_ARTIFACTS:-}" ] || ARTIFACTS=$SAVED_ARTIFACTS
	if [ -n "${TOOL_SNAPSHOT:-}" ] && [ -d "$TOOL_SNAPSHOT" ]; then
		find "$TOOL_SNAPSHOT" -depth -delete 2>/dev/null || true
	fi
	if [ -n "${ARTIFACT_SNAPSHOT:-}" ] && [ -d "$ARTIFACT_SNAPSHOT" ]; then
		find "$ARTIFACT_SNAPSHOT" -depth -delete 2>/dev/null || true
	fi
	TOOL_SNAPSHOT=
	ARTIFACT_SNAPSHOT=
	SAVED_BIN=
	SAVED_SELF=
	SAVED_INVENTORY=
	SAVED_SCAN_SH=
	SAVED_PDF_FIXTURE=
	SAVED_ARTIFACTS=
}

snapshot_artifacts() {
	artifact_input=$ARTIFACTS
	SAVED_ARTIFACTS=$artifact_input
	set -- "$artifact_input"/*.release
	[ "$#" -eq 1 ] && [ -f "$1" ] && [ ! -L "$1" ] \
		|| refuse "expected exactly one regular, non-symlink *.release in $artifact_input"
	base=${1##*/}
	base=${base%.release}
	case $base in
	*[!A-Za-z0-9._-]*) refuse "artifact basename contains unsupported characters: $base" ;;
	esac
	set -- \
		"$base.tar.gz" \
		"$base.deb" \
		"$base.rpm" \
		"$base.release" \
		"$base.sha256" \
		"$base.manifest.json" \
		"$base.manifest.json.minisig"
	artifact_member_marks=$(find "$artifact_input" -mindepth 1 -maxdepth 1 -printf .) \
		|| refuse "could not enumerate artifact directory"
	[ "${#artifact_member_marks}" -eq 7 ] \
		|| refuse "artifact directory must contain exactly seven shipped members"
	for name in "$@"; do
		path=$artifact_input/$name
		[ -f "$path" ] && [ ! -L "$path" ] \
			|| refuse "artifact member must be a regular, non-symlink file: $name"
	done
	ARTIFACT_SNAPSHOT=$(mktemp -d /var/tmp/solstone-cleanroom-artifacts-XXXXXX) \
		|| refuse "could not create artifact snapshot"
	chmod 700 "$ARTIFACT_SNAPSHOT"
	mkdir "$ARTIFACT_SNAPSHOT/artifacts"
	for name in "$@"; do
		cp --no-dereference -- "$artifact_input/$name" "$ARTIFACT_SNAPSHOT/artifacts/$name" \
			|| refuse "could not snapshot artifact member: $name"
		[ -f "$ARTIFACT_SNAPSHOT/artifacts/$name" ] \
			&& [ ! -L "$ARTIFACT_SNAPSHOT/artifacts/$name" ] \
			|| refuse "artifact member changed type while it was snapshotted: $name"
	done
	chmod 400 "$ARTIFACT_SNAPSHOT"/artifacts/*
	ARTIFACTS=$ARTIFACT_SNAPSHOT/artifacts
}

snapshot_qualification_tools() {
	producer_input=$BIN
	self_input=$SELF
	inventory_input=$INVENTORY
	scan_input=$SCAN_SH
	pdf_fixture_input=$PDF_FIXTURE
	SAVED_BIN=$producer_input
	SAVED_SELF=$self_input
	SAVED_INVENTORY=$inventory_input
	SAVED_SCAN_SH=$scan_input
	SAVED_PDF_FIXTURE=$pdf_fixture_input
	minisign_input=${SOLSTONE_CLEANROOM_MINISIGN_BIN:-}
	minisign_lib_input=${SOLSTONE_CLEANROOM_MINISIGN_LIB:-}
	expected_producer_sha256=${SOLSTONE_CLEANROOM_PRODUCER_SHA256:-}
	expected_installer_sha256=${SOLSTONE_CLEANROOM_INSTALLER_SHA256:-}
	is_lower_hex "$expected_producer_sha256" 64 \
		|| refuse "SOLSTONE_CLEANROOM_PRODUCER_SHA256 must be a lowercase sha256 digest"
	is_lower_hex "$expected_installer_sha256" 64 \
		|| refuse "SOLSTONE_CLEANROOM_INSTALLER_SHA256 must be a lowercase sha256 digest"
	[ -f "$producer_input" ] && [ ! -L "$producer_input" ] && [ -x "$producer_input" ] \
		|| refuse "qualification producer must be an executable regular file"
	[ -f "$minisign_input" ] && [ ! -L "$minisign_input" ] && [ -x "$minisign_input" ] \
		|| refuse "SOLSTONE_CLEANROOM_MINISIGN_BIN must name an executable regular file"
	[ -f "$minisign_lib_input" ] && [ ! -L "$minisign_lib_input" ] \
		|| refuse "SOLSTONE_CLEANROOM_MINISIGN_LIB must name a regular file"
	minisign_sha256=$(sha256sum "$minisign_input") || refuse "could not hash minisign"
	minisign_sha256=${minisign_sha256%% *}
	[ "$minisign_sha256" = "$EXPECTED_MINISIGN_SHA256" ] \
		|| refuse "minisign digest is not the approved qualification pin"
	minisign_lib_sha256=$(sha256sum "$minisign_lib_input") \
		|| refuse "could not hash minisign library"
	minisign_lib_sha256=${minisign_lib_sha256%% *}
	[ "$minisign_lib_sha256" = "$EXPECTED_MINISIGN_LIB_SHA256" ] \
		|| refuse "minisign library digest is not the approved qualification pin"
	TOOL_SNAPSHOT=$(mktemp -d /var/tmp/solstone-cleanroom-tools-XXXXXX) \
		|| refuse "could not create qualification-tool snapshot"
	chmod 700 "$TOOL_SNAPSHOT"
	cp --no-dereference -- "$minisign_input" "$TOOL_SNAPSHOT/minisign"
	cp --no-dereference -- "$minisign_lib_input" "$TOOL_SNAPSHOT/libsodium.so.26"
	cp --no-dereference -- "$producer_input" "$TOOL_SNAPSHOT/solstone-distribution"
	cp --no-dereference -- "$self_input" "$TOOL_SNAPSHOT/cleanroom.sh"
	cp --no-dereference -- "$inventory_input" "$TOOL_SNAPSHOT/inventory.toml"
	cp --no-dereference -- "$scan_input" "$TOOL_SNAPSHOT/scan-python.sh"
	cp --no-dereference -- "$pdf_fixture_input" "$TOOL_SNAPSHOT/text.pdf"
	for name in minisign libsodium.so.26 solstone-distribution cleanroom.sh \
		inventory.toml scan-python.sh text.pdf; do
		[ -f "$TOOL_SNAPSHOT/$name" ] && [ ! -L "$TOOL_SNAPSHOT/$name" ] \
			|| refuse "qualification tool changed type while it was snapshotted: $name"
	done
	SOURCE_REPO=${SOLSTONE_CLEANROOM_SOURCE_REPO:-$ROOT}
	GIT_NO_REPLACE_OBJECTS=1 git -C "$SOURCE_REPO" cat-file -e "${commit}^{commit}" \
		|| refuse "artifact source commit is unavailable: $commit"
	GIT_NO_REPLACE_OBJECTS=1 git -C "$SOURCE_REPO" \
		show "${commit}:core/distribution/install.sh" \
		>"$TOOL_SNAPSHOT/install.sh" \
		|| refuse "could not materialize installer from artifact source: $commit"
	chmod 500 "$TOOL_SNAPSHOT/minisign" "$TOOL_SNAPSHOT/install.sh" \
		"$TOOL_SNAPSHOT/solstone-distribution" "$TOOL_SNAPSHOT/scan-python.sh"
	chmod 400 "$TOOL_SNAPSHOT/libsodium.so.26" "$TOOL_SNAPSHOT/cleanroom.sh" \
		"$TOOL_SNAPSHOT/inventory.toml" "$TOOL_SNAPSHOT/text.pdf"
	BIN=$TOOL_SNAPSHOT/solstone-distribution
	SELF=$TOOL_SNAPSHOT/cleanroom.sh
	INVENTORY=$TOOL_SNAPSHOT/inventory.toml
	SCAN_SH=$TOOL_SNAPSHOT/scan-python.sh
	PDF_FIXTURE=$TOOL_SNAPSHOT/text.pdf
	MINISIGN_BIN=$TOOL_SNAPSHOT/minisign
	MINISIGN_LIB=$TOOL_SNAPSHOT/libsodium.so.26
	INSTALL_SH=$TOOL_SNAPSHOT/install.sh
	[ "$(sha256sum "$MINISIGN_BIN" | awk '{print $1}')" = "$EXPECTED_MINISIGN_SHA256" ] \
		|| refuse "snapshotted minisign digest changed"
	[ "$(sha256sum "$MINISIGN_LIB" | awk '{print $1}')" = "$EXPECTED_MINISIGN_LIB_SHA256" ] \
		|| refuse "snapshotted minisign library digest changed"
	installer_sha256=$(sha256sum "$INSTALL_SH") || refuse "could not hash exact installer"
	installer_sha256=${installer_sha256%% *}
	[ "$installer_sha256" = "$expected_installer_sha256" ] \
		|| refuse "artifact-source installer digest does not match the explicit pin"
	producer_sha256=$(sha256sum "$BIN") || refuse "could not hash qualification producer"
	producer_sha256=${producer_sha256%% *}
	[ "$producer_sha256" = "$expected_producer_sha256" ] \
		|| refuse "qualification producer digest does not match the explicit pin"
	cleanroom_sha256=$(sha256sum "$SELF") || refuse "could not hash cleanroom script"
	cleanroom_sha256=${cleanroom_sha256%% *}
	inventory_sha256=$(sha256sum "$INVENTORY") || refuse "could not hash cleanroom inventory"
	inventory_sha256=${inventory_sha256%% *}
	scanner_sha256=$(sha256sum "$SCAN_SH") || refuse "could not hash Python scanner"
	scanner_sha256=${scanner_sha256%% *}
	pdf_fixture_sha256=$(sha256sum "$PDF_FIXTURE") || refuse "could not hash PDF fixture"
	pdf_fixture_sha256=${pdf_fixture_sha256%% *}
}

verify_artifact_set() {
	set -- "$ARTIFACTS"/*.release
	release=$1
	base=${release##*/}
	base=${base%.release}
	sha=$ARTIFACTS/$base.sha256
	manifest=$ARTIFACTS/$base.manifest.json
	[ "$(awk 'NF { count++ } END { print count + 0 }' "$sha")" -eq 4 ] \
		|| refuse "checksum sidecar must name exactly three archives and the release declaration"
	for suffix in .tar.gz .deb .rpm .release; do
		name=$base$suffix
		line=$(awk -v name="$name" '$2 == name && NF == 2 { print $1 }' "$sha")
		is_lower_hex "$line" 64 || refuse "checksum sidecar missing or invalid: $name"
		[ "$(awk -v name="$name" '$2 == name && NF == 2 { count++ } END { print count + 0 }' "$sha")" -eq 1 ] \
			|| refuse "checksum sidecar duplicates: $name"
		grep -F "\"$name\": \"$line\"" "$manifest" >/dev/null \
			|| refuse "manifest checksum mismatch: $name"
	done
	sha_digest=$(sha256sum "$sha") || refuse "could not hash checksum sidecar"
	sha_digest=${sha_digest%% *}
	grep -F "\"$base.sha256\": \"$sha_digest\"" "$manifest" >/dev/null \
		|| refuse "manifest checksum mismatch: $base.sha256"
	[ "$(grep -c '^    "' "$manifest")" -eq 5 ] \
		|| refuse "manifest must name exactly three archives, the release declaration, and the checksum sidecar"
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
	artifact_tar_sha256=$(sha256sum "$ARTIFACTS/$base.tar.gz")
	artifact_tar_sha256=${artifact_tar_sha256%% *}
	artifact_deb_sha256=$(sha256sum "$ARTIFACTS/$base.deb")
	artifact_deb_sha256=${artifact_deb_sha256%% *}
	artifact_rpm_sha256=$(sha256sum "$ARTIFACTS/$base.rpm")
	artifact_rpm_sha256=${artifact_rpm_sha256%% *}
	artifact_release_sha256=$(sha256sum "$release")
	artifact_release_sha256=${artifact_release_sha256%% *}
	artifact_sha256_sha256=$(sha256sum "$sha")
	artifact_sha256_sha256=${artifact_sha256_sha256%% *}
	artifact_manifest_sha256=$(sha256sum "$manifest")
	artifact_manifest_sha256=${artifact_manifest_sha256%% *}
	artifact_manifest_signature_sha256=$(sha256sum "$ARTIFACTS/$base.manifest.json.minisig")
	artifact_manifest_signature_sha256=${artifact_manifest_signature_sha256%% *}
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
		-v "$ARTIFACTS:/artifacts:ro,Z" \
		-v "$TOOL_SNAPSHOT:/qualification-tools:ro,Z" \
		"$ref" sh /qualification-tools/cleanroom.sh --inside "$role"
	printf 'rung=%s subject=%s digest=%s\n' "$role" "$id" "$digest" >>"$RECEIPT_PARTIAL"
}

host_main() {
	trap cleanup_host 0 1 2 15
	out=${1:-${SOLSTONE_DISTRIBUTION_OUT:-/var/tmp/solstone-distribution-out}}
	case $out in
	*/linux-x86_64) ARTIFACTS=$out ;;
	*) ARTIFACTS=$out/linux-x86_64 ;;
	esac
	[ -d "$ARTIFACTS" ] || refuse "artifact directory missing: $ARTIFACTS"
	if [ "$BIN_EXPLICIT" = no ]; then
		cargo build --manifest-path "$ROOT/core/Cargo.toml" -p solstone-core-distribution \
			--bin solstone-distribution --locked --offline
	else
		[ -x "$BIN" ] || refuse "producer binary is not executable: $BIN"
	fi
	RUNTIME=${SOLSTONE_CONTAINER_RUNTIME:-docker}
	builder=${SOLSTONE_CLEANROOM_BUILDER_ID:-}
	case $builder in
	sha256:*) ;;
	*) refuse "SOLSTONE_CLEANROOM_BUILDER_ID must be an immutable sha256 image ID" ;;
	esac
	is_lower_hex "${builder#sha256:}" 64 \
		|| refuse "SOLSTONE_CLEANROOM_BUILDER_ID must be an immutable sha256 image ID"
	snapshot_artifacts
	verify_artifact_set
	snapshot_qualification_tools
	# cleanroom-plan validates repository-relative payload sources, so invoke the
	# already-pinned producer and inventory from their repository paths. The
	# producer and original inventory are rehashed immediately
	# afterward to close the mutation window before any subject runs.
	PLAN=$($SAVED_BIN cleanroom-plan "$SAVED_INVENTORY")
	[ "$(sha256sum "$SAVED_BIN" | awk '{print $1}')" = "$producer_sha256" ] \
		|| refuse "qualification producer changed while constructing the plan"
	[ "$(sha256sum "$SAVED_INVENTORY" | awk '{print $1}')" = "$inventory_sha256" ] \
		|| refuse "cleanroom inventory changed while constructing the plan"
	printf '%s\n' "$PLAN" | grep -F 'SUBJECT python-3.12-control ' >/dev/null \
		|| refuse "Python control is absent from the cleanroom plan"
	receipt=${SOLSTONE_CLEANROOM_RECEIPT:-/var/tmp/solstone-distribution-cleanroom.receipt}
	case $receipt in
	*/*)
		receipt_dir=${receipt%/*}
		receipt_name=${receipt##*/}
		[ -n "$receipt_dir" ] || receipt_dir=/
		;;
	*)
		receipt_dir=.
		receipt_name=$receipt
		;;
	esac
	[ -d "$receipt_dir" ] || refuse "receipt directory is missing: $receipt_dir"
	RECEIPT_PARTIAL=$(mktemp "$receipt_dir/.${receipt_name}.partial.XXXXXX") \
		|| refuse "could not create exclusive partial receipt in $receipt_dir"
	chmod 600 "$RECEIPT_PARTIAL"
	{
		printf 'schema=solstone-distribution-cleanroom-v1\n'
		printf 'commit=%s\n' "$commit"
		printf 'lock_sha256=%s\n' "$lock"
		printf 'builder_image=%s\n' "$builder"
		printf 'minisign_sha256=%s\n' "$minisign_sha256"
		printf 'minisign_lib_sha256=%s\n' "$minisign_lib_sha256"
		printf 'installer_sha256=%s\n' "$installer_sha256"
		printf 'producer_sha256=%s\n' "$producer_sha256"
		printf 'cleanroom_sha256=%s\n' "$cleanroom_sha256"
		printf 'inventory_sha256=%s\n' "$inventory_sha256"
		printf 'scanner_sha256=%s\n' "$scanner_sha256"
		printf 'pdf_fixture_sha256=%s\n' "$pdf_fixture_sha256"
		printf 'artifact_tar_sha256=%s\n' "$artifact_tar_sha256"
		printf 'artifact_deb_sha256=%s\n' "$artifact_deb_sha256"
		printf 'artifact_rpm_sha256=%s\n' "$artifact_rpm_sha256"
		printf 'artifact_release_sha256=%s\n' "$artifact_release_sha256"
		printf 'artifact_checksum_sha256=%s\n' "$artifact_sha256_sha256"
		printf 'artifact_manifest_sha256=%s\n' "$artifact_manifest_sha256"
		printf 'artifact_manifest_signature_sha256=%s\n' \
			"$artifact_manifest_signature_sha256"
	} >"$RECEIPT_PARTIAL"
	run_subject python-3.12-control scan-control
	run_subject debian-bookworm-no-python tar
	run_subject debian-bookworm-no-python deb
	run_subject fedora-42-no-python rpm
	run_subject fedora-42-no-python bootstrap
	run_subject debian-bookworm-no-python talent
	run_subject debian-bookworm-no-python speakers
	run_subject debian-bookworm-no-python pdf
	if ln "$RECEIPT_PARTIAL" "$receipt" 2>/dev/null; then
		rm -f "$RECEIPT_PARTIAL"
	elif [ -f "$receipt" ] && [ ! -L "$receipt" ] \
		&& [ "$(stat -c %u "$receipt")" = "$(id -u)" ] \
		&& [ "$(stat -c %a "$receipt")" = 600 ] \
		&& cmp -s "$RECEIPT_PARTIAL" "$receipt"; then
		rm -f "$RECEIPT_PARTIAL"
	else
		refuse "receipt conflict: $receipt"
	fi
	RECEIPT_PARTIAL=
	cleanup_host
	trap - 0 1 2 15
	printf 'cleanroom=ok receipt=%s\n' "$receipt"
}

self_test() {
	test_root=/var/tmp/solstone-cleanroom-self-test-$$
	trap 'find "$test_root" -depth -delete 2>/dev/null || true' 0 1 2 15
	mkdir -p "$test_root/artifacts/linux-x86_64"
	mkdir -p "$test_root/source/core/distribution"
	git -C "$test_root/source" init -q
	git -C "$test_root/source" config user.name cleanroom-self-test
	git -C "$test_root/source" config user.email cleanroom-self-test@invalid
	printf '%s\n' '#!/bin/sh' 'printf original-installer\\n' \
		>"$test_root/source/core/distribution/install.sh"
	git -C "$test_root/source" add core/distribution/install.sh
	git -C "$test_root/source" commit -q -m original
	fixture_commit=$(git -C "$test_root/source" rev-parse HEAD)
	GIT_NO_REPLACE_OBJECTS=1 git -C "$test_root/source" \
		show "$fixture_commit:core/distribution/install.sh" \
		>"$test_root/original-install.sh" \
		|| refuse "replace-ref control could not materialize original installer"
	expected_installer_sha256=$(sha256sum "$test_root/original-install.sh")
	expected_installer_sha256=${expected_installer_sha256%% *}
	printf '%s\n' '#!/bin/sh' 'printf replacement-installer\\n' \
		>"$test_root/source/core/distribution/install.sh"
	git -C "$test_root/source" add core/distribution/install.sh
	git -C "$test_root/source" commit -q -m replacement
	replacement_commit=$(git -C "$test_root/source" rev-parse HEAD)
	git -C "$test_root/source" replace "$fixture_commit" "$replacement_commit"
	git -C "$test_root/source" show "$fixture_commit:core/distribution/install.sh" \
		>"$test_root/replaced-install.sh" \
		|| refuse "replace-ref control could not materialize substituted installer"
	unsafe_installer_sha256=$(sha256sum "$test_root/replaced-install.sh")
	unsafe_installer_sha256=${unsafe_installer_sha256%% *}
	[ "$unsafe_installer_sha256" != "$expected_installer_sha256" ] \
		|| refuse "replace-ref negative control did not substitute installer bytes"
	base=solstone-journal-1.0.22-linux-x86_64
	artifact_dir=$test_root/artifacts/linux-x86_64
	for suffix in .tar.gz .deb .rpm; do
		: >"$artifact_dir/$base$suffix"
	done
	printf '%s\n' 'product=solstone-journal' 'version=1.0.22' 'target=linux-x86_64' \
		"commit=$fixture_commit" \
		'lock_sha256=0000000000000000000000000000000000000000000000000000000000000001' \
		'upgrade_epoch=journal-v2' \
		'retention_window=3' \
		'min_bootstrap_revision=1' \
		>"$artifact_dir/$base.release"
	(cd "$artifact_dir" && sha256sum "$base.tar.gz" "$base.deb" "$base.rpm" \
		"$base.release" >"$base.sha256")
	archive_digest=$(sha256sum "$artifact_dir/$base.tar.gz" | awk '{ print $1 }')
	release_digest=$(sha256sum "$artifact_dir/$base.release" | awk '{ print $1 }')
	sha_digest=$(sha256sum "$artifact_dir/$base.sha256" | awk '{ print $1 }')
	printf '%s\n' \
		'{' \
		'  "product": "solstone-journal",' \
		'  "version": "1.0.22",' \
		'  "target": "linux-x86_64",' \
		'  "files": {' \
		"    \"$base.tar.gz\": \"$archive_digest\"," \
		"    \"$base.deb\": \"$archive_digest\"," \
		"    \"$base.rpm\": \"$archive_digest\"," \
		"    \"$base.release\": \"$release_digest\"," \
		"    \"$base.sha256\": \"$sha_digest\"" \
		'  }' \
		'}' >"$artifact_dir/$base.manifest.json"
	: >"$artifact_dir/$base.manifest.json.minisig"
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
	artifact_source=
	for arg in "$@"; do
		[ "$arg" = --pull=never ] && seen_pull=yes
		[ "$arg" = --network=none ] && seen_network=yes
		case $arg in
		*:/artifacts:ro,Z) artifact_source=${arg%:/artifacts:ro,Z} ;;
		esac
		role=$arg
	done
	[ "$seen_pull" = yes ] && [ "$seen_network" = yes ] || exit 91
	if [ "$role" = scan-control ] && [ -n "${FAKE_MUTATE_ORIGINAL:-}" ]; then
		printf mutation >>"$FAKE_MUTATE_ORIGINAL"
		actual=$(sha256sum "$artifact_source/${FAKE_ARTIFACT_BASENAME}.tar.gz")
		actual=${actual%% *}
		[ "$actual" = "$FAKE_EXPECT_ARTIFACT_SHA256" ] || exit 93
	fi
	printf '%s\n' "$role" >>"$FAKE_LOG"
	[ "${FAKE_FAIL_ROLE:-}" != "$role" ] || exit 41
	if [ "$role" = pdf ] && [ -n "${FAKE_BARRIER_DIR:-}" ]; then
		: >"$FAKE_BARRIER_DIR/$FAKE_PARTICIPANT"
		attempt=0
		while :; do
			set -- "$FAKE_BARRIER_DIR"/*
			[ "$#" -ge 2 ] && [ -e "$2" ] && break
			attempt=$((attempt + 1))
			[ "$attempt" -lt 30 ] || exit 94
			sleep 1
		done
	fi
	;;
*) exit 92 ;;
esac
EOF
	chmod +x "$test_root/runtime"
	printf '%s\n' '#!/bin/sh' 'exit 0' >"$test_root/minisign"
	chmod +x "$test_root/minisign"
	: >"$test_root/libsodium.so.26"
	FAKE_LOG=$test_root/runtime.log
	SOLSTONE_CONTAINER_RUNTIME=$test_root/runtime
	SOLSTONE_CLEANROOM_BUILDER_ID=sha256:1111111111111111111111111111111111111111111111111111111111111111
	SOLSTONE_CLEANROOM_MINISIGN_BIN=$test_root/minisign
	SOLSTONE_CLEANROOM_MINISIGN_LIB=$test_root/libsodium.so.26
	SOLSTONE_CLEANROOM_RECEIPT=$test_root/receipt
	SOLSTONE_CLEANROOM_SOURCE_REPO=$test_root/source
	SOLSTONE_CLEANROOM_INSTALLER_SHA256=$expected_installer_sha256
	if [ "$BIN_EXPLICIT" = no ] || [ ! -x "$BIN" ]; then
		cargo build --manifest-path "$ROOT/core/Cargo.toml" -p solstone-core-distribution \
			--bin solstone-distribution --locked --offline
	fi
	SOLSTONE_CLEANROOM_PRODUCER_SHA256=$(sha256sum "$BIN" | awk '{print $1}')
	EXPECTED_MINISIGN_SHA256=$(sha256sum "$test_root/minisign" | awk '{print $1}')
	EXPECTED_MINISIGN_LIB_SHA256=$(sha256sum "$test_root/libsodium.so.26" | awk '{print $1}')
	export FAKE_LOG SOLSTONE_CONTAINER_RUNTIME SOLSTONE_CLEANROOM_BUILDER_ID
	export SOLSTONE_CLEANROOM_MINISIGN_BIN SOLSTONE_CLEANROOM_MINISIGN_LIB
	export SOLSTONE_CLEANROOM_RECEIPT
	export SOLSTONE_CLEANROOM_PRODUCER_SHA256 SOLSTONE_CLEANROOM_INSTALLER_SHA256
	export SOLSTONE_CLEANROOM_SOURCE_REPO
	FAKE_MUTATE_ORIGINAL=$artifact_dir/$base.tar.gz
	FAKE_ARTIFACT_BASENAME=$base
	FAKE_EXPECT_ARTIFACT_SHA256=$archive_digest
	export FAKE_MUTATE_ORIGINAL FAKE_ARTIFACT_BASENAME FAKE_EXPECT_ARTIFACT_SHA256
	(host_main "$test_root/artifacts") >/dev/null
	grep -Fqx "artifact_tar_sha256=$archive_digest" "$test_root/receipt" \
		|| refuse "receipt did not bind the snapshotted tar digest"
	cp "$test_root/receipt" "$test_root/existing-receipt-target"
	chmod 600 "$test_root/existing-receipt-target"
	[ "$(sha256sum "$artifact_dir/$base.tar.gz" | awk '{print $1}')" != "$archive_digest" ] \
		|| refuse "post-verification mutation control did not mutate the source artifact"
	: >"$artifact_dir/$base.tar.gz"
	unset FAKE_MUTATE_ORIGINAL FAKE_ARTIFACT_BASENAME FAKE_EXPECT_ARTIFACT_SHA256
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
	set +e
	(set -e; FAKE_FAIL_ROLE=deb; export FAKE_FAIL_ROLE; host_main "$test_root/artifacts") \
		>/dev/null 2>&1
	failure_status=$?
	set -e
	[ "$failure_status" -ne 0 ] || refuse "fake runtime failure was accepted"
	[ ! -e "$test_root/receipt" ] || refuse "failed run published a receipt"

	SOLSTONE_CLEANROOM_RECEIPT=$test_root/membership-receipt
	export SOLSTONE_CLEANROOM_RECEIPT
	mv "$artifact_dir/$base.manifest.json.minisig" "$test_root/missing.minisig"
	set +e
	(host_main "$test_root/artifacts") >/dev/null 2>&1
	missing_status=$?
	set -e
	[ "$missing_status" -ne 0 ] || refuse "missing manifest signature was accepted"
	[ ! -e "$SOLSTONE_CLEANROOM_RECEIPT" ] \
		|| refuse "missing-signature run published a receipt"
	mv "$test_root/missing.minisig" "$artifact_dir/$base.manifest.json.minisig"

	: >"$artifact_dir/unexpected-member"
	set +e
	(host_main "$test_root/artifacts") >/dev/null 2>&1
	extra_status=$?
	set -e
	[ "$extra_status" -ne 0 ] || refuse "unexpected artifact member was accepted"
	[ ! -e "$SOLSTONE_CLEANROOM_RECEIPT" ] \
		|| refuse "unexpected-member run published a receipt"
	rm "$artifact_dir/unexpected-member"

	mv "$artifact_dir/$base.deb" "$test_root/original.deb"
	ln -s "$base.tar.gz" "$artifact_dir/$base.deb"
	set +e
	(host_main "$test_root/artifacts") >/dev/null 2>&1
	symlink_status=$?
	set -e
	[ "$symlink_status" -ne 0 ] || refuse "symlink artifact member was accepted"
	[ ! -e "$SOLSTONE_CLEANROOM_RECEIPT" ] \
		|| refuse "symlink-member run published a receipt"
	rm "$artifact_dir/$base.deb"
	mv "$test_root/original.deb" "$artifact_dir/$base.deb"

	SOLSTONE_CLEANROOM_RECEIPT=$test_root/symlink-receipt
	export SOLSTONE_CLEANROOM_RECEIPT
	ln -s "$test_root/existing-receipt-target" "$SOLSTONE_CLEANROOM_RECEIPT"
	set +e
	(host_main "$test_root/artifacts") >/dev/null 2>&1
	receipt_symlink_status=$?
	set -e
	[ "$receipt_symlink_status" -eq 2 ] \
		|| refuse "matching pre-existing receipt symlink did not refuse exactly"
	[ -L "$SOLSTONE_CLEANROOM_RECEIPT" ] \
		|| refuse "receipt symlink refusal changed the destination"
	grep -Fqx "builder_image=$SOLSTONE_CLEANROOM_BUILDER_ID" \
		"$test_root/existing-receipt-target" \
		|| refuse "receipt symlink negative control did not retain matching bytes"
	rm "$SOLSTONE_CLEANROOM_RECEIPT"

	SOLSTONE_CLEANROOM_RECEIPT=$test_root/membership-receipt
	export SOLSTONE_CLEANROOM_RECEIPT
	mkdir "$test_root/publication-barrier"
	set +e
	(
		FAKE_LOG=$test_root/concurrent-a.log
		FAKE_BARRIER_DIR=$test_root/publication-barrier
		FAKE_PARTICIPANT=a
		SOLSTONE_CLEANROOM_BUILDER_ID=sha256:1111111111111111111111111111111111111111111111111111111111111111
		export FAKE_LOG FAKE_BARRIER_DIR FAKE_PARTICIPANT SOLSTONE_CLEANROOM_BUILDER_ID
		host_main "$test_root/artifacts"
	) >/dev/null 2>&1 &
	first_pid=$!
	(
		FAKE_LOG=$test_root/concurrent-b.log
		FAKE_BARRIER_DIR=$test_root/publication-barrier
		FAKE_PARTICIPANT=b
		SOLSTONE_CLEANROOM_BUILDER_ID=sha256:2222222222222222222222222222222222222222222222222222222222222222
		export FAKE_LOG FAKE_BARRIER_DIR FAKE_PARTICIPANT SOLSTONE_CLEANROOM_BUILDER_ID
		host_main "$test_root/artifacts"
	) >/dev/null 2>&1 &
	second_pid=$!
	wait "$first_pid"
	first_status=$?
	wait "$second_pid"
	second_status=$?
	set -e
	case $first_status:$second_status in
	0:2|2:0) ;;
	*) refuse "concurrent conflicting publishers did not return exact statuses {0,2}: $first_status,$second_status" ;;
	esac
	[ -f "$SOLSTONE_CLEANROOM_RECEIPT" ] \
		|| refuse "concurrent publisher did not publish one complete receipt"
	[ "$(grep -c '^builder_image=' "$SOLSTONE_CLEANROOM_RECEIPT")" -eq 1 ] \
		|| refuse "concurrent publisher produced an incomplete receipt"
	[ -z "$(find "$test_root" -maxdepth 1 -name '.membership-receipt.partial.*' -print -quit)" ] \
		|| refuse "concurrent publisher left a partial receipt"
	printf 'cleanroom-self-test=ok\n'
}

case ${1:-} in
--self-test) self_test ;;
*) host_main "$@" ;;
esac
