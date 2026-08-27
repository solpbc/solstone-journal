#!/usr/bin/env bash
# Run a single solstone install-integration test inside the
# solstone-systemd-test container.
#
# Usage:
#   ./run-test.sh                          # default: smoke (verify systemd --user only)
#   ./run-test.sh smoke                    # tiny user unit, no solstone install
#   ./run-test.sh install [extra-args]     # full: uv tool host install, then journal setup
#   ./run-test.sh observer-ingest          # install + setup + real observer ingest round-trip
#   ./run-test.sh legacy-upgrade           # install, but seed a legacy non-symlink
#                                          #   wrapper first; assert setup self-heals it
#                                          #   through to a healthy service_identity
#   ./run-test.sh shell                    # leave container up + drop into a user shell
#
# Environment overrides:
#   IMAGE       — image tag (default: solstone-systemd-test:latest)
#   CONTAINER   — container name (default: solstone-systemd-test-run)
#   TEST_USER   — non-root user in the image (default: solstone)
#   PRIVILEGED  — "1" (default) for --privileged, "0" for the less-privileged path
#   KEEP        — "1" to keep the container after the run for inspection
#   SOLSTONE_DIST_DIR — host directory of produced linux-x86_64 artifacts
#                       (solstone-journal-*-linux-x86_64.deb). Mounted read-only
#                       at /artifacts. Required for install / observer-ingest /
#                       legacy-upgrade. The wheel path is retired.
#
# Exit codes:
#   0  test passed
#   1  test failed (specific failure printed to stderr)
#   2  usage error or pre-flight failure
#
# The "install" mode passes `--skip-models --skip-skills` to `journal setup` by
# default — the systemd-runner is meant to verify the service-install path,
# not the model-installer or skill-installer. Override with `install full`
# to drop those flags.

set -euo pipefail

IMAGE="${IMAGE:-solstone-systemd-test:latest}"
CONTAINER="${CONTAINER:-solstone-systemd-test-run}"
TEST_USER="${TEST_USER:-solstone}"
PRIVILEGED="${PRIVILEGED:-1}"
KEEP="${KEEP:-0}"
SOLSTONE_DIST_DIR="${SOLSTONE_DIST_DIR:-}"

mode="${1:-smoke}"
shift || true

die() { echo "error: $*" >&2; exit 2; }
log() { echo "[$(date -u +%H:%M:%S)] $*" >&2; }

case "$mode" in
    smoke|install|observer-ingest|legacy-upgrade|shell) ;;
    *) die "unknown mode: $mode (expected: smoke | install | observer-ingest | legacy-upgrade | shell)" ;;
esac

command -v docker >/dev/null || die "docker not found in PATH"

# Image is built by `make build` — fail fast if it isn't there yet.
docker image inspect "$IMAGE" >/dev/null 2>&1 || \
    die "image $IMAGE not found; run 'make build' first"

# Clean any stale container from a previous run (KEEP=1 leaves it on success).
docker rm -f "$CONTAINER" >/dev/null 2>&1 || true

# Privilege flags. --privileged is the simple, reliable path. The less-
# privileged path (PRIVILEGED=0) uses cgroup-v2 host namespace + SYS_ADMIN,
# which works on modern Docker + cgroup-v2 hosts but is more host-sensitive.
if [ "$PRIVILEGED" = "1" ]; then
    PRIV_FLAGS=(--privileged --tmpfs /tmp --tmpfs /run --tmpfs /run/lock)
else
    PRIV_FLAGS=(
        --cap-add SYS_ADMIN
        --security-opt apparmor=unconfined
        --cgroupns=host
        --tmpfs /tmp
        --tmpfs /run
        --tmpfs /run/lock
        -v /sys/fs/cgroup:/sys/fs/cgroup:rw
    )
fi

