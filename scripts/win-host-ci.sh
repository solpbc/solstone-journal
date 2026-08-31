#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

set -eu

GIT=${GIT:-git}
SCP=${SCP:-scp}
SSH=${SSH:-ssh}
ssh_output_file=
cloud_sync_test=${JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST:-}
refs_root=${SOLSTONE_JOURNAL_WIN_REFS_ROOT:-}
owner_account=${SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT:-}
refs_publication=1

case "$cloud_sync_test" in
  '') cloud_sync_test=0 ;;
  0|1) ;;
  *)
    echo "ERROR: win-host-ci: JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST must be unset, empty, 0, or 1" >&2
    exit 1
    ;;
esac

if [ -z "$refs_root" ] || ! printf '%s\n' "$refs_root" | grep -Eq '^[A-Za-z]:[\\/][A-Za-z0-9_. ()\\/:=-]*$'; then
  echo "ERROR: win-host-ci: SOLSTONE_JOURNAL_WIN_REFS_ROOT must be a non-blank safe absolute Windows path for mandatory ReFS receipts" >&2
  exit 1
fi

if [ -z "$owner_account" ]; then
  echo "ERROR: win-host-ci: SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT is required" >&2
  exit 1
fi
if ! printf '%s\n' "$owner_account" | grep -Eq '^[A-Za-z0-9_.\\-]+$'; then
  echo "ERROR: win-host-ci: SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT must be a safe local or domain account name" >&2
  exit 1
fi

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
remote_command="\$env:EXPECTED_JOURNAL_COMMIT = '$snapshot_sha'
\$env:EXPECTED_JOURNAL_CARGO_LOCK_SHA256 = '$cargo_lock_sha256'
\$env:JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST = '$cloud_sync_test'
\$env:SOLSTONE_JOURNAL_WIN_REFS_ROOT = '$refs_root'
\$env:SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT = '$owner_account'
& 'C:\\sol\\sj-ci.cmd'
exit \$LASTEXITCODE"
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
ok_count=$(printf '%s\n' "$normalized_output" | awk '/^=== JOURNAL_WIN_CI_OK:/ { count++ } END { print count + 0 }')
if [ "$ok_count" -ne 1 ]; then
  echo "ERROR: win-host-ci: expected exactly one JOURNAL_WIN_CI_OK acknowledgement, found $ok_count; rerun the complete box gate" >&2
  exit 1
