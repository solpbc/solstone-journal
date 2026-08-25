#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

set -eu

GIT=${GIT:-git}
SCP=${SCP:-scp}

phase=initialize
snapshot_sha=
cargo_lock_sha256=
did_create_sync_ref=0
bundle_file=
binding_temp_file=
cargo_lock_temp_file=
status_temp_file=

cleanup() {
  original_status=$1
  trap - EXIT HUP INT TERM
  set +e

  if [ "$did_create_sync_ref" -eq 1 ]; then
    if ! "$GIT" update-ref -d refs/heads/__sjwsync "$snapshot_sha"; then
      echo "WARNING: sync-win-host: cleanup failed for refs/heads/__sjwsync; preserving $phase exit $original_status" >&2
    fi
  fi
  for cleanup_file in \
    "$bundle_file" \
    "$binding_temp_file" \
    "$cargo_lock_temp_file" \
    "$status_temp_file"
  do
    if [ -n "$cleanup_file" ] && ! rm -f "$cleanup_file"; then
      echo "WARNING: sync-win-host: cleanup failed for $cleanup_file; preserving $phase exit $original_status" >&2
    fi
  done

  exit "$original_status"
}

trap 'cleanup $?' EXIT
trap 'cleanup 129' HUP
trap 'cleanup 130' INT
trap 'cleanup 143' TERM

fail_phase() {
  failed_phase=$1
  failed_status=$2
  echo "ERROR: sync-win-host: $failed_phase failed" >&2
  if [ "$failed_status" -eq 0 ]; then
    exit 1
  fi
  exit "$failed_status"
}

is_lower_hex() {
  candidate=$1
  expected_length=$2
  [ "${#candidate}" -eq "$expected_length" ] || return 1
  case "$candidate" in
    *[!0-9a-f]*) return 1 ;;
  esac
}

if script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd) &&
  repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd); then
  :
else
  fail_phase "$phase" "$?"
fi
WIN_CI_BINDING_FILE=${WIN_CI_BINDING_FILE:-"$repo_root/target/win-host-ci-source-binding.json"}
binding_dir=$(dirname -- "$WIN_CI_BINDING_FILE")

if [ -z "${WIN_REMOTE_HOST:-}" ]; then
  echo "WIN_REMOTE_HOST is required" >&2
  fail_phase "$phase" 1
fi
if mkdir -p "$repo_root/target" "$binding_dir" && rm -f "$WIN_CI_BINDING_FILE"; then
  :
else
  fail_phase "$phase" "$?"
fi
cd "$repo_root"

phase=guard
if GIT="$GIT" sh "$script_dir/check-win-sync-tree.sh"; then
  :
else
  fail_phase "$phase" "$?"
fi

phase=resolve-sha
if [ -n "${EXPECTED_WIN_COMMIT:-}" ]; then
  if ! is_lower_hex "$EXPECTED_WIN_COMMIT" 40; then
    echo "ERROR: sync-win-host: EXPECTED_WIN_COMMIT must be the full lowercase 40-hex commit; correct it and retry" >&2
    fail_phase "$phase" 1
  fi
  if status_temp_file=$(mktemp "$repo_root/target/win-host-ci.status.XXXXXX") &&
    "$GIT" status --porcelain=v1 -z --untracked-files=all --ignore-submodules=none >"$status_temp_file"; then
    :
  else
    fail_phase "$phase" "$?"
  fi
  if [ -s "$status_temp_file" ]; then
    echo "ERROR: sync-win-host: pinned mode refuses a synthetic or dirty snapshot; restore a clean checkout at EXPECTED_WIN_COMMIT and retry" >&2
    fail_phase "$phase" 1
  fi
  if snapshot_sha=$("$GIT" rev-parse HEAD); then
    :
  else
    fail_phase "$phase" "$?"
  fi
  if [ "$snapshot_sha" != "$EXPECTED_WIN_COMMIT" ]; then
    echo "ERROR: sync-win-host: HEAD does not equal EXPECTED_WIN_COMMIT; check out the exact commit and retry" >&2
    fail_phase "$phase" 1
  fi
