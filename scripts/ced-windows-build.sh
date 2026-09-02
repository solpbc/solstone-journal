#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Driver for one source-bound, network-denied CED Windows build slot. Inputs
# are verified here before transfer; the native runner re-verifies them before
# extraction and writes a pre-signing receipt. This does not produce a package,
# signature, installed tree, or runtime claim.

set -eu

GIT=${GIT:-git}
SCP=${SCP:-scp}
SSH=${SSH:-ssh}
WIN_REMOTE_HOST=${WIN_REMOTE_HOST:-}
EXPECTED_WIN_COMMIT=${EXPECTED_WIN_COMMIT:-}
CED_WINDOWS_SOURCE_ROOT=${CED_WINDOWS_SOURCE_ROOT:-}
WIN_REMOTE_HOME=${WIN_REMOTE_HOME:-'C:\Users\solbuild'}

require_lower_hex() {
  value=$1
  length=$2
  [ "${#value}" -eq "$length" ] || return 1
  case "$value" in
    *[!0-9a-f]*) return 1 ;;
  esac
}

if [ -z "$WIN_REMOTE_HOST" ]; then
  echo "ERROR: ced-windows-build: WIN_REMOTE_HOST is required" >&2
  exit 1
fi
if ! printf '%s\n' "$WIN_REMOTE_HOST" | grep -Eq '^[A-Za-z0-9_.@-]+$'; then
  echo "ERROR: ced-windows-build: WIN_REMOTE_HOST must be a safe user@host value" >&2
  exit 1
fi
if ! require_lower_hex "$EXPECTED_WIN_COMMIT" 40; then
  echo "ERROR: ced-windows-build: EXPECTED_WIN_COMMIT must be a full lowercase commit" >&2
  exit 1
fi
if [ -z "$CED_WINDOWS_SOURCE_ROOT" ] || [ ! -d "$CED_WINDOWS_SOURCE_ROOT" ]; then
  echo "ERROR: ced-windows-build: CED_WINDOWS_SOURCE_ROOT must name the clean pinned ced.cpp checkout" >&2
  exit 1
fi
if ! printf '%s\n' "$WIN_REMOTE_HOME" | grep -Eq '^[A-Za-z]:[\\/][A-Za-z0-9_. ()\\/:=-]*$'; then
  echo "ERROR: ced-windows-build: WIN_REMOTE_HOME must be a safe absolute Windows path" >&2
  exit 1
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
cd "$repo_root"

if [ -n "$("$GIT" status --porcelain=v1 --untracked-files=all --ignore-submodules=none)" ]; then
  echo "ERROR: ced-windows-build: source-bound build requires a clean Journal checkout" >&2
  exit 1
fi
if [ "$("$GIT" rev-parse HEAD)" != "$EXPECTED_WIN_COMMIT" ]; then
  echo "ERROR: ced-windows-build: HEAD does not equal EXPECTED_WIN_COMMIT" >&2
  exit 1
fi

cmake_archive="$repo_root/target/windows-builder-inputs/cmake-3.31.12-windows-x86_64.zip"
source_dir="$repo_root/target/ced-windows-inputs/$EXPECTED_WIN_COMMIT"
source_archive="$source_dir/ced.cpp-with-ggml.tar.gz"
local_slot="$repo_root/target/ced-windows-controlled-build/$EXPECTED_WIN_COMMIT"
if [ -e "$source_archive" ] || [ -e "$local_slot" ]; then
  echo "ERROR: ced-windows-build: refusing to reuse local CED input or output slot for $EXPECTED_WIN_COMMIT" >&2
  exit 1
fi

cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked -- acquire cmake-windows
mkdir -p "$source_dir"
cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked -- \
  ced-windows source-archive --source "$CED_WINDOWS_SOURCE_ROOT" --out "$source_archive"
source_sha256=$(sha256sum "$source_archive" | awk '{ print $1 }')
source_size=$(wc -c < "$source_archive" | tr -d '[:space:]')
if ! require_lower_hex "$source_sha256" 64 || [ "$source_size" -le 0 ]; then
  echo "ERROR: ced-windows-build: source archive identity could not be derived" >&2
  exit 1
fi

# Reuse the ordinary source-bound native gate to install the exact clean bundle
# and prove the current host rail before this distinct dependency slot starts.
make win-host-ci

