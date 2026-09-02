#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Source-bound driver for the controlled Windows parakeet.cpp build. It moves
# only verified driver inputs into one new network-denied Windows slot, then
# independently rehashes the returned pre-signing evidence. It does not
# package, sign, launch, or advertise a provider.

set -euo pipefail

: "${EXPECTED_WIN_COMMIT:?set EXPECTED_WIN_COMMIT to the exact Journal commit}"
: "${WIN_REMOTE_HOST:?set WIN_REMOTE_HOST to the Windows build account}"
: "${WIN_REMOTE_HOME:?set WIN_REMOTE_HOME to the Windows build-home path}"
: "${SOLSTONE_JOURNAL_WIN_REFS_ROOT:?set SOLSTONE_JOURNAL_WIN_REFS_ROOT}"
: "${SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT:?set SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT}"
: "${PARAKEET_WINDOWS_SOURCE_ROOT:?set PARAKEET_WINDOWS_SOURCE_ROOT to the patched source checkout}"
: "${PARAKEET_WINDOWS_MODEL_PATH:?set PARAKEET_WINDOWS_MODEL_PATH to the pinned GGUF}"

SSH=${SSH:-ssh}
SCP=${SCP:-scp}
GIT=${GIT:-git}

printf '%s\n' "$EXPECTED_WIN_COMMIT" | grep -Eq '^[0-9a-f]{40}$' || {
  echo 'ERROR: EXPECTED_WIN_COMMIT must be a lowercase full commit SHA' >&2
  exit 1
}
printf '%s\n' "$WIN_REMOTE_HOME" | grep -Eq '^[A-Za-z]:[\\/][A-Za-z0-9_. ()\\/:=-]*$' || {
  echo 'ERROR: WIN_REMOTE_HOME must be a safe absolute Windows path' >&2
  exit 1
}
printf '%s\n' "$SOLSTONE_JOURNAL_WIN_REFS_ROOT" | grep -Eq '^[A-Za-z]:[\\/][A-Za-z0-9_. ()\\/:=-]*$' || {
  echo 'ERROR: SOLSTONE_JOURNAL_WIN_REFS_ROOT must be a safe absolute Windows path' >&2
  exit 1
}
printf '%s\n' "$SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT" | grep -Eq '^[A-Za-z0-9_.-]+$' || {
  echo 'ERROR: SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT must be a safe account name' >&2
  exit 1
}

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
cd "$repo_root"

[ -z "$("$GIT" status --porcelain=v1 --untracked-files=all --ignore-submodules=none)" ] || {
  echo 'ERROR: source-bound build requires a clean Journal checkout' >&2
  exit 1
}
[ "$("$GIT" rev-parse HEAD)" = "$EXPECTED_WIN_COMMIT" ] || {
  echo 'ERROR: HEAD does not equal EXPECTED_WIN_COMMIT' >&2
  exit 1
}
[ -d "$PARAKEET_WINDOWS_SOURCE_ROOT/.git" ] || {
  echo 'ERROR: PARAKEET_WINDOWS_SOURCE_ROOT must be a Git checkout' >&2
  exit 1
}
[ -f "$PARAKEET_WINDOWS_MODEL_PATH" ] || {
  echo 'ERROR: PARAKEET_WINDOWS_MODEL_PATH must be a regular file' >&2
  exit 1
}

input_dir="$repo_root/target/parakeet-windows-inputs/$EXPECTED_WIN_COMMIT"
slot="$repo_root/target/parakeet-windows-controlled-build/$EXPECTED_WIN_COMMIT"
source_archive="$input_dir/parakeet.cpp-patched-source.tar.gz"
cmake_archive="$repo_root/target/windows-builder-inputs/cmake-3.31.12-windows-x86_64.zip"
[ ! -e "$input_dir" ] && [ ! -e "$slot" ] || {
  echo 'ERROR: refusing to reuse Parakeet input or output slot' >&2
  exit 1
}

cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked -- acquire cmake-windows
mkdir -p "$input_dir"
cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked -- parakeet-windows source-archive --source "$PARAKEET_WINDOWS_SOURCE_ROOT" --out "$source_archive"
cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked -- parakeet-windows verify-inputs --source-archive "$source_archive" --cmake-archive "$cmake_archive" --model "$PARAKEET_WINDOWS_MODEL_PATH"

SOLSTONE_JOURNAL_WIN_REFS_ROOT="$SOLSTONE_JOURNAL_WIN_REFS_ROOT" \
SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT="$SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT" \
make win-host-ci

