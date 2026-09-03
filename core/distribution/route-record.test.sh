#!/bin/sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# POSIX process fixture for the installer-facing journal-route record protocol.
# Override SOLSTONE_ROUTE_JOURNAL_BIN and SOLSTONE_ROUTE_CORE_BIN to test
# binaries outside core/target/debug.

set -eu

SCRIPT_DIR=$(CDPATH= cd "$(dirname "$0")" && pwd)
ROOT=$(CDPATH= cd "$SCRIPT_DIR/../.." && pwd)
TMP_ROOT=${TMPDIR:-/var/tmp}
BASE=$(mktemp -d "$TMP_ROOT/solstone-route-record-test-XXXXXX")
SYSTEM_PATH=${PATH:-/usr/bin:/bin}
SOLSTONE_ROUTE_JOURNAL_BIN=${SOLSTONE_ROUTE_JOURNAL_BIN:-"$ROOT/core/target/debug/solstone-core-journal"}
SOLSTONE_ROUTE_CORE_BIN=${SOLSTONE_ROUTE_CORE_BIN:-"$ROOT/core/target/debug/solstone-core"}
PASSED=0

# route_record_reset removes the parser's private capture, if there is one.
. "$SCRIPT_DIR/route-record.sh"

cleanup() {
    route_record_reset
    rm -rf "$BASE"
}
trap cleanup EXIT HUP INT TERM

fail() {
    printf 'not ok - %s\n' "$1" >&2
    exit 1
}

pass() {
    PASSED=$((PASSED + 1))
    printf 'ok - %s\n' "$1"
}

require_binary() {
    [ -x "$1" ] || fail "missing executable binary: $1"
}

fixture_new() {
    FIX_NAME=$1
    FIX_ROOT="$BASE/$FIX_NAME"
    FIX_PREFIX="$FIX_ROOT/prefix"
    FIX_HOME="$FIX_ROOT/home"
    FIX_JOURNAL="$FIX_ROOT/journal"
    FIX_FIRST_VERSION=1.0.0-aaaaaaaaaaaa
    mkdir -p "$FIX_PREFIX" "$FIX_HOME" "$FIX_JOURNAL"
    fixture_make_version "$FIX_FIRST_VERSION"
    ln -s "versions/$FIX_FIRST_VERSION" "$FIX_PREFIX/current"
}

fixture_make_version() {
    FIX_VERSION=$1
    FIX_BIN="$FIX_PREFIX/versions/$FIX_VERSION/bin"
    mkdir -p "$FIX_BIN"
    cp "$SOLSTONE_ROUTE_CORE_BIN" "$FIX_BIN/solstone-core"
    cp "$SOLSTONE_ROUTE_JOURNAL_BIN" "$FIX_BIN/journal"
    cp "$SOLSTONE_ROUTE_JOURNAL_BIN" "$FIX_BIN/solstone"
    printf '%s\n' '#!/bin/sh' 'exit 0' > "$FIX_BIN/solstone-core-speakers-analyze"
    printf '%s\n' \
        '#!/bin/sh' \
        "printf '%s\\n' '{\"schema\":\"solstone-vad-error-v1\",\"reason\":\"malformed-request\",\"detail\":\"empty\"}'" \
        'exit 64' > "$FIX_BIN/solstone-core-vad-analyze"
    chmod 755 "$FIX_BIN/solstone-core-speakers-analyze" "$FIX_BIN/solstone-core-vad-analyze"
}

fixture_select_version() {
    rm -f "$FIX_PREFIX/current"
    ln -s "versions/$1" "$FIX_PREFIX/current"
}

fixture_command() {
    HOME="$FIX_HOME" \
        SOLSTONE_JOURNAL="$FIX_JOURNAL" \
        PATH="$FIX_PREFIX/current/bin:$SYSTEM_PATH" \
        "$@"
}