RUN_FLAGS=("${PRIV_FLAGS[@]}")
if [ -n "$SOLSTONE_DIST_DIR" ]; then
    [ -d "$SOLSTONE_DIST_DIR" ] || die "SOLSTONE_DIST_DIR is not a directory: $SOLSTONE_DIST_DIR"
    compgen -G "$SOLSTONE_DIST_DIR/solstone-journal-*-linux-x86_64.deb" >/dev/null \
        || die "SOLSTONE_DIST_DIR has no solstone-journal-*-linux-x86_64.deb: $SOLSTONE_DIST_DIR"
    RUN_FLAGS+=(-v "$SOLSTONE_DIST_DIR:/artifacts:ro")
elif [ "$mode" != "smoke" ] && [ "$mode" != "shell" ]; then
    die "SOLSTONE_DIST_DIR is required for $mode (produced linux-x86_64 .deb)"
fi

install_solstone_cmd='
set -euo pipefail
deb=$(ls /artifacts/solstone-journal-*-linux-x86_64.deb | sort -V | tail -1)
echo "install target: ${deb}"
sudo apt-get update -qq
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "$deb"
'

log "starting $CONTAINER from $IMAGE (privileged=$PRIVILEGED, mode=$mode)"
docker run -d --rm --name "$CONTAINER" "${RUN_FLAGS[@]}" "$IMAGE" >/dev/null

cleanup() {
    rc=$?
    if [ "$KEEP" = "1" ] && [ "$rc" = "0" ]; then
        log "KEEP=1: leaving $CONTAINER running for inspection"
        log "  docker exec -u $TEST_USER -it $CONTAINER bash -l"
        log "  docker rm -f $CONTAINER"
    else
        docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    fi
    exit $rc
}
trap cleanup EXIT

# Wait for PID 1 systemd to reach a running/degraded steady state. We
# accept "degraded" because some masked units may be reported as failed-by-
# masking; the user-level path doesn't depend on them.
log "waiting for system manager to reach steady state..."
for _ in $(seq 1 60); do
    state=$(docker exec "$CONTAINER" systemctl is-system-running 2>/dev/null || true)
    case "$state" in
        running|degraded) break ;;
    esac
    sleep 1
done
if [ "$state" != "running" ] && [ "$state" != "degraded" ]; then
    log "system manager never became ready (last state: ${state:-unknown})"
    docker exec "$CONTAINER" systemctl --failed --no-pager || true
    exit 1
fi
log "system manager: $state"

# Start the user instance manager. systemd-logind would normally read the
# linger marker file we baked into the image and fire user@<uid>.service
# automatically, but logind is one of the units we mask (it pulls in seat
# / TTY / utmp logic that doesn't apply in a container). So we start the
# user manager explicitly — same end state, just deterministic.
USER_UID=$(docker exec "$CONTAINER" id -u "$TEST_USER")
log "starting user@${USER_UID}.service..."
docker exec "$CONTAINER" systemctl start "user@${USER_UID}.service" \
    || { log "failed to start user@${USER_UID}.service"; exit 1; }

# Wait for the user instance to accept commands.
log "waiting for user@${USER_UID}..."
for _ in $(seq 1 30); do
    if docker exec -u "$TEST_USER" "$CONTAINER" bash -lc 'systemctl --user list-units --no-pager >/dev/null 2>&1'; then
        break
    fi
    sleep 1
done
docker exec -u "$TEST_USER" "$CONTAINER" bash -lc 'systemctl --user list-units --no-pager >/dev/null' \
    || { log "user systemd never became responsive"; exit 1; }
log "user systemd: ready"

case "$mode" in
    smoke)
        log "smoke test: install a tiny --user unit and verify it starts"
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc '
            set -euo pipefail
            mkdir -p ~/.config/systemd/user
            cat > ~/.config/systemd/user/runner-smoke.service <<UNIT
[Unit]
Description=solstone-systemd-test smoke unit

[Service]
Type=oneshot
ExecStart=/bin/sh -c "echo runner-smoke-ok > /tmp/runner-smoke.out"
RemainAfterExit=yes