remote_inputs_rel="ced-windows-inputs/$EXPECTED_WIN_COMMIT"
remote_slot_rel="ced-windows-controlled-build/$EXPECTED_WIN_COMMIT"
remote_inputs="$WIN_REMOTE_HOME\\ced-windows-inputs\\$EXPECTED_WIN_COMMIT"
remote_slot="$WIN_REMOTE_HOME\\ced-windows-controlled-build\\$EXPECTED_WIN_COMMIT"
remote_repo="$WIN_REMOTE_HOME\\sjbuild"

remote_prepare="\$ErrorActionPreference = 'Stop'
\$input = '$remote_inputs'
\$slot = '$remote_slot'
if (Test-Path -LiteralPath \$input) { throw 'remote CED input slot already exists' }
if (Test-Path -LiteralPath \$slot) { throw 'remote CED output slot already exists' }
New-Item -ItemType Directory -Path \$input | Out-Null"
"$SSH" \
  -o ControlMaster=auto \
  -o "ControlPath=/tmp/sj-%r@%h:%p" \
  -o ControlPersist=60s \
  "$WIN_REMOTE_HOST" \
  "powershell -NoProfile -Command \"$remote_prepare\""

"$SCP" \
  -o ControlMaster=auto \
  -o "ControlPath=/tmp/sj-%r@%h:%p" \
  -o ControlPersist=60s \
  "$source_archive" \
  "$cmake_archive" \
  "$WIN_REMOTE_HOST:$remote_inputs_rel/"

remote_command="& '$remote_repo\\core\\distribution\\ced-windows-build.ps1' \
  -RepositoryRoot '$remote_repo' \
  -SourceArchive '$remote_inputs\\ced.cpp-with-ggml.tar.gz' \
  -SourceSha256 '$source_sha256' \
  -SourceSize $source_size \
  -CmakeArchive '$remote_inputs\\cmake-3.31.12-windows-x86_64.zip' \
  -WorkRoot '$remote_slot\\work' \
  -OutputRoot '$remote_slot\\output' \
  -ReportRoot '$remote_slot\\report' \
  -ExpectedProductCommit '$EXPECTED_WIN_COMMIT' \
  -ExpectedCargoLockSha256 '$(sha256sum core/Cargo.lock | awk '{ print $1 }')' \
  -BuilderHost '$WIN_REMOTE_HOST'"
"$SSH" \
  -o ControlMaster=auto \
  -o "ControlPath=/tmp/sj-%r@%h:%p" \
  -o ControlPersist=60s \
  "$WIN_REMOTE_HOST" \
  "powershell -NoProfile -ExecutionPolicy Bypass -Command \"$remote_command\""

mkdir -p "$local_slot/output/bin" "$local_slot/report"
"$SCP" \
  -o ControlMaster=auto \
  -o "ControlPath=/tmp/sj-%r@%h:%p" \
  -o ControlPersist=60s \
  "$WIN_REMOTE_HOST:$remote_slot_rel/output/bin/ced.dll" \
  "$local_slot/output/bin/ced.dll"
"$SCP" \
  -o ControlMaster=auto \
  -o "ControlPath=/tmp/sj-%r@%h:%p" \
  -o ControlPersist=60s \
  "$WIN_REMOTE_HOST:$remote_slot_rel/report/ced-build-evidence.json" \
  "$WIN_REMOTE_HOST:$remote_slot_rel/report/ced-build-receipt.json" \
  "$WIN_REMOTE_HOST:$remote_slot_rel/report/ced-build-validation.log" \
  "$local_slot/report/"

cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked -- \
  ced-windows verify --receipt "$local_slot/report/ced-build-receipt.json" --output-root "$local_slot/output"

remote_cleanup="\$ErrorActionPreference = 'Stop'
Remove-Item -LiteralPath '$remote_inputs' -Recurse -Force
Remove-Item -LiteralPath '$remote_slot' -Recurse -Force"
"$SSH" \
  -o ControlMaster=auto \
  -o "ControlPath=/tmp/sj-%r@%h:%p" \
  -o ControlPersist=60s \
  "$WIN_REMOTE_HOST" \
  "powershell -NoProfile -Command \"$remote_cleanup\""

echo "CED_WINDOWS_DRIVER_OK commit=$EXPECTED_WIN_COMMIT source_sha256=$source_sha256 source_size=$source_size output=$local_slot/output/bin/ced.dll receipt=$local_slot/report/ced-build-receipt.json"