fixture_bootstrap_owned() {
    if ! fixture_command "$FIX_PREFIX/current/bin/journal" setup --yes --journal "$FIX_JOURNAL" --skip-models --skip-brain --skip-skills --skip-service > "$FIX_ROOT/setup.stdout" 2> "$FIX_ROOT/setup.stderr"; then
        cat "$FIX_ROOT/setup.stdout" >&2
        cat "$FIX_ROOT/setup.stderr" >&2
        fail "$FIX_NAME real setup bootstrap"
    fi
    [ -f "$FIX_HOME/.local/bin/journal" ] || fail "$FIX_NAME setup journal wrapper"
    [ -f "$FIX_HOME/.local/bin/solstone" ] || fail "$FIX_NAME setup solstone wrapper"
}

run_route() {
    ROUTE_STATUS=0
    if fixture_command "$@" > "$FIX_ROOT/record" 2> "$FIX_ROOT/stderr"; then
        ROUTE_STATUS=0
    else
        ROUTE_STATUS=$?
    fi
    route_record_parse < "$FIX_ROOT/record" || fail "$FIX_NAME parse journal-route record"
}

run_inspect() {
    run_route "$FIX_PREFIX/current/bin/journal" __journal-route-inspect
}

run_repair() {
    run_route "$FIX_PREFIX/current/bin/journal" __journal-route-repair --route-lock-owner "$1"
}

record_value() {
    route_record_get "$1"
}

expect_value() {
    EXPECT_KEY=$1
    EXPECT_VALUE=$2
    ACTUAL_VALUE=$(record_value "$EXPECT_KEY")
    [ "$ACTUAL_VALUE" = "$EXPECT_VALUE" ] || fail "$FIX_NAME $EXPECT_KEY expected $EXPECT_VALUE, got $ACTUAL_VALUE"
}

expect_parse_reject() {
    REJECT_NAME=$1
    REJECT_FILE=$2
    if route_record_parse < "$REJECT_FILE"; then
        fail "$REJECT_NAME accepted malformed record"
    fi
    if route_record_get record_version > /dev/null 2>&1; then
        fail "$REJECT_NAME exposed a partial record"
    fi
    pass "$REJECT_NAME is refused without partial state"
}

replace_wrapper_id() {
    REPLACE_WRAPPER=$1
    sed 's/^# solstone-installation-id: .*/# solstone-installation-id: ffeeddccbbaa99887766554433221100/' "$REPLACE_WRAPPER" > "$REPLACE_WRAPPER.tmp"
    mv "$REPLACE_WRAPPER.tmp" "$REPLACE_WRAPPER"
    chmod 755 "$REPLACE_WRAPPER"
}

corrupt_wrapper_guard() {
    CORRUPT_WRAPPER=$1
    {
        cat "$CORRUPT_WRAPPER"
        printf '%s\n' '# solstone-installation-id: duplicate'
    } > "$CORRUPT_WRAPPER.tmp"
    mv "$CORRUPT_WRAPPER.tmp" "$CORRUPT_WRAPPER"
    chmod 755 "$CORRUPT_WRAPPER"
}

strip_wrapper_guards() {
    STRIP_WRAPPER=$1
    sed '/^# solstone-installation-/d' "$STRIP_WRAPPER" > "$STRIP_WRAPPER.tmp"
    mv "$STRIP_WRAPPER.tmp" "$STRIP_WRAPPER"
    chmod 755 "$STRIP_WRAPPER"
}

make_route_lock() {
    LOCK_OWNER_INPUT=$1
    LOCK_PATH="$FIX_PREFIX/.solstone-route.lock"
    LOCK_UMASK=$(umask)
    umask 077
    if ! mkdir "$LOCK_PATH"; then
        umask "$LOCK_UMASK"
        fail "$FIX_NAME acquire route lock"
    fi
    printf 'solstone-route-lock-v1\n%s\n' "$LOCK_OWNER_INPUT" > "$LOCK_PATH/owner"
    chmod 700 "$LOCK_PATH"
    chmod 600 "$LOCK_PATH/owner"
    umask "$LOCK_UMASK"
}

remove_route_lock() {
    rm -f "$FIX_PREFIX/.solstone-route.lock/owner"
    rmdir "$FIX_PREFIX/.solstone-route.lock"
}