[Install]
WantedBy=default.target
UNIT
            systemctl --user daemon-reload
            systemctl --user enable --now runner-smoke.service
            test "$(cat /tmp/runner-smoke.out)" = "runner-smoke-ok"
            systemctl --user is-active runner-smoke.service
        '
        log "smoke: PASS"
        ;;

    install)
        # Heavy: pulls solstone wheel + transitive deps. --skip-models /
        # --skip-skills cuts faster-whisper / Parakeet / Claude-skill
        # downloads — those are orthogonal to the systemd integration we're
        # testing here. Pass "full" as an extra arg to drop both flags.
        extra=("$@")
        skip_flags=(--skip-models --skip-skills)
        if [ "${1:-}" = "full" ]; then
            skip_flags=()
            extra=("${extra[@]:1}")
        fi

        log "install: apt install solstone-journal .deb"
        # Debian bookworm ships python 3.11; solstone 0.4.0+ requires >=3.12.
        # uv downloads a standalone 3.12 on the fly when requested explicitly.
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc "$install_solstone_cmd"

        log "install: journal setup -y ${skip_flags[*]} ${extra[*]:-}"
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc \
            "journal setup -y ${skip_flags[*]} ${extra[*]:-}"

        log "verify: unit file exists and was loaded by user systemd"
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc '
            set -euo pipefail
            test -f ~/.config/systemd/user/solstone.service
            systemctl --user cat solstone.service >/dev/null
        '

        log "verify: systemctl --user is-active solstone"
        # 30s budget for Type=notify READY=1.
        for _ in $(seq 1 30); do
            state=$(docker exec -u "$TEST_USER" "$CONTAINER" \
                bash -lc 'systemctl --user is-active solstone' 2>/dev/null || true)
            [ "$state" = "active" ] && break
            sleep 1
        done
        if [ "$state" != "active" ]; then
            log "solstone.service did not reach active (last: ${state:-unknown})"
            docker exec -u "$TEST_USER" "$CONTAINER" \
                bash -lc 'systemctl --user status solstone --no-pager -l || true' >&2
            exit 1
        fi

        log "verify: journal service status"
        # `journal service status` returns 0 only when the callosum health probe
        # succeeds — this is the authoritative readiness signal. Port 5015
        # is the plain-HTTP convey Flask app (login, /init, /app/today);
        # 7657 is the mutual-TLS pairing/sync surface. Neither exposes an
        # explicit /health route — `journal service status` (callosum.sock) is
        # the canonical probe.
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc 'journal service status'

        log "install: PASS"
        ;;

    observer-ingest)
        log "observer-ingest: apt install solstone-journal .deb"
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc "$install_solstone_cmd"

        log "observer-ingest: journal setup -y --skip-models --skip-skills"
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc \
            "journal setup -y --skip-models --skip-skills"

        log "verify: systemctl --user is-active solstone"
        for _ in $(seq 1 30); do
            state=$(docker exec -u "$TEST_USER" "$CONTAINER" \
                bash -lc 'systemctl --user is-active solstone' 2>/dev/null || true)
            [ "$state" = "active" ] && break
            sleep 1
        done
        if [ "$state" != "active" ]; then
            log "solstone.service did not reach active (last: ${state:-unknown})"
            docker exec -u "$TEST_USER" "$CONTAINER" \
                bash -lc 'systemctl --user status solstone --no-pager -l || true' >&2
            exit 1
        fi

        log "verify: journal service status"
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc 'journal service status'

        log "observer-ingest: register loopback observer and post one segment"
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc '
            set -euo pipefail

            base_url="http://127.0.0.1:5015"
            day="$(date -u +%Y%m%d)"
            segment="120000_1"
            host="systemd-gate"

            register_body=$(curl -fsS \
                -H "Content-Type: application/json" \
                -d "{\"platform\":\"linux\",\"hostname\":\"${host}\",\"stream_type\":\"tmux\",\"version\":\"systemd-test\"}" \
                "${base_url}/app/observer/register")
            printf "%s\n" "$register_body" > /tmp/observer-register.json

            key=$(python3 - <<PY
import json
data = json.load(open("/tmp/observer-register.json", encoding="utf-8"))
assert data.get("key"), data
print(data["key"])
PY
)
            stream=$(python3 - <<PY
import json
data = json.load(open("/tmp/observer-register.json", encoding="utf-8"))
assert data.get("name"), data
print(data["name"])
PY
)

            payload=$(mktemp)
            cat > "$payload" <<JSONL
{"raw":"screen.webm"}
{"timestamp":0,"analysis":{"visible":"release-gate","visual_description":"observer ingest smoke"}}
JSONL

            code=$(curl -sS -o /tmp/observer-ingest-response.json -w "%{http_code}" \
                -H "Authorization: Bearer ${key}" \
                -F "day=${day}" \
                -F "segment=${segment}" \
                -F "host=${host}" \
                -F "platform=linux" \
                -F "meta={\"source\":\"systemd-test\"}" \
                -F "files=@${payload};filename=screen.jsonl;type=application/x-ndjson" \
                "${base_url}/app/observer/ingest")
            if [ "$code" != "200" ]; then
                echo "observer ingest returned HTTP ${code}" >&2
                cat /tmp/observer-ingest-response.json >&2 || true
                exit 1
            fi

            saved_segment=$(python3 - <<PY
import json
data = json.load(open("/tmp/observer-ingest-response.json", encoding="utf-8"))
assert data.get("status") in {"ok", "collision"}, data
assert data.get("files") == ["screen.jsonl"], data
print(data["segment"])
PY
)

            segment_dir="${HOME}/journal/chronicle/${day}/${stream}/${saved_segment}"
            test -s "${segment_dir}/screen.jsonl"
            test -s "${segment_dir}/stream.json"
            python3 - "$segment_dir" "$stream" <<PY
