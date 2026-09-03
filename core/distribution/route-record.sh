# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# POSIX shell consumer for the fixed journal-route stdout record protocol.
# Source this file; route_record_parse retains only validated input in a private
# temporary file, so callers never evaluate record-controlled shell source.

set -eu

ROUTE_RECORD_MAX_BYTES=65536
ROUTE_RECORD_FILE=
ROUTE_RECORD_COMMAND=
ROUTE_RECORD_VALID=0

_route_record_expected_keys() {
    case "$1" in
        inspect)
            cat <<'EOF'
record_version
command
outcome
platform
prefix_hex
current_bin_hex
current_state
identity_state
identity_namespace
identity_id
identity_generation
identity_journal_token_hex
tuple_state
refusal
journal_wrapper_state
journal_wrapper_path_hex
journal_wrapper_target_hex
journal_wrapper_guard_namespace
journal_wrapper_guard_id
journal_wrapper_guard_generation
journal_wrapper_guard_journal_token_hex
solstone_wrapper_state
solstone_wrapper_path_hex
solstone_wrapper_target_hex
solstone_wrapper_guard_namespace
solstone_wrapper_guard_id
solstone_wrapper_guard_generation
solstone_wrapper_guard_journal_token_hex
service_state
service_path_hex
service_launcher_hex
service_runtime_dir_hex
service_guard_namespace
service_guard_id
service_guard_generation
service_guard_journal_token_hex
EOF
            ;;
        repair)
            cat <<'EOF'
record_version
command
outcome
platform
prefix_hex
current_bin_hex
current_state
identity_state
identity_namespace
identity_id
identity_generation
identity_journal_token_hex
tuple_state
refusal
route_lock_state
repair_wrapper
repair_service
terminal_identity_state
journal_wrapper_state
journal_wrapper_path_hex
journal_wrapper_target_hex
journal_wrapper_guard_namespace
journal_wrapper_guard_id
journal_wrapper_guard_generation
journal_wrapper_guard_journal_token_hex
solstone_wrapper_state
solstone_wrapper_path_hex
solstone_wrapper_target_hex
solstone_wrapper_guard_namespace
solstone_wrapper_guard_id
solstone_wrapper_guard_generation
solstone_wrapper_guard_journal_token_hex
service_state
service_path_hex
service_launcher_hex
service_runtime_dir_hex
service_guard_namespace
service_guard_id
service_guard_generation
service_guard_journal_token_hex
EOF
            ;;
        *)
            return 1
            ;;
    esac
}

_route_record_is_lower_hex() {
    case "$1" in
        '') return 0 ;;
        *[!0123456789abcdef]*) return 1 ;;
    esac
    [ $(( ${#1} % 2 )) -eq 0 ]
}

_route_record_is_generation() {
    case "$1" in
        '') return 0 ;;
        0|0*) return 1 ;;
        *[!0123456789]*) return 1 ;;
    esac
    return 0
}