short_commit=$(printf '%s' "$EXPECTED_WIN_COMMIT" | cut -c 1-12)
remote_rel="parakeet-windows-controlled-build/$EXPECTED_WIN_COMMIT"
remote_repo="$WIN_REMOTE_HOME\\sjbuild"
remote_source="$WIN_REMOTE_HOME\\parakeet-source-$EXPECTED_WIN_COMMIT.tar.gz"
remote_cmake="$WIN_REMOTE_HOME\\parakeet-cmake-$EXPECTED_WIN_COMMIT.zip"
remote_model="$WIN_REMOTE_HOME\\parakeet-model-$EXPECTED_WIN_COMMIT.gguf"
remote_slot="$WIN_REMOTE_HOME\\parakeet-windows-controlled-build\\$EXPECTED_WIN_COMMIT"
remote_work="$WIN_REMOTE_HOME\\parakeet-work-$short_commit"
remote_guard="if (Test-Path -LiteralPath '$remote_source') { throw 'remote Parakeet source input already exists' }; if (Test-Path -LiteralPath '$remote_cmake') { throw 'remote Parakeet CMake input already exists' }; if (Test-Path -LiteralPath '$remote_model') { throw 'remote Parakeet model input already exists' }; if (Test-Path -LiteralPath '$remote_slot') { throw 'remote Parakeet output slot already exists' }; if (Test-Path -LiteralPath '$remote_work') { throw 'remote Parakeet work root already exists' }"
"$SSH" "$WIN_REMOTE_HOST" "powershell -NoProfile -Command \"$remote_guard\""

"$SCP" "$source_archive" "$WIN_REMOTE_HOST:parakeet-source-$EXPECTED_WIN_COMMIT.tar.gz"
"$SCP" "$cmake_archive" "$WIN_REMOTE_HOST:parakeet-cmake-$EXPECTED_WIN_COMMIT.zip"
"$SCP" "$PARAKEET_WINDOWS_MODEL_PATH" "$WIN_REMOTE_HOST:parakeet-model-$EXPECTED_WIN_COMMIT.gguf"

remote_command="& '$remote_repo\\core\\distribution\\parakeet-windows-build.ps1' -RepositoryRoot '$remote_repo' -SourceArchive '$remote_source' -CmakeArchive '$remote_cmake' -Model '$remote_model' -WorkRoot '$remote_work' -OutputRoot '$remote_slot\\output' -ReportRoot '$remote_slot\\report' -ExpectedProductCommit '$EXPECTED_WIN_COMMIT' -ExpectedCargoLockSha256 '$(sha256sum core/Cargo.lock | awk '{ print $1 }')' -BuilderHost '$WIN_REMOTE_HOST'"
"$SSH" "$WIN_REMOTE_HOST" "powershell -NoProfile -ExecutionPolicy Bypass -Command \"$remote_command\""

mkdir -p "$slot/output/bin" "$slot/output/models" "$slot/report"
"$SCP" "$WIN_REMOTE_HOST:$remote_rel/output/bin/parakeet-server.exe" "$slot/output/bin/parakeet-server.exe"
"$SCP" "$WIN_REMOTE_HOST:$remote_rel/output/models/tdt-0.6b-v3-q8_0.gguf" "$slot/output/models/tdt-0.6b-v3-q8_0.gguf"
"$SCP" "$WIN_REMOTE_HOST:$remote_rel/report/parakeet-build-evidence.json" "$WIN_REMOTE_HOST:$remote_rel/report/parakeet-build-receipt.json" "$WIN_REMOTE_HOST:$remote_rel/report/parakeet-build-validation.log" "$slot/report/"
cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked -- parakeet-windows verify --receipt "$slot/report/parakeet-build-receipt.json" --output-root "$slot/output"

remote_cleanup="\$ErrorActionPreference = 'Stop'; Remove-Item -LiteralPath '$remote_source' -Force; Remove-Item -LiteralPath '$remote_cmake' -Force; Remove-Item -LiteralPath '$remote_model' -Force; Remove-Item -LiteralPath '$remote_work' -Recurse -Force; Remove-Item -LiteralPath '$remote_slot' -Recurse -Force; foreach (\$path in @('$remote_source', '$remote_cmake', '$remote_model', '$remote_work', '$remote_slot')) { if (Test-Path -LiteralPath \$path) { throw \"remote Parakeet cleanup left path: \$path\" } }"
remote_cleanup_encoded=$(printf '%s' "$remote_cleanup" | iconv -f UTF-8 -t UTF-16LE | base64 | tr -d '\r\n')
[ -n "$remote_cleanup_encoded" ] || {
  echo 'ERROR: could not encode remote Parakeet cleanup command' >&2
  exit 1
}
"$SSH" "$WIN_REMOTE_HOST" "powershell -NoProfile -EncodedCommand $remote_cleanup_encoded"
echo "PARAKEET_WINDOWS_DRIVER_OK commit=$EXPECTED_WIN_COMMIT server=$slot/output/bin/parakeet-server.exe receipt=$slot/report/parakeet-build-receipt.json"