import json
import pathlib
import sys

segment_dir = pathlib.Path(sys.argv[1])
stream = sys.argv[2]
stream_doc = json.loads((segment_dir / "stream.json").read_text(encoding="utf-8"))
assert stream_doc.get("stream") == stream, stream_doc
assert stream_doc.get("seq") == 1, stream_doc
PY

            observed="no"
            for _ in $(seq 1 30); do
                curl -fsS \
                    -H "Authorization: Bearer ${key}" \
                    -H "X-Solstone-Protocol-Version: 2" \
                    "${base_url}/app/observer/ingest/segments/${day}" \
                    > /tmp/observer-segments.json
                if python3 - "$saved_segment" <<PY
import json
import sys

segment = sys.argv[1]
body = json.load(open("/tmp/observer-segments.json", encoding="utf-8"))
items = body.get("items", body if isinstance(body, list) else [])
matches = [item for item in items if item.get("key") == segment]
if not matches:
    raise SystemExit(1)
item = matches[0]
files = item.get("files") or []
if not any(f.get("name") == "screen.jsonl" and f.get("status") == "present" for f in files):
    raise SystemExit(1)
if item.get("observed") is not True:
    raise SystemExit(1)
PY
                then
                    observed="yes"
                    break
                fi
                sleep 1
            done

            if [ "$observed" != "yes" ]; then
                echo "observer segment did not reach observed=true" >&2
                echo "register:" >&2
                cat /tmp/observer-register.json >&2 || true
                echo "ingest:" >&2
                cat /tmp/observer-ingest-response.json >&2 || true
                echo "segments:" >&2
                cat /tmp/observer-segments.json >&2 || true
                echo "service log tail:" >&2
                tail -120 "${HOME}/journal/health/service.log" >&2 || true
                exit 1
            fi

            echo "observer-ingest: ${day}/${stream}/${saved_segment} observed=true"
        '

        log "observer-ingest: PASS"
        ;;

    legacy-upgrade)
        # V2-over-v1 crossover cell. Seed exact V1 console scripts plus a live
        # historical systemd service, then prove one setup invocation replaces
        # their runtime authority without changing any pre-existing journal
        # artifact outside setup's closed write set.
        log "legacy-upgrade: apt install solstone-journal .deb"
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc "$install_solstone_cmd"

        log "legacy-upgrade: seed exact V1 launchers, service, manifest, and owner content"
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc '
            set -euo pipefail
            legacy_bin="$HOME/.local/share/uv/tools/solstone/bin"
            public_bin="$HOME/.local/bin"
            mkdir -p "$legacy_bin" "$public_bin" \
                "$HOME/.config/systemd/user" \
                "$HOME/journal/20260826" "$HOME/journal/media" \
                "$HOME/journal/identity/entities" "$HOME/journal/config" \
                "$HOME/journal/health"

            cat > "$legacy_bin/python3" <<'"'"'PYTHON'"'"'
