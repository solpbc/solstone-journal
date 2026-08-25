#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

set -eu

GIT=${GIT:-git}
SCP=${SCP:-scp}
SSH=${SSH:-ssh}
ssh_output_file=

cleanup() {
  original_status=$1
  trap - EXIT HUP INT TERM
  set +e
  if [ -n "$ssh_output_file" ]; then
    rm -f "$ssh_output_file"
  fi
  exit "$original_status"
}

trap 'cleanup $?' EXIT
trap 'cleanup 129' HUP
trap 'cleanup 130' INT
trap 'cleanup 143' TERM

is_lower_hex() {
  candidate=$1
  expected_length=$2
  [ "${#candidate}" -eq "$expected_length" ] || return 1
  case "$candidate" in
    *[!0-9a-f]*) return 1 ;;
  esac
}

if ! command -v flock >/dev/null 2>&1; then
  echo "ERROR: win-host-ci: flock is required on the driver host but was not found" >&2
  exit 1
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
WIN_CI_BINDING_FILE=${WIN_CI_BINDING_FILE:-"$repo_root/target/win-host-ci-source-binding.json"}
cd "$repo_root"

if git_common_dir=$("$GIT" rev-parse --git-common-dir); then
  :
else
  echo "ERROR: win-host-ci: unable to resolve git common directory" >&2
  exit 1
