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
refs_requested=0
refs_enumeration_evidence=unrun/skipped
refs_enumeration_capability=not-asserted
refs_revalidation_evidence=unrun/skipped
refs_revalidation_capability=not-asserted
refs_claimed_removal_evidence=unrun/skipped
refs_claimed_removal_capability=unsupported
refs_archive_traversal_evidence=unrun/skipped
refs_archive_traversal_capability=not-asserted

case "$cloud_sync_test" in
  '') cloud_sync_test=0 ;;
  0|1) ;;
  *)
    echo "ERROR: win-host-ci: JOURNAL_WIN_CI_RUN_CLOUD_SYNC_TEST must be unset, empty, 0, or 1" >&2
    exit 1
    ;;
esac

if [ -n "$refs_root" ]; then
  if printf '%s\n' "$refs_root" | grep -Eq '^[A-Za-z]:[\\/][A-Za-z0-9_. ()\\/:=-]*$'; then
    refs_requested=1
  else
    echo "win-host-ci: SOLSTONE_JOURNAL_WIN_REFS_ROOT is not a safe absolute Windows path; ReFS matrix evidence will be skipped" >&2
    refs_root=
  fi
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
head_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_HEAD=/ { count++ } END { print count + 0 }')
cargo_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_CARGO_LOCK_SHA256=/ { count++ } END { print count + 0 }')
cloud_evidence_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=/ { count++ } END { print count + 0 }')
ordinary_owner_evidence_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=/ { count++ } END { print count + 0 }')
refs_enumeration_evidence_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE=/ { count++ } END { print count + 0 }')
refs_enumeration_capability_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY=/ { count++ } END { print count + 0 }')
refs_revalidation_evidence_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE=/ { count++ } END { print count + 0 }')
refs_revalidation_capability_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY=/ { count++ } END { print count + 0 }')
refs_claimed_removal_evidence_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE=/ { count++ } END { print count + 0 }')
refs_claimed_removal_capability_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY=/ { count++ } END { print count + 0 }')
refs_archive_traversal_evidence_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE=/ { count++ } END { print count + 0 }')
refs_archive_traversal_capability_count=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY=/ { count++ } END { print count + 0 }')
if [ "$cloud_sync_test" -eq 1 ]; then
  expected_cloud_evidence=passed
else
  expected_cloud_evidence=skipped
fi
expected_cloud_evidence_count=$(printf '%s\n' "$normalized_output" | awk -v expected="$expected_cloud_evidence" '$0 == "JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=" expected { count++ } END { print count + 0 }')
ordinary_owner_passed_count=$(printf '%s\n' "$normalized_output" | awk '$0 == "JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=passed" { count++ } END { print count + 0 }')
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
if [ "$refs_requested" -eq 1 ]; then
  if [ "$refs_enumeration_evidence_count" -ne 1 ] ||
    [ "$refs_enumeration_capability_count" -ne 1 ] ||
    [ "$refs_revalidation_evidence_count" -ne 1 ] ||
    [ "$refs_revalidation_capability_count" -ne 1 ] ||
    [ "$refs_claimed_removal_evidence_count" -ne 1 ] ||
    [ "$refs_claimed_removal_capability_count" -ne 1 ] ||
    [ "$refs_archive_traversal_evidence_count" -ne 1 ] ||
    [ "$refs_archive_traversal_capability_count" -ne 1 ]; then
    echo "ERROR: win-host-ci: requested ReFS fixture requires exactly one marker for every matrix row and capability; rerun the complete box gate" >&2
    exit 1
  fi
elif [ "$refs_enumeration_evidence_count" -ne 0 ] ||
  [ "$refs_enumeration_capability_count" -ne 0 ] ||
  [ "$refs_revalidation_evidence_count" -ne 0 ] ||
  [ "$refs_revalidation_capability_count" -ne 0 ] ||
  [ "$refs_claimed_removal_evidence_count" -ne 0 ] ||
  [ "$refs_claimed_removal_capability_count" -ne 0 ] ||
  [ "$refs_archive_traversal_evidence_count" -ne 0 ] ||
  [ "$refs_archive_traversal_capability_count" -ne 0 ]; then
  echo "ERROR: win-host-ci: unrequested ReFS fixture must not emit matrix markers; rerun the complete box gate" >&2
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
if [ "$refs_requested" -eq 1 ]; then
  refs_enumeration_evidence=$(printf '%s\n' "$normalized_output" | sed -n 's/^JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE=//p')
  refs_enumeration_capability=$(printf '%s\n' "$normalized_output" | sed -n 's/^JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY=//p')
  refs_revalidation_evidence=$(printf '%s\n' "$normalized_output" | sed -n 's/^JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE=//p')
  refs_revalidation_capability=$(printf '%s\n' "$normalized_output" | sed -n 's/^JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY=//p')
  refs_claimed_removal_evidence=$(printf '%s\n' "$normalized_output" | sed -n 's/^JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE=//p')
  refs_claimed_removal_capability=$(printf '%s\n' "$normalized_output" | sed -n 's/^JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY=//p')
  refs_archive_traversal_evidence=$(printf '%s\n' "$normalized_output" | sed -n 's/^JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE=//p')
  refs_archive_traversal_capability=$(printf '%s\n' "$normalized_output" | sed -n 's/^JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY=//p')
  if [ "$refs_claimed_removal_evidence" != "unrun/skipped" ] || [ "$refs_claimed_removal_capability" != "unsupported" ]; then
    echo "ERROR: win-host-ci: Windows claimed-removal must remain unrun/skipped and unsupported" >&2
    exit 1
  fi
  if [ "$refs_enumeration_evidence" != "executed/pass" ] ||
    [ "$refs_enumeration_capability" != "available" ] ||
    [ "$refs_revalidation_evidence" != "executed/pass" ] ||
    [ "$refs_revalidation_capability" != "available" ] ||
    [ "$refs_archive_traversal_evidence" != "executed/pass" ] ||
    [ "$refs_archive_traversal_capability" != "available" ]; then
    echo "ERROR: win-host-ci: requested ReFS fixture did not produce complete available enumeration, revalidation, and archive-traversal evidence" >&2
    exit 1
  fi