_route_record_token_is_valid() {
    _route_record_token_key=$1
    _route_record_token_value=$2
    case "$_route_record_token_key" in
        record_version)
            [ "$_route_record_token_value" = "1" ]
            ;;
        command)
            case "$_route_record_token_value" in inspect|repair) return 0 ;; *) return 1 ;; esac
            ;;
        outcome)
            case "$ROUTE_RECORD_PARSE_COMMAND:$_route_record_token_value" in
                inspect:success|inspect:refused|repair:success|repair:refused|repair:partial-failure)
                    return 0
                    ;;
                *) return 1 ;;
            esac
            ;;
        platform)
            case "$_route_record_token_value" in
                ''|linux|darwin|darwin-not-applicable|unsupported-not-applicable) return 0 ;;
                *) return 1 ;;
            esac
            ;;
        current_state)
            case "$_route_record_token_value" in
                ''|selected|not-selected|malformed|not-applicable) return 0 ;;
                *) return 1 ;;
            esac
            ;;
        identity_state)
            case "$_route_record_token_value" in
                ''|present|missing|not-adopted|mismatch|malformed) return 0 ;;
                *) return 1 ;;
            esac
            ;;
        tuple_state)
            case "$_route_record_token_value" in
                ''|aligned|drifted|foreign|ambiguous|malformed|unguarded|exact-v1|missing-identity|missing|not-applicable) return 0 ;;
                *) return 1 ;;
            esac
            ;;
        refusal)
            case "$_route_record_token_value" in
                none|invalid-arguments|unsupported-platform|missing-identity|identity-not-adopted|identity-mismatch|artifact-foreign|artifact-ambiguous|artifact-malformed|artifact-unguarded|record-too-large|observation-failed|not-current|lock-missing|lock-invalid|lock-owner-mismatch|tuple-not-repair-eligible|artifact-exact-v1|service-lock-unavailable) return 0 ;;
                *) return 1 ;;
            esac
            ;;
        route_lock_state)
            case "$_route_record_token_value" in
                ''|validated|missing|invalid|owner-mismatch|not-applicable) return 0 ;;
                *) return 1 ;;
            esac
            ;;
        repair_wrapper|repair_service)
            case "$_route_record_token_value" in ''|not-run|unchanged|rewritten|failed) return 0 ;; *) return 1 ;; esac
            ;;
        terminal_identity_state)
            case "$_route_record_token_value" in ''|not-run|matched|changed) return 0 ;; *) return 1 ;; esac
            ;;
        journal_wrapper_state|solstone_wrapper_state)
            case "$_route_record_token_value" in
                ''|aligned|drifted|foreign|cross-prefix|dangling|malformed|unguarded|ambiguous|exact-v1|missing-identity|missing|not-applicable) return 0 ;;
                *) return 1 ;;
            esac
            ;;
        service_state)
            case "$_route_record_token_value" in
                ''|aligned|drifted|foreign|dangling|malformed|unguarded|ambiguous|missing-identity|runtime-drifted|missing|not-applicable) return 0 ;;
                *) return 1 ;;
            esac
            ;;
        *_hex)
            _route_record_is_lower_hex "$_route_record_token_value"
            ;;
        identity_namespace|identity_id|journal_wrapper_guard_namespace|journal_wrapper_guard_id|solstone_wrapper_guard_namespace|solstone_wrapper_guard_id|service_guard_namespace|service_guard_id)
            _route_record_is_lower_hex "$_route_record_token_value"
            ;;
        identity_generation|journal_wrapper_guard_generation|solstone_wrapper_guard_generation|service_guard_generation)
            _route_record_is_generation "$_route_record_token_value"
            ;;
        *)
            return 1
            ;;
    esac
}

_route_record_discard_candidate() {
    [ -n "${1:-}" ] && rm -f -- "$1"
}

# Discard the last parsed record and its private backing file.
route_record_reset() {
    if [ -n "$ROUTE_RECORD_FILE" ]; then
        rm -f -- "$ROUTE_RECORD_FILE"
    fi
    ROUTE_RECORD_FILE=
    ROUTE_RECORD_COMMAND=
    ROUTE_RECORD_PARSE_COMMAND=
    ROUTE_RECORD_VALID=0
}

# Parse one complete record from stdin; failed parsing leaves no values available.
route_record_parse() {
    route_record_reset
    _route_record_candidate=$(mktemp "${TMPDIR:-/var/tmp}/solstone-route-record.XXXXXX") || return 1
    if ! cat > "$_route_record_candidate"; then
        _route_record_discard_candidate "$_route_record_candidate"
        ROUTE_RECORD_PARSE_COMMAND=
        return 1
    fi

    _route_record_size=$(wc -c < "$_route_record_candidate" | tr -d '[:space:]')
    case "$_route_record_size" in ''|*[!0123456789]*) _route_record_discard_candidate "$_route_record_candidate"; ROUTE_RECORD_PARSE_COMMAND=; return 1 ;; esac
    if [ "$_route_record_size" -eq 0 ] || [ "$_route_record_size" -gt "$ROUTE_RECORD_MAX_BYTES" ]; then
        _route_record_discard_candidate "$_route_record_candidate"
        ROUTE_RECORD_PARSE_COMMAND=
        return 1
    fi
    _route_record_non_ascii=$(LC_ALL=C tr -d '\012\040-\176' < "$_route_record_candidate" | wc -c | tr -d '[:space:]')
    if [ "$_route_record_non_ascii" -ne 0 ]; then
        _route_record_discard_candidate "$_route_record_candidate"
        ROUTE_RECORD_PARSE_COMMAND=
        return 1
    fi
    _route_record_last_byte=$(dd if="$_route_record_candidate" bs=1 skip=$(( _route_record_size - 1 )) count=1 2>/dev/null | od -An -tu1 | tr -d '[:space:]')
    if [ "$_route_record_last_byte" != "10" ]; then
        _route_record_discard_candidate "$_route_record_candidate"
        ROUTE_RECORD_PARSE_COMMAND=
        return 1
    fi

    _route_record_first=$(sed -n '1p' "$_route_record_candidate")
    _route_record_second=$(sed -n '2p' "$_route_record_candidate")
    if [ "$_route_record_first" != "record_version=1" ]; then
        _route_record_discard_candidate "$_route_record_candidate"
        ROUTE_RECORD_PARSE_COMMAND=
        return 1
    fi
    case "$_route_record_second" in
        command=inspect) ROUTE_RECORD_PARSE_COMMAND=inspect ;;
        command=repair) ROUTE_RECORD_PARSE_COMMAND=repair ;;
        *) _route_record_discard_candidate "$_route_record_candidate"; ROUTE_RECORD_PARSE_COMMAND=; return 1 ;;
    esac

    _route_record_expected_keys=$(_route_record_expected_keys "$ROUTE_RECORD_PARSE_COMMAND")
    _route_record_expected_count=$(printf '%s\n' "$_route_record_expected_keys" | wc -l | tr -d '[:space:]')
    _route_record_actual_count=$(wc -l < "$_route_record_candidate" | tr -d '[:space:]')
    if [ "$_route_record_actual_count" -ne "$_route_record_expected_count" ]; then
        _route_record_discard_candidate "$_route_record_candidate"
        ROUTE_RECORD_PARSE_COMMAND=
        return 1
    fi

    _route_record_line_number=1
    for _route_record_expected_key in $_route_record_expected_keys; do
        _route_record_line=$(sed -n "${_route_record_line_number}p" "$_route_record_candidate")
        case "$_route_record_line" in
            *=*)
                _route_record_actual_key=${_route_record_line%%=*}
                _route_record_value=${_route_record_line#*=}
                ;;
            *)
                _route_record_discard_candidate "$_route_record_candidate"
                ROUTE_RECORD_PARSE_COMMAND=
                return 1
                ;;
        esac
        if [ "$_route_record_actual_key" != "$_route_record_expected_key" ] || ! _route_record_token_is_valid "$_route_record_actual_key" "$_route_record_value"; then
            _route_record_discard_candidate "$_route_record_candidate"
            ROUTE_RECORD_PARSE_COMMAND=
            return 1
        fi
        _route_record_line_number=$(( _route_record_line_number + 1 ))
    done

    ROUTE_RECORD_FILE=$_route_record_candidate
    ROUTE_RECORD_COMMAND=$ROUTE_RECORD_PARSE_COMMAND
    ROUTE_RECORD_VALID=1
    return 0
}