fi
case "$git_common_dir" in
  /*) ;;
  *) git_common_dir=$repo_root/$git_common_dir ;;
esac
if git_common_dir=$(CDPATH= cd -- "$git_common_dir" && pwd); then
  :
else
  echo "ERROR: win-host-ci: unable to resolve git common directory" >&2
  exit 1
fi
lock_path=$git_common_dir/solstone-journal-win-host-ci.lock

if exec 9>"$lock_path"; then
  :
else
  echo "ERROR: win-host-ci: lock file open failed: $lock_path" >&2
  exit 1
fi
echo "win-host-ci: waiting for lock $lock_path"
if flock 9; then
  :
else
  echo "ERROR: win-host-ci: lock acquisition failed: $lock_path" >&2
  exit 1
fi
echo "win-host-ci: acquired lock $lock_path"

if WIN_REMOTE_HOST="${WIN_REMOTE_HOST:-}" \
  WIN_CI_BINDING_FILE="$WIN_CI_BINDING_FILE" \
  EXPECTED_WIN_COMMIT="${EXPECTED_WIN_COMMIT:-}" \
  GIT="$GIT" \
  SCP="$SCP" \
  sh "$script_dir/sync-win-host.sh"; then
  :
else
  sync_status=$?
  echo "ERROR: win-host-ci: sync failed" >&2
  exit "$sync_status"
fi

binding_valid=1
if [ -f "$WIN_CI_BINDING_FILE" ]; then
  binding_line_count=$(awk 'END { print NR + 0 }' "$WIN_CI_BINDING_FILE")
  schema_line=$(sed -n '2p' "$WIN_CI_BINDING_FILE")
  snapshot_sha=$(sed -n 's/^  "commit": "\([0-9a-f]*\)",$/\1/p' "$WIN_CI_BINDING_FILE")
  cargo_lock_sha256=$(sed -n 's/^  "cargo_lock_sha256": "\([0-9a-f]*\)"$/\1/p' "$WIN_CI_BINDING_FILE")
  [ "$(sed -n '1p' "$WIN_CI_BINDING_FILE")" = "{" ] || binding_valid=0
  [ "$schema_line" = '  "schema": "solstone.journal.win-source-binding.v1",' ] || binding_valid=0
  [ "$(sed -n '5p' "$WIN_CI_BINDING_FILE")" = "}" ] || binding_valid=0
else
  binding_line_count=0
  snapshot_sha=
  cargo_lock_sha256=
  binding_valid=0
fi
if [ "$binding_line_count" -ne 5 ] ||
  ! is_lower_hex "$snapshot_sha" 40 ||
  ! is_lower_hex "$cargo_lock_sha256" 64; then
  binding_valid=0
fi
if [ "$binding_valid" -ne 1 ]; then
  echo "ERROR: win-host-ci: local source binding is missing or malformed; rerun sync-win-host and do not invoke the box until it succeeds" >&2
  exit 1
fi

if ssh_output_file=$(mktemp "$repo_root/target/win-host-ci.ssh.XXXXXX"); then
  :
else
  echo "ERROR: win-host-ci: SSH output file creation failed" >&2
  exit 1
fi
remote_command="cmd /d /c \"set EXPECTED_JOURNAL_COMMIT=$snapshot_sha&&set EXPECTED_JOURNAL_CARGO_LOCK_SHA256=$cargo_lock_sha256&&C:\\sol\\sj-ci.cmd\""
if "$SSH" \
  -o ControlMaster=auto \
  -o "ControlPath=/tmp/sj-%r@%h:%p" \
  -o ControlPersist=60s \
  "${WIN_REMOTE_HOST:-}" \
  "$remote_command" >"$ssh_output_file"; then
  ssh_status=0
else
  ssh_status=$?
fi
cat "$ssh_output_file"
if [ "$ssh_status" -ne 0 ]; then
  echo "ERROR: win-host-ci: ssh failed (exit $ssh_status)" >&2
  exit "$ssh_status"
fi

normalized_output=$(awk '{ sub(/\r$/, ""); print }' "$ssh_output_file")
head_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_HEAD=/ { count++ } END { print count + 0 }')
cargo_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_CARGO_LOCK_SHA256=/ { count++ } END { print count + 0 }')
ok_count=$(printf '%s\n' "$normalized_output" | awk '/^=== JOURNAL_WIN_CI_OK:/ { count++ } END { print count + 0 }')
if [ "$head_count" -ne 1 ]; then
  echo "ERROR: win-host-ci: expected exactly one JOURNAL_WIN_CI_HEAD line, found $head_count; rerun the box gate for the transferred binding" >&2
  exit 1
fi
if [ "$cargo_count" -ne 1 ]; then
  echo "ERROR: win-host-ci: expected exactly one JOURNAL_WIN_CI_CARGO_LOCK_SHA256 line, found $cargo_count; rerun the box gate for the transferred binding" >&2
  exit 1
fi
if [ "$ok_count" -ne 1 ]; then
  echo "ERROR: win-host-ci: expected exactly one JOURNAL_WIN_CI_OK acknowledgement, found $ok_count; rerun the complete box gate" >&2
  exit 1
fi

remote_head=$(printf '%s\n' "$normalized_output" | sed -n 's/^JOURNAL_WIN_CI_HEAD=//p')
remote_cargo_lock_sha256=$(printf '%s\n' "$normalized_output" | sed -n 's/^JOURNAL_WIN_CI_CARGO_LOCK_SHA256=//p')
if [ "$remote_head" != "$snapshot_sha" ]; then
  echo "ERROR: win-host-ci: remote HEAD mismatch: expected $snapshot_sha, actual $remote_head; restore the transferred snapshot and rerun" >&2
  exit 1
fi
if [ "$remote_cargo_lock_sha256" != "$cargo_lock_sha256" ]; then
  echo "ERROR: win-host-ci: remote Cargo.lock SHA-256 mismatch: expected $cargo_lock_sha256, actual $remote_cargo_lock_sha256; restore the transferred lockfile and rerun" >&2
  exit 1
fi

head_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_HEAD=/ { print NR }')
cargo_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_CARGO_LOCK_SHA256=/ { print NR }')
ok_line=$(printf '%s\n' "$normalized_output" | awk '/^=== JOURNAL_WIN_CI_OK:/ { print NR }')
if [ "$head_line" -ge "$ok_line" ] || [ "$cargo_line" -ge "$ok_line" ]; then
  echo "ERROR: win-host-ci: source-binding acknowledgements must precede JOURNAL_WIN_CI_OK; rerun the current box gate" >&2
  exit 1
fi

echo "JOURNAL_WIN_HOST_CI_VERIFIED commit=$snapshot_sha cargo_lock_sha256=$cargo_lock_sha256"