else
  if snapshot_sha=$("$GIT" stash create); then
    :
  else
    fail_phase "$phase" "$?"
  fi
  if [ -z "$snapshot_sha" ]; then
    if snapshot_sha=$("$GIT" rev-parse HEAD); then
      :
    else
      fail_phase "$phase" "$?"
    fi
  fi
fi
if ! is_lower_hex "$snapshot_sha" 40; then
  echo "ERROR: sync-win-host: snapshot commit is not full lowercase 40-hex; repair the local Git checkout and retry" >&2
  fail_phase "$phase" 1
fi

phase=resolve-binding
if cargo_lock_temp_file=$(mktemp "$repo_root/target/win-host-ci.cargo-lock.XXXXXX") &&
  "$GIT" show "$snapshot_sha:core/Cargo.lock" >"$cargo_lock_temp_file"; then
  :
else
  echo "ERROR: sync-win-host: exact snapshot core/Cargo.lock is unavailable; restore the tracked lockfile and retry" >&2
  fail_phase "$phase" "$?"
fi
if cargo_lock_sha256=$(sha256sum "$cargo_lock_temp_file" | awk '{ print $1 }'); then
  :
else
  fail_phase "$phase" "$?"
fi
if ! is_lower_hex "$cargo_lock_sha256" 64; then
  echo "ERROR: sync-win-host: snapshot Cargo.lock digest is not lowercase SHA-256; repair sha256sum and retry" >&2
  fail_phase "$phase" 1
fi

phase=create-temp-ref
if "$GIT" update-ref refs/heads/__sjwsync "$snapshot_sha" ""; then
  did_create_sync_ref=1
else
  fail_phase "$phase" "$?"
fi

phase=create-bundle
if bundle_file=$(mktemp "$repo_root/target/win-host-ci.bundle.XXXXXX") &&
  "$GIT" bundle create "$bundle_file" refs/heads/__sjwsync; then
  :
else
  fail_phase "$phase" "$?"
fi

phase=verify-bundle
if "$GIT" bundle verify "$bundle_file"; then
  :
else
  fail_phase "$phase" "$?"
fi
if bundle_heads=$("$GIT" bundle list-heads "$bundle_file"); then
  :
else
  fail_phase "$phase" "$?"
fi
if [ "$bundle_heads" != "$snapshot_sha refs/heads/__sjwsync" ]; then
  fail_phase "$phase" 1
fi

phase=delete-temp-ref
if "$GIT" update-ref -d refs/heads/__sjwsync "$snapshot_sha"; then
  did_create_sync_ref=0
else
  fail_phase "$phase" "$?"
fi

phase=materialize-binding
if binding_temp_file=$(mktemp "$binding_dir/.win-host-ci-source-binding.json.XXXXXX") &&
  printf '{\n  "schema": "solstone.journal.win-source-binding.v1",\n  "commit": "%s",\n  "cargo_lock_sha256": "%s"\n}\n' \
    "$snapshot_sha" \
    "$cargo_lock_sha256" >"$binding_temp_file"; then
  :
else
  fail_phase "$phase" "$?"
fi

phase=write-binding
if mv "$binding_temp_file" "$WIN_CI_BINDING_FILE"; then
  binding_temp_file=
else
  fail_phase "$phase" "$?"
fi

phase=scp
if "$SCP" \
  -o ControlMaster=auto \
  -o "ControlPath=/tmp/sj-%r@%h:%p" \
  -o ControlPersist=60s \
  "$bundle_file" \
  "$WIN_REMOTE_HOST:sjbuild.bundle"; then
  :
else
  fail_phase "$phase" "$?"
fi

phase=scp-binding
if "$SCP" \
  -o ControlMaster=auto \
  -o "ControlPath=/tmp/sj-%r@%h:%p" \
  -o ControlPersist=60s \
  "$WIN_CI_BINDING_FILE" \
  "$WIN_REMOTE_HOST:journal-win-host-ci-source-binding.json"; then
  :
else
  fail_phase "$phase" "$?"
fi

phase=success
echo "SYNC_WIN_HOST_OK commit=$snapshot_sha cargo_lock_sha256=$cargo_lock_sha256 remote=sjbuild.bundle binding=journal-win-host-ci-source-binding.json"
