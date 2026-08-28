#!/usr/bin/env bash
# Real public-v1.0.22 to native-v2 package crossover reference.

set -euo pipefail

IMAGE="${IMAGE:-solstone-systemd-test:latest}"
CONTAINER="${CONTAINER:-solstone-systemd-v1022}"
TEST_USER="${TEST_USER:-solstone}"
PACKAGE_FORMAT="${PACKAGE_FORMAT:-deb}"
SOLSTONE_DIST_DIR="${SOLSTONE_DIST_DIR:-}"
V1022_RELEASE_DIR="${V1022_RELEASE_DIR:-}"
KEEP="${KEEP:-0}"

die() { printf 'error: %s\n' "$*" >&2; exit 2; }
log() { printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*" >&2; }

case "$PACKAGE_FORMAT" in
    deb|rpm) ;;
    *) die "PACKAGE_FORMAT must be deb or rpm" ;;
esac
command -v docker >/dev/null || die "docker not found in PATH"
command -v curl >/dev/null || die "curl not found in PATH"
command -v sha256sum >/dev/null || die "sha256sum not found in PATH"
[ -n "$SOLSTONE_DIST_DIR" ] || die "SOLSTONE_DIST_DIR is required"
[ -d "$SOLSTONE_DIST_DIR" ] || die "SOLSTONE_DIST_DIR is not a directory: $SOLSTONE_DIST_DIR"
docker image inspect "$IMAGE" >/dev/null 2>&1 || die "image not found: $IMAGE"

case "$PACKAGE_FORMAT" in
    deb) package_glob="$SOLSTONE_DIST_DIR/solstone-journal-*-linux-x86_64.deb" ;;
    rpm) package_glob="$SOLSTONE_DIST_DIR/solstone-journal-*-linux-x86_64.rpm" ;;
esac
compgen -G "$package_glob" >/dev/null || die "candidate $PACKAGE_FORMAT package not found under $SOLSTONE_DIST_DIR"

script_dir="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
manifest="$script_dir/fixtures/v1022-release-artifacts.txt"
[ -f "$manifest" ] || die "missing pinned v1.0.22 manifest: $manifest"

tag_object=$(git -C "$script_dir/../.." rev-parse v1.0.22)
tag_commit=$(git -C "$script_dir/../.." rev-parse 'v1.0.22^{}')
tag_type=$(git -C "$script_dir/../.." cat-file -t v1.0.22)
[ "$tag_type" = "tag" ] || die "v1.0.22 is not an annotated tag"
[ "$tag_object" = "6aa821da9af87a01ba4e7d4ef6a8e1e13e0fa137" ] || die "v1.0.22 tag-object drift: $tag_object"
[ "$tag_commit" = "27440a1ed118486263eac6e662256f39e46cd6eb" ] || die "v1.0.22 commit drift: $tag_commit"
printf 'v1.0.22 annotated tag object: %s\n' "$tag_object"
printf 'v1.0.22 dereferenced commit: %s\n' "$tag_commit"

if [ -z "$V1022_RELEASE_DIR" ]; then
    V1022_RELEASE_DIR="${XDG_CACHE_HOME:-$HOME/.cache}/solstone-systemd-test/v1.0.22"
fi
mkdir -p "$V1022_RELEASE_DIR"
while read -r digest filename url; do
    case "$digest" in
        ""|\#*) continue ;;
    esac
    if ! printf '%s  %s\n' "$digest" "$V1022_RELEASE_DIR/$filename" | sha256sum -c - >/dev/null 2>&1; then
        log "downloading pinned public v1.0.22 artifact: $filename"
        curl -fL --retry 3 -o "$V1022_RELEASE_DIR/$filename.part" "$url"
        mv "$V1022_RELEASE_DIR/$filename.part" "$V1022_RELEASE_DIR/$filename"
    fi
    printf '%s  %s\n' "$digest" "$V1022_RELEASE_DIR/$filename" | sha256sum -c -
done < "$manifest"

docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
log "starting $CONTAINER from $IMAGE for real v1.0.22/$PACKAGE_FORMAT crossover"
docker run -d --rm --name "$CONTAINER" \
    --privileged \
    --tmpfs /tmp --tmpfs /run --tmpfs /run/lock \
    -v "$SOLSTONE_DIST_DIR:/artifacts:ro" \
    -v "$V1022_RELEASE_DIR:/v1022:ro" \
    "$IMAGE" >/dev/null

cleanup() {
    rc=$?
    if [ "$KEEP" = "1" ]; then
        log "KEEP=1: leaving $CONTAINER running"
    else
        docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
    fi
    exit "$rc"
}
trap cleanup EXIT

wait_system() {
    local state=""
    for _ in $(seq 1 60); do
        state=$(docker exec "$CONTAINER" systemctl is-system-running 2>/dev/null || true)
        case "$state" in
            running|degraded) break ;;
        esac
        sleep 1
    done
    case "$state" in
        running|degraded) ;;
        *) die "system manager did not become ready (last state: ${state:-unknown})" ;;
    esac
}

start_user_manager() {
    docker exec "$CONTAINER" systemctl start "user@$user_uid.service"
    for _ in $(seq 1 30); do
        if docker exec -u "$TEST_USER" \
            -e HOME="$user_home" -e XDG_RUNTIME_DIR="/run/user/$user_uid" \
            "$CONTAINER" systemctl --user list-units --no-pager >/dev/null 2>&1
        then
            return
        fi
        sleep 1
    done
    die "user manager did not become ready"
}

run_phase() {
    local phase=$1
    docker exec -u "$TEST_USER" \
        -e HOME="$user_home" \
        -e XDG_RUNTIME_DIR="/run/user/$user_uid" \
        -e PATH="$user_home/.local/bin:/usr/local/bin:/usr/bin:/bin" \
        "$CONTAINER" /opt/release-crossover-v1022-inside.sh "$phase" "$PACKAGE_FORMAT"
}

wait_system
user_uid=$(docker exec "$CONTAINER" id -u "$TEST_USER")
user_home=$(docker exec "$CONTAINER" getent passwd "$TEST_USER" | cut -d: -f6)
[ -n "$user_home" ] || die "could not resolve test-user home"
start_user_manager

docker cp "$script_dir/release-crossover-v1022-inside.sh" \
    "$CONTAINER:/opt/release-crossover-v1022-inside.sh"
docker cp "$script_dir/systemctl-stop-fault.sh" \
    "$CONTAINER:/opt/systemctl-stop-fault.sh"
docker exec "$CONTAINER" chmod 755 /opt/release-crossover-v1022-inside.sh
docker exec "$CONTAINER" chmod 755 /opt/systemctl-stop-fault.sh

run_phase crossover

log "emulating logout: stop and recreate the owner user manager"
before_logout_pid=$(docker exec -u "$TEST_USER" \
    -e HOME="$user_home" -e XDG_RUNTIME_DIR="/run/user/$user_uid" \
    "$CONTAINER" systemctl --user show solstone.service --property=MainPID --value)
docker exec "$CONTAINER" systemctl stop "user@$user_uid.service"
if docker exec "$CONTAINER" kill -0 "$before_logout_pid" >/dev/null 2>&1; then
    die "v2 service PID survived owner logout"
fi
start_user_manager
run_phase verify-v2

log "emulating reboot: restart container and re-enter through enabled user service"
docker restart "$CONTAINER" >/dev/null
wait_system
start_user_manager
run_phase verify-v2

run_phase clean-uninstall
case "$PACKAGE_FORMAT" in
    deb) docker exec "$CONTAINER" apt-get remove -y solstone-journal ;;
    rpm) docker exec "$CONTAINER" dnf remove -y solstone-journal ;;
esac
if docker exec "$CONTAINER" test -e /usr/bin/journal; then
    die "candidate package uninstall left /usr/bin/journal"
fi

run_phase downgrade
run_phase teardown
log "release-crossover-v1022/$PACKAGE_FORMAT: PASS"
