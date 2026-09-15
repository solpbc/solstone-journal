#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Places the pinned FFmpeg Windows build toolchain on the native build host for
# the journal gate.
#
# `solstone-distribution acquire ffmpeg-windows-tools` is the one fetch path for
# these archives: it reads the pins from core/distribution/builder-inputs.toml
# and sha256-verifies every byte it writes. It does not run on Windows, so the
# driver acquires and transfers, exactly as the controlled producer driver does.
# The box re-verifies what arrives against its own transferred checkout before
# staging anything, so this script is transport and nothing else.
#
# Transfers are content-skipping and crash-safe: an archive already on the host
# with the right digest is left alone, and a new one lands under a temporary
# name that is only renamed into place after the host has hashed it.

set -eu

GIT=${GIT:-git}
SCP=${SCP:-scp}
SSH=${SSH:-ssh}
WIN_REMOTE_HOST=${WIN_REMOTE_HOST:-}
# Relative to the build host's home directory, and it must agree with
# scripts/win-ci.cmd's JOURNAL_WIN_CI_FFMPEG_INPUT_ROOT default
# (%USERPROFILE%\sj-ffmpeg-tools\inputs) -- the gate looks for the archives
# there and refuses rather than fetching if they are absent.
WIN_FFMPEG_INPUT_ROOT=${WIN_FFMPEG_INPUT_ROOT:-sj-ffmpeg-tools/inputs}

if [ -z "$WIN_REMOTE_HOST" ] || ! printf '%s\n' "$WIN_REMOTE_HOST" | grep -Eq '^[A-Za-z0-9_.@-]+$'; then
  echo 'ERROR: sync-win-ffmpeg-tools: WIN_REMOTE_HOST must be a safe user@host value' >&2
  exit 1
fi
if ! printf '%s\n' "$WIN_FFMPEG_INPUT_ROOT" | grep -Eq '^[A-Za-z0-9_.-]+(/[A-Za-z0-9_.-]+)*$'; then
  echo 'ERROR: sync-win-ffmpeg-tools: WIN_FFMPEG_INPUT_ROOT must be a safe relative path' >&2
  exit 1
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
cd "$repo_root"

input_root=$repo_root/target/windows-ffmpeg-builder-inputs
acquire_output=$(cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution \
  --bin solstone-distribution --locked -- acquire ffmpeg-windows-tools --dest "$input_root")
printf '%s\n' "$acquire_output"

# `acquire` names every archive it admitted; the count is the check that the
# pin table still carries the four this gate stages.
filenames=$(printf '%s\n' "$acquire_output" |
  sed -n 's/^ffmpeg-windows-tool \([A-Za-z0-9_.+-]*\) \(fetched\|cached\) dest=.*$/\1/p')
filename_count=$(printf '%s\n' "$filenames" | grep -c '[^[:space:]]' || true)
if [ "$filename_count" -ne 4 ]; then
  echo "ERROR: sync-win-ffmpeg-tools: expected four acquired tool archives, found $filename_count" >&2
  exit 1
fi

remote_root_literal=$(printf '%s' "$WIN_FFMPEG_INPUT_ROOT" | sed "s/'/''/g" | tr '/' '\\')
remote_probe="\$ErrorActionPreference = 'Stop'
\$root = Join-Path \$env:USERPROFILE '$remote_root_literal'
if (-not (Test-Path -LiteralPath \$root)) { New-Item -ItemType Directory -Path \$root -Force | Out-Null }
foreach (\$file in @(Get-ChildItem -LiteralPath \$root -File)) {
  Write-Output (\$file.Name + ' ' + (Get-FileHash -LiteralPath \$file.FullName -Algorithm SHA256).Hash.ToLowerInvariant())
}"
# No pipe between the probe and its result: the exit status read here is the
# ssh invocation's own, so a failed probe stops the sync instead of reading as
# a host that holds no archives.
remote_digests=$("$SSH" \
  -o ControlMaster=auto \
  -o "ControlPath=/tmp/sj-%r@%h:%p" \
  -o ControlPersist=60s \
  "$WIN_REMOTE_HOST" "$remote_probe")
remote_digests=$(printf '%s\n' "$remote_digests" | tr -d '\r')

for filename in $filenames; do
  archive=$input_root/$filename
  if [ ! -f "$archive" ]; then
    echo "ERROR: sync-win-ffmpeg-tools: acquired archive absent: $archive" >&2
    exit 1
  fi
  digest=$(sha256sum "$archive" | awk '{ print $1 }')
  if [ "${#digest}" -ne 64 ]; then
    echo "ERROR: sync-win-ffmpeg-tools: unreadable digest for $filename" >&2
    exit 1
  fi
  if printf '%s\n' "$remote_digests" | grep -qx "$filename $digest"; then
    echo "SYNC_WIN_FFMPEG_TOOL present $filename sha256=$digest"
    continue
  fi
  incoming="$WIN_FFMPEG_INPUT_ROOT/.incoming-$$-$filename"
  echo "SYNC_WIN_FFMPEG_TOOL transferring $filename sha256=$digest"
  "$SCP" \
    -o ControlMaster=auto \
    -o "ControlPath=/tmp/sj-%r@%h:%p" \
    -o ControlPersist=60s \
    "$archive" "$WIN_REMOTE_HOST:$incoming"
  incoming_literal=$(printf '%s' "$incoming" | sed "s/'/''/g" | tr '/' '\\')
  filename_literal=$(printf '%s' "$filename" | sed "s/'/''/g")
  # The host hashes what it actually received; a short or interrupted transfer
  # is deleted there rather than published under the real name.
  remote_publish="\$ErrorActionPreference = 'Stop'
\$incoming = Join-Path \$env:USERPROFILE '$incoming_literal'
\$final = Join-Path (Join-Path \$env:USERPROFILE '$remote_root_literal') '$filename_literal'
\$actual = (Get-FileHash -LiteralPath \$incoming -Algorithm SHA256).Hash.ToLowerInvariant()
if (\$actual -cne '$digest') {
  Remove-Item -LiteralPath \$incoming -Force
  throw ('transferred archive digest ' + \$actual + ' does not match the acquired archive')
}
Move-Item -LiteralPath \$incoming -Destination \$final -Force
Write-Output ('SYNC_WIN_FFMPEG_TOOL published $filename_literal sha256=' + \$actual)"
  publish_output=$("$SSH" \
    -o ControlMaster=auto \
    -o "ControlPath=/tmp/sj-%r@%h:%p" \
    -o ControlPersist=60s \
    "$WIN_REMOTE_HOST" "$remote_publish")
  printf '%s\n' "$publish_output" | tr -d '\r'
done

echo "SYNC_WIN_FFMPEG_TOOLS_OK archives=$filename_count remote=$WIN_FFMPEG_INPUT_ROOT"