#!/bin/sh
printf "%s\n" "$$" > "$HOME/journal/health/v1.pid"
trap '"'"'if grep -q "^ExecStart=.*/sol supervisor 5015$" "$HOME/.config/systemd/user/solstone.service"; then
    printf "%s\n" "stopped-before-publish" > /tmp/v1-stopped-before-publish
fi
exit 0'"'"' TERM INT
while :; do sleep 1; done
PYTHON
            chmod 755 "$legacy_bin/python3"

            for command in solstone sol; do
                cat > "$legacy_bin/$command" <<WRAPPER
#!$legacy_bin/python3
# -*- coding: utf-8 -*-
import sys
from solstone.think.sol_cli import main
if __name__ == '"'"'__main__'"'"':
    if sys.argv[0].endswith('"'"'-script.pyw'"'"'):
        sys.argv[0] = sys.argv[0][:-11]
    elif sys.argv[0].endswith('"'"'.exe'"'"'):
        sys.argv[0] = sys.argv[0][:-4]
    sys.exit(main())
WRAPPER
                chmod 755 "$legacy_bin/$command"
                ln -sfn "$legacy_bin/$command" "$public_bin/$command"
            done

            printf "%s\n" '"'"'{"owner":"ledger"}'"'"' > "$HOME/journal/ledger.jsonl"
            printf "%s\n" '"'"'{"owner":"dated"}'"'"' > "$HOME/journal/20260826/owner.jsonl"
            printf "%s" '"'"'owner-media-bytes'"'"' > "$HOME/journal/media/owner.bin"
            printf "%s\n" '"'"'# owner entity'"'"' > "$HOME/journal/identity/entities/owner.md"
            printf "%s\n" '"'"'{"owner_setting":true}'"'"' > "$HOME/journal/config/owner.json"
            cat > "$HOME/journal/health/setup-state.json" <<'"'"'MANIFEST'"'"'
{
  "schema_version": 1,
  "started_at": "2026-08-26T00:00:00Z",
  "completed_at": "2026-08-26T00:00:01Z",
  "mode": "non_interactive",
  "args_resolved": {},
  "steps": [
    {"name":"wrapper","status":"ok","paths":[]},
    {"name":"service","status":"ok","paths":[]}
  ]
}
MANIFEST

            cat > "$HOME/.config/systemd/user/solstone.service" <<UNIT
[Unit]
Description=Solstone Supervisor
After=default.target

[Service]
Type=simple
Environment=HOME=$HOME
Environment=PATH=$public_bin:/usr/bin:/bin
ExecStart=$public_bin/sol supervisor 5015
Restart=on-failure
RestartSec=5

