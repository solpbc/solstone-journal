#!/usr/bin/env bash
set -euo pipefail

phase=${1:?phase is required}
package_format=${2:?package format is required}
setup=(/usr/bin/journal setup -y --accept-existing-journal --skip-models --skip-brain --skip-skills)
state_dir="$HOME/.local/state/solstone-v1022-reference"
payload_dir="$HOME/journal/reference-v1022"

log() { printf '[inside:%s] %s\n' "$phase" "$*" >&2; }
fail() { printf 'failure: %s\n' "$*" >&2; exit 1; }
assert_payload() { sha256sum -c "$state_dir/payload.sha256"; }

wait_active() {
    local state=""
    for _ in $(seq 1 60); do
        state=$(systemctl --user is-active solstone.service 2>/dev/null || true)
        [ "$state" = "active" ] && return
        sleep 1
    done
    systemctl --user status solstone.service --no-pager -l >&2 || true
    fail "solstone.service did not become active (last state: ${state:-unknown})"
}

listener_inode() {
    awk '$2 ~ /:1397$/ && $4 == "0A" { print $10; exit }' /proc/net/tcp /proc/net/tcp6
}

descendant_pid() {
    local root=$1 child
    child=$(pgrep -P "$root" | head -1 || true)
    [ -n "$child" ] || return 1
    printf '%s\n' "$child"
}

install_public_v1() {
    local solstone_wheel journal_wheel core_wheel speakers_wheel
    solstone_wheel=$(find /v1022 -maxdepth 1 -name 'solstone-1.0.22-*.whl' -print -quit)
    journal_wheel=$(find /v1022 -maxdepth 1 -name 'solstone_journal-1.0.22-*.whl' -print -quit)
    core_wheel=$(find /v1022 -maxdepth 1 -name 'solstone_core-1.0.22-*.whl' -print -quit)
    speakers_wheel=$(find /v1022 -maxdepth 1 -name 'solstone_core_speakers_analyze-1.0.22-*.whl' -print -quit)
    if [ -z "$solstone_wheel" ] || [ -z "$journal_wheel" ] \
        || [ -z "$core_wheel" ] || [ -z "$speakers_wheel" ]; then
        fail "pinned v1.0.22 wheels are incomplete"
    fi
    uv tool install --python 3.12 --force "$solstone_wheel" --with "$core_wheel"
    uv tool install --python 3.12 --force "$journal_wheel" \
        --with "$solstone_wheel" --with "$core_wheel" --with "$speakers_wheel"
}

inspect_and_install_candidate() {
    local package
    case "$package_format" in
        deb)
            package=$(find /artifacts -maxdepth 1 -name 'solstone-journal-*-linux-x86_64.deb' -print | sort -V | tail -1)
            [ -n "$package" ] || fail "candidate .deb missing"
            dpkg-deb --fsys-tarfile "$package" | tar -tf - | sort > /tmp/deb-data-members
            grep -Eq '^(\./)?usr/bin/journal$' /tmp/deb-data-members
            rm -rf /tmp/deb-control
            mkdir /tmp/deb-control
            dpkg-deb -e "$package" /tmp/deb-control
            for script in preinst postinst prerm postrm config triggers; do
                [ ! -e "/tmp/deb-control/$script" ] || fail "deb contains prohibited maintainer script: $script"
            done
            dpkg-deb --ctrl-tarfile "$package" | tar -tf - | sort > /tmp/deb-control-members
            if grep -Ev '^(\./)?(control)?$' /tmp/deb-control-members; then
                fail "deb control archive contains more than control metadata"
            fi
            if grep -Eq '(^|/)(preinst|postinst|prerm|postrm|_gpgorigin|.*\.sig)$' \
                /tmp/deb-data-members; then
                fail "deb data archive contains a prohibited script/signature"
            fi
            printf 'candidate package: %s\n' "$(sha256sum "$package")"
            printf 'ordinary install uid: %s\n' "$(id -u)"
            sudo apt-get update -qq
            sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "$package"
            ;;
        rpm)
            package=$(find /artifacts -maxdepth 1 -name 'solstone-journal-*-linux-x86_64.rpm' -print | sort -V | tail -1)
            [ -n "$package" ] || fail "candidate .rpm missing"
            rpm -qpl "$package" | grep -Fxq '/usr/bin/journal'
            [ -z "$(rpm -qp --scripts "$package")" ] || fail "rpm contains prohibited scriptlets"
            if rpm -qp --qf '%{SIGPGP:pgpsig}\n%{SIGGPG:pgpsig}\n%{RSAHEADER:pgpsig}\n%{DSAHEADER:pgpsig}\n' "$package" \
                | grep -Fv '(none)' | grep -q .; then
                fail "rpm contains a prohibited embedded signature"
            fi
            printf 'candidate package: %s\n' "$(sha256sum "$package")"
            printf 'ordinary install uid: %s\n' "$(id -u)"
            sudo dnf install -y "$package"
            ;;
        *) fail "unknown package format: $package_format" ;;
    esac
    test -x /usr/bin/journal
}