new_lock_token() {
    LOCK_TOKEN=$(LC_ALL=C od -An -tx1 -N16 /dev/urandom | tr -d ' \n')
    case "$LOCK_TOKEN" in ''|*[!0123456789abcdef]*) fail "generated route lock token" ;; esac
    [ "${#LOCK_TOKEN}" -eq 32 ] || fail "generated route lock token length"
}

test_inspection_classifications() {
    fixture_new inspect-absent
    run_inspect
    [ "$ROUTE_STATUS" -eq 0 ] || fail "absent inspect status"
    expect_value tuple_state missing-identity
    expect_value journal_wrapper_state missing
    pass "inspection reports an absent unadopted route"

    fixture_new inspect-aligned
    fixture_bootstrap_owned
    run_inspect
    [ "$ROUTE_STATUS" -eq 0 ] || fail "aligned inspect status"
    expect_value journal_wrapper_state aligned
    expect_value solstone_wrapper_state aligned
    expect_value service_state missing
    cp "$FIX_ROOT/record" "$BASE/valid-inspect-record"
    pass "inspection reports setup-owned wrappers independently"

    fixture_new inspect-drifted
    fixture_bootstrap_owned
    fixture_make_version 1.0.0-bbbbbbbbbbbb
    fixture_select_version 1.0.0-bbbbbbbbbbbb
    run_inspect
    expect_value journal_wrapper_state drifted
    expect_value solstone_wrapper_state drifted
    pass "inspection reports a selected-build wrapper drift"

    fixture_new inspect-foreign
    fixture_bootstrap_owned
    replace_wrapper_id "$FIX_HOME/.local/bin/journal"
    replace_wrapper_id "$FIX_HOME/.local/bin/solstone"
    run_inspect
    expect_value journal_wrapper_state foreign
    pass "inspection reports a foreign guarded wrapper"

    fixture_new inspect-malformed
    fixture_bootstrap_owned
    corrupt_wrapper_guard "$FIX_HOME/.local/bin/journal"
    run_inspect
    expect_value journal_wrapper_state malformed
    pass "inspection reports a malformed wrapper"

    fixture_new inspect-exact-v1
    LEGACY_BIN="$FIX_HOME/.local/share/uv/tools/solstone/bin"
    mkdir -p "$LEGACY_BIN" "$FIX_HOME/.local/bin"
    printf '%s\n' \
        '#!/usr/bin/python3' \
        '# -*- coding: utf-8 -*-' \
        'import sys' \
        'from solstone.think.sol_cli import main' \
        "if __name__ == '__main__':" \
        "    if sys.argv[0].endswith('-script.pyw'):" \
        '        sys.argv[0] = sys.argv[0][:-11]' \
        "    elif sys.argv[0].endswith('.exe'):" \
        '        sys.argv[0] = sys.argv[0][:-4]' \
        '    sys.exit(main())' > "$LEGACY_BIN/solstone"
    chmod 755 "$LEGACY_BIN/solstone"
    ln -s "$LEGACY_BIN/solstone" "$FIX_HOME/.local/bin/solstone"
    run_inspect
    expect_value solstone_wrapper_state exact-v1
    pass "inspection reports an exact V1 launcher"

    fixture_new inspect-unguarded
    fixture_bootstrap_owned
    strip_wrapper_guards "$FIX_HOME/.local/bin/journal"
    run_inspect
    expect_value journal_wrapper_state unguarded
    pass "inspection reports an unguarded wrapper"

    fixture_new inspect-mixed
    fixture_bootstrap_owned
    rm -f "$FIX_HOME/.local/bin/solstone"
    run_inspect
    expect_value journal_wrapper_state aligned
    expect_value solstone_wrapper_state missing
    pass "inspection preserves mixed per-wrapper states"
}