[Install]
WantedBy=default.target
UNIT
            systemctl --user daemon-reload
            systemctl --user enable --now solstone.service
            for _ in $(seq 1 50); do
                test -s "$HOME/journal/health/v1.pid" && break
                sleep 0.1
            done
            test -s "$HOME/journal/health/v1.pid"
            cp "$HOME/journal/health/v1.pid" /tmp/v1.pid

            cd "$HOME/journal"
            find . -mindepth 1 \
                ! -path '"'"'./config/journal.json'"'"' \
                ! -path '"'"'./config/journal.json.lock'"'"' \
                ! -path '"'"'./health/setup-state.json'"'"' \
                ! -path '"'"'./.claude/skills'"'"' ! -path '"'"'./.claude/skills/*'"'"' \
                ! -path '"'"'./.agents/skills'"'"' ! -path '"'"'./.agents/skills/*'"'"' \
                -print0 > /tmp/journal-preexisting.paths
            tar --null --no-recursion --files-from=/tmp/journal-preexisting.paths \
                --mtime=@0 --owner=0 --group=0 --numeric-owner \
                -cf /tmp/journal-preexisting.before.tar
        '

        log "legacy-upgrade: one journal setup invocation"
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc \
            'journal setup -y --accept-existing-journal --skip-models --skip-skills'

        log "verify: systemctl --user is-active solstone"
        for _ in $(seq 1 30); do
            state=$(docker exec -u "$TEST_USER" "$CONTAINER" \
                bash -lc 'systemctl --user is-active solstone' 2>/dev/null || true)
            [ "$state" = "active" ] && break
            sleep 1
        done
        if [ "$state" != "active" ]; then
            log "solstone.service did not reach active (last: ${state:-unknown})"
            docker exec -u "$TEST_USER" "$CONTAINER" \
                bash -lc 'systemctl --user status solstone --no-pager -l || true' >&2
            exit 1
        fi

        log "verify: V1 stopped before publication; authority replaced with durable recovery copies"
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc '
            set -euo pipefail
            old_pid=$(cat /tmp/v1.pid)
            ! kill -0 "$old_pid" 2>/dev/null
            test -f /tmp/v1-stopped-before-publish
            new_pid=$(systemctl --user show solstone.service --property=MainPID --value)
            test "$new_pid" -gt 1
            test "$new_pid" != "$old_pid"

            grep -q "^# managed-version:" ~/.local/bin/solstone
            grep -q "^# managed-version:" ~/.local/bin/journal
            test ! -e ~/.local/bin/sol
            test ! -L ~/.local/bin/sol
            test -x ~/.local/share/uv/tools/solstone/bin/solstone
            test -x ~/.local/share/uv/tools/solstone/bin/sol
            compgen -G "$HOME/.local/share/solstone/setup-backups/solstone.old-symlink-*" >/dev/null
            compgen -G "$HOME/.local/share/solstone/setup-backups/sol.old-symlink-*" >/dev/null
            solstone_backup=$(compgen -G "$HOME/.local/share/solstone/setup-backups/solstone.old-symlink-*" | head -1)
            sol_backup=$(compgen -G "$HOME/.local/share/solstone/setup-backups/sol.old-symlink-*" | head -1)
            test -L "$solstone_backup"
            test -L "$sol_backup"
            test "$(readlink "$solstone_backup")" = "$HOME/.local/share/uv/tools/solstone/bin/solstone"
            test "$(readlink "$sol_backup")" = "$HOME/.local/share/uv/tools/solstone/bin/sol"

            grep -q "^ExecStart=.*/journal start 5015$" ~/.config/systemd/user/solstone.service
            cd "$HOME/journal"
            tar --null --no-recursion --files-from=/tmp/journal-preexisting.paths \
                --mtime=@0 --owner=0 --group=0 --numeric-owner \
                -cf /tmp/journal-preexisting.after.tar
            cmp /tmp/journal-preexisting.before.tar /tmp/journal-preexisting.after.tar
        '

        log "verify: full journal doctor reports service_identity ok"
        # journal doctor runs JOURNAL_CHECKS (service_identity lives only there,
        # not in the setup --readiness battery — which is exactly why Ryan's
        # FAIL service_identity never gated setup). After the heal the service
        # target resolves to the current install, so it must report ok.
        docker exec -u "$TEST_USER" "$CONTAINER" bash -lc '
            set -euo pipefail
            journal doctor --json > /tmp/legacy-upgrade-doctor.json
            python3 - <<PY
import json
checks = json.load(open("/tmp/legacy-upgrade-doctor.json")).get("checks", [])
rows = [c for c in checks if c.get("name") == "service_identity"]
assert rows, "service_identity check missing from journal doctor output"
status = rows[0].get("status")
print("service_identity:", status)
assert status == "ok", "expected ok, got " + str(rows[0])
PY
        '

        log "legacy-upgrade: PASS"
        ;;

    shell)
        log "dropping into shell as $TEST_USER (Ctrl-D to exit, container stops)"
        docker exec -u "$TEST_USER" -it "$CONTAINER" bash -l
        ;;
esac