case "$phase" in
    crossover)
        mkdir -p "$state_dir"
        log "installing the digest-pinned public v1.0.22 wheels"
        sha256sum /v1022/*.whl | sort
        install_public_v1
        solstone --version | grep -F '1.0.22'
        journal --version | grep -F '1.0.22'
        journal setup -y --skip-models --skip-brain --skip-skills
        wait_active

        mkdir -p "$payload_dir"
        printf 'reference payload survives failures, crossover, reboot, uninstall, and downgrade\n' \
            > "$payload_dir/owner-payload.txt"
        printf '\000\001\002owner-bytes\377' > "$payload_dir/owner-payload.bin"
        find "$payload_dir" -type f -print0 | sort -z | xargs -0 sha256sum > "$state_dir/payload.sha256"

        v1_main=$(systemctl --user show solstone.service --property=MainPID --value)
        [ "$v1_main" -gt 1 ] || fail "v1 main PID missing"
        v1_child=$(descendant_pid "$v1_main") || fail "v1 descendant PID missing"
        v1_listener=$(listener_inode)
        [ -n "$v1_listener" ] || fail "v1 listener inode missing"
        printf '%s\n' "$v1_main" > "$state_dir/v1-main.pid"
        printf '%s\n' "$v1_child" > "$state_dir/v1-child.pid"
        printf '%s\n' "$v1_listener" > "$state_dir/v1-listener.inode"
        printf 'v1 main PID: %s\nv1 descendant PID: %s\nv1 listener inode: %s\n' \
            "$v1_main" "$v1_child" "$v1_listener"
        cp "$HOME/.local/bin/journal" "$state_dir/v1-journal.wrapper"
        sha256sum "$HOME/.local/bin/solstone" "$HOME/.local/bin/journal" \
            "$HOME/.local/bin/sol" "$HOME/.config/systemd/user/solstone.service" \
            > "$state_dir/v1-artifacts.sha256"

        log "installing the native candidate and proving the normal PATH shadow"
        inspect_and_install_candidate
        [ "$(command -v journal)" = "$HOME/.local/bin/journal" ] \
            || fail "bare journal does not resolve to the v1 owner path"
        journal --version | grep -F '1.0.22'
        /usr/bin/journal --version | grep -F '2.0.0'

        log "near-twin: exact recognition must fail before mutation or backup"
        sed -i 's/Edits will be overwritten/Changes will be overwritten/' "$HOME/.local/bin/journal"
        near_twin_hash=$(sha256sum "$HOME/.local/bin/journal")
        unit_hash=$(sha256sum "$HOME/.config/systemd/user/solstone.service")
        if "${setup[@]}" > "$state_dir/near-twin.out" 2>&1; then
            fail "near-twin launcher was admitted"
        fi
        [ "$(sha256sum "$HOME/.local/bin/journal")" = "$near_twin_hash" ] \
            || fail "near-twin launcher changed on refusal"
        [ "$(sha256sum "$HOME/.config/systemd/user/solstone.service")" = "$unit_hash" ] \
            || fail "service changed on near-twin refusal"
        [ ! -e "$HOME/.local/share/solstone/setup-backups" ] \
            || fail "near-twin refusal created a backup"
        [ ! -e "$HOME/.local/share/solstone/installations" ] \
            || fail "near-twin refusal created installation identity"
        kill -0 "$v1_main"
        assert_payload
        cp "$state_dir/v1-journal.wrapper" "$HOME/.local/bin/journal"
        chmod 755 "$HOME/.local/bin/journal"

        log "artifact replacement failure: no v2 artifact or authority may publish"
        mkdir -p "$HOME/.local/share/solstone"
        printf 'injected test fault\n' > "$HOME/.local/share/solstone/setup-backups"
        if "${setup[@]}" > "$state_dir/artifact-failure.out" 2>&1; then
            fail "artifact replacement fault unexpectedly succeeded"
        fi
        sha256sum -c "$state_dir/v1-artifacts.sha256"
        kill -0 "$v1_main"
        [ "$(listener_inode)" = "$v1_listener" ] || fail "v1 listener changed after artifact failure"
        assert_payload
        rm "$HOME/.local/share/solstone/setup-backups"

        log "legacy retirement failure: v1 authority must remain and the same command must be retryable"
        restore_systemctl() {
            if sudo test -e /usr/bin/systemctl.reference-real; then
                sudo mv /usr/bin/systemctl.reference-real /usr/bin/systemctl
            fi
        }
        sudo mv /usr/bin/systemctl /usr/bin/systemctl.reference-real
        trap restore_systemctl EXIT
        trap 'exit 130' INT
        trap 'exit 143' TERM
        sudo cp /opt/systemctl-stop-fault.sh /usr/bin/systemctl
        sudo chmod 755 /usr/bin/systemctl
        retirement_unit_hash=$(sha256sum "$HOME/.config/systemd/user/solstone.service")
        retirement_lock_identity=$(stat -c '%d:%i:%a' "$HOME/journal/health/supervisor.lock")
        retirement_status=0
        "${setup[@]}" > "$state_dir/retirement-failure.out" 2>&1 \
            || retirement_status=$?
        restore_systemctl
        trap - EXIT INT TERM
        if [ "$retirement_status" -eq 0 ]; then
            fail "legacy retirement fault unexpectedly succeeded"
        fi
        kill -0 "$v1_main"
        kill -0 "$v1_child"
        [ "$(listener_inode)" = "$v1_listener" ] || fail "v1 listener changed after retirement failure"
        [ "$(sha256sum "$HOME/.config/systemd/user/solstone.service")" = "$retirement_unit_hash" ] \
            || fail "v2 service unit published after failed legacy retirement"
        [ "$(stat -c '%d:%i:%a' "$HOME/journal/health/supervisor.lock")" \
            = "$retirement_lock_identity" ] \
            || fail "legacy supervisor lock changed before retirement completed"
        [ "$(stat -c %a "$HOME/journal/health/supervisor.lock")" = "644" ] \
            || fail "failed retirement did not preserve the v1 lock mode"
        assert_payload

        log "retrying the same absolute setup command without product cleanup"
        "${setup[@]}"
        wait_active
        old_main=$(cat "$state_dir/v1-main.pid")
        old_child=$(cat "$state_dir/v1-child.pid")
        old_listener=$(cat "$state_dir/v1-listener.inode")
        ! kill -0 "$old_main" 2>/dev/null || fail "v1 main PID survived crossover"
        ! kill -0 "$old_child" 2>/dev/null || fail "v1 descendant PID survived crossover"
        ! awk -v inode="$old_listener" '$10 == inode { found=1 } END { exit found ? 0 : 1 }' \
            /proc/net/tcp /proc/net/tcp6 || fail "v1 listener inode survived crossover"
        grep -Fq '# managed-version: 8' "$HOME/.local/bin/journal"
        grep -Fq '# managed-version: 8' "$HOME/.local/bin/solstone"
        [ ! -e "$HOME/.local/bin/sol" ] || fail "v1 sol authority survived crossover"
        journal --version | grep -F '2.0.0'
        grep -Fq "ExecStart=$HOME/.local/bin/journal start 5015" \
            "$HOME/.config/systemd/user/solstone.service"
        grep -Fq 'SOLSTONE_INSTALLATION_NAMESPACE=' "$HOME/.config/systemd/user/solstone.service"
        [ "$(stat -c %a "$HOME/journal/health/supervisor.lock")" = "600" ] \
            || fail "legacy supervisor lock mode did not migrate to 600"
        candidate_main=$(systemctl --user show solstone.service --property=MainPID --value)
        [ "$(readlink -f "/proc/$candidate_main/exe")" = "/usr/bin/solstone-core" ] \
            || fail "active v2 unit is not executing the candidate runtime"
        find "$HOME/.local/share/solstone/setup-backups" -name 'journal.old-*' -print -quit \
            | grep -q . || fail "recognized journal launcher has no recovery backup"
        assert_payload

        log "running a basic native journal write operation"
        mkdir -p "$HOME/journal/facets/reference/news"
        printf '{"title":"Reference","description":"","color":"#667eea","emoji":"R"}\n' \
            > "$HOME/journal/facets/reference/facet.json"
        printf '# Crossover\n\nnative v2 write\n' \
            | journal news write reference --day 20260828
        grep -Fq 'native v2 write' "$HOME/journal/facets/reference/news/20260828.md"
        journal doctor --json > "$state_dir/v2-doctor.json" || true
        python3 - "$state_dir/v2-doctor.json" <<'PY'
import json
import sys
checks = json.load(open(sys.argv[1], encoding="utf-8")).get("checks", [])
service = [row for row in checks if row.get("name") == "service_identity"]
assert service and service[0].get("status") == "ok", service
PY
        printf 'crossover successful; v1 identities retired and journal payload preserved\n'
        ;;

    verify-v2)
        wait_active
        assert_payload
        journal --version | grep -F '2.0.0'
        grep -Fq 'native v2 write' "$HOME/journal/facets/reference/news/20260828.md"
        main=$(systemctl --user show solstone.service --property=MainPID --value)
        [ "$main" -gt 1 ] || fail "v2 PID missing after lifecycle transition"
        [ "$(readlink -f "/proc/$main/exe")" = "/usr/bin/solstone-core" ] \
            || fail "v2 service did not return through the candidate runtime"
        printf 'v2 lifecycle ready: PID %s, listener inode %s\n' "$main" "$(listener_inode)"
        ;;

    clean-uninstall)
        assert_payload
        /usr/bin/journal setup --clean-uninstall --yes
        [ ! -e "$HOME/.config/systemd/user/solstone.service" ] \
            || fail "clean uninstall left the service unit"
        [ ! -e "$HOME/.local/bin/journal" ] \
            || fail "clean uninstall left the journal wrapper"
        [ -z "$(listener_inode)" ] || fail "clean uninstall left the service listener"
        assert_payload
        printf 'v2 clean uninstall preserved the journal and removed runtime authority\n'
        ;;

    downgrade)
        log "performing the concrete downgrade with the same pinned public release artifacts"
        install_public_v1
        journal --version | grep -F '1.0.22'
        journal setup -y --force --accept-existing-journal --skip-models --skip-brain --skip-skills
        wait_active
        journal --version | grep -F '1.0.22'
        assert_payload
        grep -Fq 'native v2 write' "$HOME/journal/facets/reference/news/20260828.md"
        downgrade_main=$(systemctl --user show solstone.service --property=MainPID --value)
        [ "$downgrade_main" -gt 1 ] || fail "downgraded v1 service has no main PID"
        [ -n "$(listener_inode)" ] || fail "downgraded v1 service has no listener"
        printf 'downgrade ready: v1.0.22 PID %s; seeded and v2-written journal payloads preserved\n' \
            "$downgrade_main"
        ;;

    teardown)
        teardown_pid=$(systemctl --user show solstone.service --property=MainPID --value)
        systemctl --user disable --now solstone.service
        ! kill -0 "$teardown_pid" 2>/dev/null || fail "service PID survived teardown"
        [ -z "$(listener_inode)" ] || fail "listener survived teardown"
        assert_payload
        printf 'teardown complete: no service PID or listener remains; journal preserved\n'
        ;;

    *) fail "unknown phase: $phase" ;;
esac