fi
ok_line=$(printf '%s\n' "$normalized_output" | awk '/^=== JOURNAL_WIN_CI_OK:/ { print NR }')
require_native_receipt() {
  receipt_key=$1
  receipt_filesystem=$2
  pass_line="$receipt_key=executed/pass"
  filesystem_line="${receipt_key}_FILESYSTEM=$receipt_filesystem"
  key_count=$(printf '%s\n' "$normalized_output" | awk -v key="$receipt_key" 'index($0, key "=") == 1 { count++ } END { print count + 0 }')
  filesystem_key_count=$(printf '%s\n' "$normalized_output" | awk -v key="${receipt_key}_FILESYSTEM" 'index($0, key "=") == 1 { count++ } END { print count + 0 }')
  pass_count=$(printf '%s\n' "$normalized_output" | awk -v expected="$pass_line" '$0 == expected { count++ } END { print count + 0 }')
  filesystem_count=$(printf '%s\n' "$normalized_output" | awk -v expected="$filesystem_line" '$0 == expected { count++ } END { print count + 0 }')
  pass_position=$(printf '%s\n' "$normalized_output" | awk -v expected="$pass_line" '$0 == expected { print NR }')
  filesystem_position=$(printf '%s\n' "$normalized_output" | awk -v expected="$filesystem_line" '$0 == expected { print NR }')
  if [ "$key_count" -ne 1 ] || [ "$filesystem_key_count" -ne 1 ] || [ "$pass_count" -ne 1 ] || [ "$filesystem_count" -ne 1 ] ||
    [ "$pass_position" -ge "$ok_line" ] || [ "$filesystem_position" -ge "$ok_line" ]; then
    echo "ERROR: win-host-ci: $receipt_key requires exactly one source-originated pass and $receipt_filesystem filesystem marker before JOURNAL_WIN_CI_OK" >&2
    exit 1
  fi
}
require_cortex_namespace_receipts() {
  receipt_token=$1
  receipt_filesystem=$2
  for receipt_category in CREATE_ADMIT WRONG_KIND_REPARSE RETAINED_ROOT RETAINED_HEALTH FAILURE_MAPPING PRESERVATION; do
    require_native_receipt "JOURNAL_WIN_CI_CORTEX_NAMESPACE_${receipt_token}_${receipt_category}" "$receipt_filesystem"
  done
}
require_platform_receipt() {
  receipt_key=$1
  pass_line="$receipt_key=executed/pass"
  key_count=$(printf '%s\n' "$normalized_output" | awk -v key="$receipt_key" 'index($0, key "=") == 1 { count++ } END { print count + 0 }')
  pass_count=$(printf '%s\n' "$normalized_output" | awk -v expected="$pass_line" '$0 == expected { count++ } END { print count + 0 }')
  pass_position=$(printf '%s\n' "$normalized_output" | awk -v expected="$pass_line" '$0 == expected { print NR }')
  if [ "$key_count" -ne 1 ] || [ "$pass_count" -ne 1 ] || [ "$pass_position" -ge "$ok_line" ]; then
    echo "ERROR: win-host-ci: $receipt_key requires exactly one source-originated pass marker before JOURNAL_WIN_CI_OK" >&2
    exit 1
  fi
}
require_platform_receipt JOURNAL_WIN_CI_LAUNCH_ENVIRONMENT_PREPARATION
require_platform_receipt JOURNAL_WIN_CI_LAUNCH_PATH_PREPARATION
require_platform_receipt JOURNAL_WIN_CI_JOB_LIST_NO_HANDLE_INHERITANCE
require_platform_receipt JOURNAL_WIN_CI_JOB_PROCESS_OWNER
require_platform_receipt JOURNAL_WIN_CI_JOB_LAST_HANDLE_NEGATIVE
require_native_receipt JOURNAL_WIN_CI_NTFS_PUBLICATION NTFS
require_native_receipt JOURNAL_WIN_CI_REFS_PUBLICATION ReFS
require_native_receipt JOURNAL_WIN_CI_CORTEX_USE_NTFS NTFS
require_native_receipt JOURNAL_WIN_CI_CORTEX_USE_REFS ReFS
require_cortex_namespace_receipts NTFS NTFS
require_cortex_namespace_receipts REFS ReFS
require_native_receipt JOURNAL_WIN_CI_NTFS_MANAGED_LOG_REFERENCE NTFS
require_native_receipt JOURNAL_WIN_CI_REFS_MANAGED_LOG_REFERENCE ReFS
require_native_receipt JOURNAL_WIN_CI_NTFS_STALE_HEARTBEAT_CLEANUP NTFS
require_native_receipt JOURNAL_WIN_CI_REFS_STALE_HEARTBEAT_CLEANUP ReFS
head_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_HEAD=/ { count++ } END { print count + 0 }')
cargo_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_CARGO_LOCK_SHA256=/ { count++ } END { print count + 0 }')
cloud_evidence_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=/ { count++ } END { print count + 0 }')
ordinary_owner_evidence_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=/ { count++ } END { print count + 0 }')
ordinary_owner_refs_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=/ { count++ } END { print count + 0 }')
refs_publication_evidence_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_PUBLICATION=/ { count++ } END { print count + 0 }')
refs_publication_filesystem_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_PUBLICATION_FILESYSTEM=/ { count++ } END { print count + 0 }')
if [ "$cloud_sync_test" -eq 1 ]; then
  expected_cloud_evidence=passed
else
  expected_cloud_evidence=skipped
fi
expected_cloud_evidence_count=$(printf '%s\n' "$normalized_output" | awk -v expected="$expected_cloud_evidence" '$0 == "JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=" expected { count++ } END { print count + 0 }')
ordinary_owner_passed_count=$(printf '%s\n' "$normalized_output" | awk '$0 == "JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=passed" { count++ } END { print count + 0 }')
ordinary_owner_refs_passed_count=$(printf '%s\n' "$normalized_output" | awk '$0 == "JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=passed" { count++ } END { print count + 0 }')
refs_publication_passed_count=$(printf '%s\n' "$normalized_output" | awk '$0 == "JOURNAL_WIN_CI_REFS_PUBLICATION=executed/pass" { count++ } END { print count + 0 }')
refs_publication_refs_count=$(printf '%s\n' "$normalized_output" | awk '$0 == "JOURNAL_WIN_CI_REFS_PUBLICATION_FILESYSTEM=ReFS" { count++ } END { print count + 0 }')
ok_count=$(printf '%s\n' "$normalized_output" | awk '/^=== JOURNAL_WIN_CI_OK:/ { count++ } END { print count + 0 }')
if [ "$head_count" -ne 1 ]; then
  echo "ERROR: win-host-ci: expected exactly one JOURNAL_WIN_CI_HEAD line, found $head_count; rerun the box gate for the transferred binding" >&2
  exit 1