test_parser_rejections() {
    VALID_RECORD="$BASE/valid-inspect-record"
    [ -f "$VALID_RECORD" ] || fail "valid inspection record fixture"

    dd if=/dev/zero of="$BASE/oversize" bs=1 count=65537 > /dev/null 2>&1
    expect_parse_reject oversize-record "$BASE/oversize"

    sed '2s/^command=/record_version=/' "$VALID_RECORD" > "$BASE/duplicate-key"
    expect_parse_reject duplicate-key "$BASE/duplicate-key"

    sed '10d' "$VALID_RECORD" > "$BASE/missing-key"
    expect_parse_reject missing-key "$BASE/missing-key"

    sed '4s/=.*//' "$VALID_RECORD" > "$BASE/missing-equals"
    expect_parse_reject missing-equals "$BASE/missing-equals"

    {
        sed -n '1p' "$VALID_RECORD"
        sed -n '3p' "$VALID_RECORD"
        sed -n '2p' "$VALID_RECORD"
        sed -n '4,$p' "$VALID_RECORD"
    } > "$BASE/shuffled-keys"
    expect_parse_reject shuffled-keys "$BASE/shuffled-keys"

    sed '3s/=success/=unexpected/' "$VALID_RECORD" > "$BASE/invalid-token"
    expect_parse_reject invalid-token "$BASE/invalid-token"

    sed '5s/=.*/=AA/' "$VALID_RECORD" > "$BASE/uppercase-hex"
    expect_parse_reject uppercase-hex "$BASE/uppercase-hex"

    sed '5s/=.*/=a/' "$VALID_RECORD" > "$BASE/odd-hex"
    expect_parse_reject odd-hex "$BASE/odd-hex"

    sed '5s/=.*/=gg/' "$VALID_RECORD" > "$BASE/nonhex"
    expect_parse_reject nonhex "$BASE/nonhex"

    sed '1s/=1/=2/' "$VALID_RECORD" > "$BASE/unsupported-version"
    expect_parse_reject unsupported-version "$BASE/unsupported-version"

    {
        sed -n '1,2p' "$VALID_RECORD"
        printf 'outcome=success\377\n'
        sed -n '4,$p' "$VALID_RECORD"
    } > "$BASE/non-ascii"
    expect_parse_reject non-ascii "$BASE/non-ascii"

    {
        cat "$VALID_RECORD"
        printf '%s\n' narration
    } > "$BASE/extra-stdout"
    expect_parse_reject extra-stdout "$BASE/extra-stdout"

    {
        cat "$VALID_RECORD"
        printf '\n'
    } > "$BASE/trailing-blank-line"
    expect_parse_reject trailing-blank-line "$BASE/trailing-blank-line"
}

test_hex_round_trip() {
    fixture_new hex-round-trip
    SPECIAL_COMPONENT=$(printf 'route space \047\042=\012\044\140\377')
    FIX_HOME="$FIX_ROOT/$SPECIAL_COMPONENT"
    mkdir -p "$FIX_HOME"
    fixture_bootstrap_owned
    run_inspect
    expect_value journal_wrapper_state aligned
    printf '%s/.local/bin/journal' "$FIX_HOME" > "$FIX_ROOT/expected-path-bytes"
    route_record_hex_decode "$(record_value journal_wrapper_path_hex)" > "$FIX_ROOT/decoded-path-bytes"
    cmp "$FIX_ROOT/expected-path-bytes" "$FIX_ROOT/decoded-path-bytes" || fail "hex decoder exact path bytes"
    pass "hex decoding preserves arbitrary Unix path bytes"
}