# Print a raw field value from a successfully parsed record without evaluating it.
route_record_get() {
    [ "$ROUTE_RECORD_VALID" = "1" ] || return 1
    _route_record_get_key=$1
    _route_record_get_number=1
    _route_record_get_keys=$(_route_record_expected_keys "$ROUTE_RECORD_COMMAND") || return 1
    for _route_record_get_expected in $_route_record_get_keys; do
        if [ "$_route_record_get_expected" = "$_route_record_get_key" ]; then
            _route_record_get_line=$(sed -n "${_route_record_get_number}p" "$ROUTE_RECORD_FILE")
            printf '%s\n' "${_route_record_get_line#*=}"
            return 0
        fi
        _route_record_get_number=$(( _route_record_get_number + 1 ))
    done
    return 1
}

_route_record_hex_nibble() {
    case "$1" in
        0) printf '%s\n' 0 ;; 1) printf '%s\n' 1 ;; 2) printf '%s\n' 2 ;; 3) printf '%s\n' 3 ;;
        4) printf '%s\n' 4 ;; 5) printf '%s\n' 5 ;; 6) printf '%s\n' 6 ;; 7) printf '%s\n' 7 ;;
        8) printf '%s\n' 8 ;; 9) printf '%s\n' 9 ;; a) printf '%s\n' 10 ;; b) printf '%s\n' 11 ;;
        c) printf '%s\n' 12 ;; d) printf '%s\n' 13 ;; e) printf '%s\n' 14 ;; f) printf '%s\n' 15 ;;
        *) return 1 ;;
    esac
}

# Decode validated lowercase hex to stdout without placing arbitrary bytes in shell variables.
route_record_hex_decode() {
    _route_record_hex=$1
    _route_record_is_lower_hex "$_route_record_hex" || return 1
    _route_record_hex_length=${#_route_record_hex}
    _route_record_hex_index=1
    while [ "$_route_record_hex_index" -le "$_route_record_hex_length" ]; do
        _route_record_hex_high=$(printf '%s' "$_route_record_hex" | cut -c "$_route_record_hex_index")
        _route_record_hex_low=$(printf '%s' "$_route_record_hex" | cut -c $(( _route_record_hex_index + 1 )))
        _route_record_hex_high_value=$(_route_record_hex_nibble "$_route_record_hex_high") || return 1
        _route_record_hex_low_value=$(_route_record_hex_nibble "$_route_record_hex_low") || return 1
        _route_record_hex_byte=$(( _route_record_hex_high_value * 16 + _route_record_hex_low_value ))
        _route_record_hex_octal=$(printf '%03o' "$_route_record_hex_byte")
        printf "\\$_route_record_hex_octal"
        _route_record_hex_index=$(( _route_record_hex_index + 2 ))
    done
}