fi
if [ "$cargo_count" -ne 1 ]; then
  echo "ERROR: win-host-ci: expected exactly one JOURNAL_WIN_CI_CARGO_LOCK_SHA256 line, found $cargo_count; rerun the box gate for the transferred binding" >&2
  exit 1
fi
if [ "$cloud_evidence_count" -ne 1 ] || [ "$expected_cloud_evidence_count" -ne 1 ]; then
  echo "ERROR: win-host-ci: expected exactly one JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=$expected_cloud_evidence line, found $cloud_evidence_count evidence-key lines and $expected_cloud_evidence_count exact matches; rerun the complete box gate" >&2
  exit 1
fi
if [ "$ordinary_owner_evidence_count" -ne 1 ] || [ "$ordinary_owner_passed_count" -ne 1 ]; then
  echo "ERROR: win-host-ci: expected exactly one JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=passed line, found $ordinary_owner_evidence_count evidence-key lines and $ordinary_owner_passed_count exact matches; rerun the complete box gate" >&2
  exit 1
fi
if [ "$refs_publication" -eq 1 ]; then
  if [ "$ordinary_owner_refs_count" -ne 1 ] || [ "$ordinary_owner_refs_passed_count" -ne 1 ] ||
    [ "$refs_publication_evidence_count" -ne 1 ] || [ "$refs_publication_passed_count" -ne 1 ] ||
    [ "$refs_publication_filesystem_count" -ne 1 ] || [ "$refs_publication_refs_count" -ne 1 ]; then
    echo "ERROR: win-host-ci: required ReFS publication receipt needs exactly one ordinary-owner, publication, and runtime filesystem marker; rerun the complete box gate" >&2
    exit 1
  fi
elif [ "$refs_publication_evidence_count" -ne 0 ] || [ "$refs_publication_filesystem_count" -ne 0 ]; then
  echo "ERROR: win-host-ci: unrequested ReFS publication receipt must not emit publication markers; rerun the complete box gate" >&2
  exit 1
fi
if [ "$refs_publication" -eq 1 ]; then
  refs_publication_evidence=$(printf '%s\n' "$normalized_output" | sed -n 's/^JOURNAL_WIN_CI_REFS_PUBLICATION=//p')
  refs_publication_filesystem=$(printf '%s\n' "$normalized_output" | sed -n 's/^JOURNAL_WIN_CI_REFS_PUBLICATION_FILESYSTEM=//p')
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
cloud_evidence_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=/ { print NR }')
ordinary_owner_evidence_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=/ { print NR }')
ordinary_owner_refs_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_ORDINARY_OWNER_REFS=/ { print NR }')
refs_publication_evidence_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_PUBLICATION=/ { print NR }')
refs_publication_filesystem_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_PUBLICATION_FILESYSTEM=/ { print NR }')
ok_line=$(printf '%s\n' "$normalized_output" | awk '/^=== JOURNAL_WIN_CI_OK:/ { print NR }')
if [ "$head_line" -ge "$ok_line" ] ||
  [ "$cargo_line" -ge "$ok_line" ] ||
  [ "$cloud_evidence_line" -ge "$ok_line" ] ||
  [ "$ordinary_owner_evidence_line" -ge "$ok_line" ]; then
  echo "ERROR: win-host-ci: source-binding, Cloud Files, and ordinary-owner evidence acknowledgements must precede JOURNAL_WIN_CI_OK; rerun the current box gate" >&2
  exit 1
fi
if [ "$refs_publication" -eq 1 ]; then
  if [ "$ordinary_owner_refs_line" -ge "$ok_line" ] ||
    [ "$refs_publication_evidence_line" -ge "$ok_line" ] ||
    [ "$refs_publication_filesystem_line" -ge "$ok_line" ]; then
    echo "ERROR: win-host-ci: required ReFS publication receipt acknowledgements must precede JOURNAL_WIN_CI_OK; rerun the current box gate" >&2
    exit 1
  fi
fi

echo "JOURNAL_WIN_HOST_CI_VERIFIED commit=$snapshot_sha cargo_lock_sha256=$cargo_lock_sha256 cloud_sync_evidence=$expected_cloud_evidence ordinary_owner_evidence=passed launch_environment_preparation=executed/pass launch_path_preparation=executed/pass job_list_no_handle_inheritance=executed/pass job_process_owner=executed/pass job_last_handle_negative=executed/pass ntfs_publication=executed/pass refs_publication=executed/pass ntfs_cortex_use=executed/pass refs_cortex_use=executed/pass ntfs_stale_heartbeat_cleanup=executed/pass refs_stale_heartbeat_cleanup=executed/pass"