test_lock_join_protocol() {
    fixture_new lock-join
    fixture_bootstrap_owned
    fixture_make_version 1.0.0-bbbbbbbbbbbb
    fixture_select_version 1.0.0-bbbbbbbbbbbb
    JOURNAL_WRAPPER="$FIX_HOME/.local/bin/journal"
    SOLSTONE_WRAPPER="$FIX_HOME/.local/bin/solstone"
    cp "$JOURNAL_WRAPPER" "$FIX_ROOT/journal-before"
    cp "$SOLSTONE_WRAPPER" "$FIX_ROOT/solstone-before"
    new_lock_token
    make_route_lock "$LOCK_TOKEN"
    cp "$FIX_PREFIX/.solstone-route.lock/owner" "$FIX_ROOT/owner-before"

    run_repair "$LOCK_TOKEN"
    [ "$ROUTE_STATUS" -eq 0 ] || fail "lock-held repair status"
    expect_value outcome success
    expect_value route_lock_state validated
    expect_value repair_wrapper rewritten
    expect_value repair_service not-run
    grep -F "$FIX_PREFIX/versions/1.0.0-bbbbbbbbbbbb/bin/journal" "$JOURNAL_WRAPPER" > /dev/null || fail "repair journal target"
    grep -F "$FIX_PREFIX/versions/1.0.0-bbbbbbbbbbbb/bin/solstone" "$SOLSTONE_WRAPPER" > /dev/null || fail "repair solstone target"
    cmp "$FIX_ROOT/owner-before" "$FIX_PREFIX/.solstone-route.lock/owner" || fail "repair modified route lock owner"
    [ -d "$FIX_PREFIX/.solstone-route.lock" ] || fail "repair removed route lock"
    pass "shell-held route lock authorizes repair without being modified"

    if mkdir "$FIX_PREFIX/.solstone-route.lock" > /dev/null 2>&1; then
        fail "second shell actor acquired route lock"
    fi
    pass "second shell actor cannot acquire a live route lock"

    cp "$JOURNAL_WRAPPER" "$FIX_ROOT/journal-after-success"
    cp "$SOLSTONE_WRAPPER" "$FIX_ROOT/solstone-after-success"
    remove_route_lock
    run_repair "$LOCK_TOKEN"
    [ "$ROUTE_STATUS" -eq 2 ] || fail "released-lock replay status"
    expect_value refusal lock-missing
    cmp "$FIX_ROOT/journal-after-success" "$JOURNAL_WRAPPER" || fail "released-lock refusal rewrote journal"
    cmp "$FIX_ROOT/solstone-after-success" "$SOLSTONE_WRAPPER" || fail "released-lock refusal rewrote solstone"
    [ ! -e "$FIX_HOME/.config/systemd/user/solstone.service" ] || fail "released-lock refusal created service"
    pass "released lock rejects a replayed owner token without mutation"

    run_repair "$LOCK_TOKEN"
    [ "$ROUTE_STATUS" -eq 2 ] || fail "absent-lock status"
    expect_value refusal lock-missing
    [ ! -e "$FIX_HOME/.config/systemd/user/solstone.service" ] || fail "absent-lock refusal created service"
    pass "absent lock is refused"

    make_route_lock "$LOCK_TOKEN"
    chmod 644 "$FIX_PREFIX/.solstone-route.lock/owner"
    run_repair "$LOCK_TOKEN"
    [ "$ROUTE_STATUS" -eq 2 ] || fail "malformed-lock status"
    expect_value refusal lock-invalid
    cmp "$FIX_ROOT/journal-after-success" "$JOURNAL_WRAPPER" || fail "malformed-lock refusal rewrote journal"
    cmp "$FIX_ROOT/solstone-after-success" "$SOLSTONE_WRAPPER" || fail "malformed-lock refusal rewrote solstone"
    [ ! -e "$FIX_HOME/.config/systemd/user/solstone.service" ] || fail "malformed-lock refusal created service"
    pass "malformed lock is refused without mutation"
    remove_route_lock

    OTHER_TOKEN=11111111111111111111111111111111
    if [ "$OTHER_TOKEN" = "$LOCK_TOKEN" ]; then
        OTHER_TOKEN=22222222222222222222222222222222
    fi
    make_route_lock "$OTHER_TOKEN"
    run_repair "$LOCK_TOKEN"
    [ "$ROUTE_STATUS" -eq 2 ] || fail "owner-mismatch status"
    expect_value refusal lock-owner-mismatch
    cmp "$FIX_ROOT/journal-after-success" "$JOURNAL_WRAPPER" || fail "owner-mismatch refusal rewrote journal"
    cmp "$FIX_ROOT/solstone-after-success" "$SOLSTONE_WRAPPER" || fail "owner-mismatch refusal rewrote solstone"
    [ ! -e "$FIX_HOME/.config/systemd/user/solstone.service" ] || fail "owner-mismatch refusal created service"
    pass "wrong live lock owner is refused without mutation"
}