fi

head_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_HEAD=/ { print NR }')
cargo_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_CARGO_LOCK_SHA256=/ { print NR }')
cloud_evidence_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_CLOUD_SYNC_EVIDENCE=/ { print NR }')
ordinary_owner_evidence_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_ORDINARY_OWNER_EVIDENCE=/ { print NR }')
ok_line=$(printf '%s\n' "$normalized_output" | awk '/^=== JOURNAL_WIN_CI_OK:/ { print NR }')
if [ "$head_line" -ge "$ok_line" ] ||
  [ "$cargo_line" -ge "$ok_line" ] ||
  [ "$cloud_evidence_line" -ge "$ok_line" ] ||
  [ "$ordinary_owner_evidence_line" -ge "$ok_line" ]; then
  echo "ERROR: win-host-ci: source-binding, Cloud Files, and ordinary-owner evidence acknowledgements must precede JOURNAL_WIN_CI_OK; rerun the current box gate" >&2
  exit 1
fi
if [ "$refs_requested" -eq 1 ]; then
  refs_enumeration_evidence_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_ENUMERATION_EVIDENCE=/ { print NR }')
  refs_enumeration_capability_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_ENUMERATION_CAPABILITY=/ { print NR }')
  refs_revalidation_evidence_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_REVALIDATION_EVIDENCE=/ { print NR }')
  refs_revalidation_capability_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_REVALIDATION_CAPABILITY=/ { print NR }')
  refs_claimed_removal_evidence_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_EVIDENCE=/ { print NR }')
  refs_claimed_removal_capability_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_CLAIMED_REMOVAL_CAPABILITY=/ { print NR }')
  refs_archive_traversal_evidence_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_EVIDENCE=/ { print NR }')
  refs_archive_traversal_capability_line=$(printf '%s\n' "$normalized_output" | awk '/^JOURNAL_WIN_CI_REFS_ARCHIVE_TRAVERSAL_CAPABILITY=/ { print NR }')
  if [ "$refs_enumeration_evidence_line" -ge "$ok_line" ] ||
    [ "$refs_enumeration_capability_line" -ge "$ok_line" ] ||
    [ "$refs_revalidation_evidence_line" -ge "$ok_line" ] ||
    [ "$refs_revalidation_capability_line" -ge "$ok_line" ] ||
    [ "$refs_claimed_removal_evidence_line" -ge "$ok_line" ] ||
    [ "$refs_claimed_removal_capability_line" -ge "$ok_line" ] ||
    [ "$refs_archive_traversal_evidence_line" -ge "$ok_line" ] ||
    [ "$refs_archive_traversal_capability_line" -ge "$ok_line" ]; then
    echo "ERROR: win-host-ci: ReFS matrix evidence acknowledgements must precede JOURNAL_WIN_CI_OK; rerun the current box gate" >&2
    exit 1
  fi
fi

echo "JOURNAL_WIN_HOST_CI_VERIFIED commit=$snapshot_sha cargo_lock_sha256=$cargo_lock_sha256 cloud_sync_evidence=$expected_cloud_evidence ordinary_owner_evidence=passed refs_enumeration_evidence=$refs_enumeration_evidence refs_enumeration_capability=$refs_enumeration_capability refs_revalidation_evidence=$refs_revalidation_evidence refs_revalidation_capability=$refs_revalidation_capability refs_claimed_removal_evidence=$refs_claimed_removal_evidence refs_claimed_removal_capability=$refs_claimed_removal_capability refs_archive_traversal_evidence=$refs_archive_traversal_evidence refs_archive_traversal_capability=$refs_archive_traversal_capability"