test_repair_refusal_classes() {
    fixture_new repair-not-current
    fixture_bootstrap_owned
    fixture_make_version 1.0.0-bbbbbbbbbbbb
    fixture_select_version 1.0.0-bbbbbbbbbbbb
    cp "$FIX_HOME/.local/bin/journal" "$FIX_ROOT/journal-before"
    cp "$FIX_HOME/.local/bin/solstone" "$FIX_ROOT/solstone-before"
    run_route "$FIX_PREFIX/versions/$FIX_FIRST_VERSION/bin/journal" __journal-route-repair --route-lock-owner 11111111111111111111111111111111
    [ "$ROUTE_STATUS" -eq 2 ] || fail "not-current repair status"
    expect_value refusal not-current
    cmp "$FIX_ROOT/journal-before" "$FIX_HOME/.local/bin/journal" || fail "not-current refusal rewrote journal"
    cmp "$FIX_ROOT/solstone-before" "$FIX_HOME/.local/bin/solstone" || fail "not-current refusal rewrote solstone"
    [ ! -e "$FIX_HOME/.config/systemd/user/solstone.service" ] || fail "not-current refusal created service"
    pass "repair record reports not-current before mutation"

    fixture_new repair-foreign
    fixture_bootstrap_owned
    replace_wrapper_id "$FIX_HOME/.local/bin/journal"
    replace_wrapper_id "$FIX_HOME/.local/bin/solstone"
    new_lock_token
    make_route_lock "$LOCK_TOKEN"
    cp "$FIX_HOME/.local/bin/journal" "$FIX_ROOT/journal-before"
    cp "$FIX_HOME/.local/bin/solstone" "$FIX_ROOT/solstone-before"
    run_repair "$LOCK_TOKEN"
    [ "$ROUTE_STATUS" -eq 2 ] || fail "foreign repair status"
    expect_value refusal artifact-foreign
    cmp "$FIX_ROOT/journal-before" "$FIX_HOME/.local/bin/journal" || fail "foreign refusal rewrote journal"
    cmp "$FIX_ROOT/solstone-before" "$FIX_HOME/.local/bin/solstone" || fail "foreign refusal rewrote solstone"
    [ ! -e "$FIX_HOME/.config/systemd/user/solstone.service" ] || fail "foreign refusal created service"
    pass "repair record refuses a foreign artifact without mutation"

    fixture_new repair-malformed
    fixture_bootstrap_owned
    corrupt_wrapper_guard "$FIX_HOME/.local/bin/journal"
    new_lock_token
    make_route_lock "$LOCK_TOKEN"
    cp "$FIX_HOME/.local/bin/journal" "$FIX_ROOT/journal-before"
    cp "$FIX_HOME/.local/bin/solstone" "$FIX_ROOT/solstone-before"
    run_repair "$LOCK_TOKEN"
    [ "$ROUTE_STATUS" -eq 2 ] || fail "malformed repair status"
    expect_value refusal artifact-malformed
    cmp "$FIX_ROOT/journal-before" "$FIX_HOME/.local/bin/journal" || fail "malformed refusal rewrote journal"
    cmp "$FIX_ROOT/solstone-before" "$FIX_HOME/.local/bin/solstone" || fail "malformed refusal rewrote solstone"
    [ ! -e "$FIX_HOME/.config/systemd/user/solstone.service" ] || fail "malformed refusal created service"
    pass "repair record refuses a malformed artifact without mutation"
}

require_binary "$SOLSTONE_ROUTE_JOURNAL_BIN"
require_binary "$SOLSTONE_ROUTE_CORE_BIN"
test_inspection_classifications
test_parser_rejections
test_hex_round_trip
test_lock_join_protocol
test_repair_refusal_classes
printf '%s\n' "$PASSED route-record checks passed"
