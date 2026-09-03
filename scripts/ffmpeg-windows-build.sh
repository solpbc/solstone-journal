#!/usr/bin/env sh
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
#
# Driver for one fresh, source-bound, network-denied FFmpeg Windows build.
# The five admitted archives are verified locally, transferred into a single
# fresh native slot, verified again there, and removed from the host after the
# receipt and the two pre-signing PE files return to this checkout.

set -eu

GIT=${GIT:-git}
SCP=${SCP:-scp}
SSH=${SSH:-ssh}
WIN_REMOTE_HOST=${WIN_REMOTE_HOST:-}
EXPECTED_WIN_COMMIT=${EXPECTED_WIN_COMMIT:-}
WIN_REMOTE_HOME=${WIN_REMOTE_HOME:-'C:\Users\solbuild'}
SOLSTONE_JOURNAL_WIN_REFS_ROOT=${SOLSTONE_JOURNAL_WIN_REFS_ROOT:-}
SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT=${SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT:-}

require_lower_hex() {
  value=$1
  length=$2
  [ "${#value}" -eq "$length" ] || return 1
  case "$value" in
    *[!0-9a-f]*) return 1 ;;
  esac
}

if [ -z "$WIN_REMOTE_HOST" ] || ! printf '%s\n' "$WIN_REMOTE_HOST" | grep -Eq '^[A-Za-z0-9_.@-]+$'; then
  echo 'ERROR: ffmpeg-windows-build: WIN_REMOTE_HOST must be a safe user@host value' >&2
  exit 1
fi
if ! require_lower_hex "$EXPECTED_WIN_COMMIT" 40; then
  echo 'ERROR: ffmpeg-windows-build: EXPECTED_WIN_COMMIT must be a full lowercase commit' >&2
  exit 1
fi
if [ -z "$SOLSTONE_JOURNAL_WIN_REFS_ROOT" ] || [ -z "$SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT" ]; then
  echo 'ERROR: ffmpeg-windows-build: the mandatory native host gate requires refs root and owner account' >&2
  exit 1
fi
if ! printf '%s\n' "$WIN_REMOTE_HOME" | grep -Eq '^[A-Za-z]:[\\/][A-Za-z0-9_. ()\\/:=-]*$'; then
  echo 'ERROR: ffmpeg-windows-build: WIN_REMOTE_HOME must be a safe absolute Windows path' >&2
  exit 1
fi

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root=$(CDPATH= cd -- "$script_dir/.." && pwd)
cd "$repo_root"

if [ -n "$("$GIT" status --porcelain=v1 --untracked-files=all --ignore-submodules=none)" ]; then
  echo 'ERROR: ffmpeg-windows-build: source-bound build requires a clean Journal checkout' >&2
  exit 1
fi
if [ "$("$GIT" rev-parse HEAD)" != "$EXPECTED_WIN_COMMIT" ]; then
  echo 'ERROR: ffmpeg-windows-build: HEAD does not equal EXPECTED_WIN_COMMIT' >&2
  exit 1
fi

input_root="$repo_root/target/windows-ffmpeg-builder-inputs"
source_archive="$repo_root/target/ffmpeg-source-cache/ffmpeg.tar.gz"
local_slot="$repo_root/target/ffmpeg-windows-controlled-build/$EXPECTED_WIN_COMMIT"
if [ -e "$local_slot" ]; then
  echo "ERROR: ffmpeg-windows-build: refusing to reuse local output slot for $EXPECTED_WIN_COMMIT" >&2
  exit 1
fi

cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked -- acquire ffmpeg
cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked -- acquire ffmpeg-windows-tools --dest "$input_root"

msys2_archive="$input_root/msys2-base-x86_64-20260611.tar.xz"
make_archive="$input_root/make-4.4.1-3-x86_64.pkg.tar.zst"
nasm_archive="$input_root/nasm-3.02-win64.zip"
llvm_archive="$input_root/clang+llvm-22.1.6-x86_64-pc-windows-msvc.tar.xz"
for input in "$source_archive" "$msys2_archive" "$make_archive" "$nasm_archive" "$llvm_archive"; do
  [ -f "$input" ] || { echo "ERROR: ffmpeg-windows-build: verified input absent: $input" >&2; exit 1; }
done
cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked -- \
  ffmpeg-windows verify-inputs \
  --source-archive "$source_archive" \
  --msys2-archive "$msys2_archive" \
  --make-archive "$make_archive" \
  --nasm-archive "$nasm_archive" \
  --llvm-archive "$llvm_archive"

SOLSTONE_JOURNAL_WIN_REFS_ROOT="$SOLSTONE_JOURNAL_WIN_REFS_ROOT" \
SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT="$SOLSTONE_JOURNAL_WIN_OWNER_ACCOUNT" \
make win-host-ci

remote_rel="ffmpeg-windows-controlled-build/$EXPECTED_WIN_COMMIT"
remote_repo="$WIN_REMOTE_HOME\\sjbuild"
remote_slot="$WIN_REMOTE_HOME\\ffmpeg-windows-controlled-build\\$EXPECTED_WIN_COMMIT"
remote_source="$WIN_REMOTE_HOME\\ffmpeg-source-$EXPECTED_WIN_COMMIT.tar.gz"
remote_msys2="$WIN_REMOTE_HOME\\ffmpeg-msys2-$EXPECTED_WIN_COMMIT.tar.xz"
remote_make="$WIN_REMOTE_HOME\\ffmpeg-make-$EXPECTED_WIN_COMMIT.pkg.tar.zst"
remote_nasm="$WIN_REMOTE_HOME\\ffmpeg-nasm-$EXPECTED_WIN_COMMIT.zip"
remote_llvm="$WIN_REMOTE_HOME\\ffmpeg-llvm-$EXPECTED_WIN_COMMIT.tar.xz"
remote_guard="foreach (\$path in @('$remote_source', '$remote_msys2', '$remote_make', '$remote_nasm', '$remote_llvm', '$remote_slot')) { if (Test-Path -LiteralPath \$path) { throw \"remote FFmpeg slot path already exists: \$path\" } }"
"$SSH" "$WIN_REMOTE_HOST" "powershell -NoProfile -Command \"$remote_guard\""

remote_cleanup="\$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath '$remote_source') { Remove-Item -LiteralPath '$remote_source' -Recurse -Force }
if (Test-Path -LiteralPath '$remote_msys2') { Remove-Item -LiteralPath '$remote_msys2' -Recurse -Force }
if (Test-Path -LiteralPath '$remote_make') { Remove-Item -LiteralPath '$remote_make' -Recurse -Force }
if (Test-Path -LiteralPath '$remote_nasm') { Remove-Item -LiteralPath '$remote_nasm' -Recurse -Force }
if (Test-Path -LiteralPath '$remote_llvm') { Remove-Item -LiteralPath '$remote_llvm' -Recurse -Force }
if (Test-Path -LiteralPath '$remote_slot') { Remove-Item -LiteralPath '$remote_slot' -Recurse -Force }
if (Test-Path -LiteralPath '$remote_source') { throw 'remote FFmpeg cleanup left source archive' }
if (Test-Path -LiteralPath '$remote_msys2') { throw 'remote FFmpeg cleanup left MSYS2 archive' }
if (Test-Path -LiteralPath '$remote_make') { throw 'remote FFmpeg cleanup left make archive' }
if (Test-Path -LiteralPath '$remote_nasm') { throw 'remote FFmpeg cleanup left NASM archive' }
if (Test-Path -LiteralPath '$remote_llvm') { throw 'remote FFmpeg cleanup left LLVM archive' }
if (Test-Path -LiteralPath '$remote_slot') { throw 'remote FFmpeg cleanup left build slot' }
Write-Output 'FFMPEG_REMOTE_CLEANUP_OK paths=6'"
remote_cleanup_encoded=$(printf '%s' "$remote_cleanup" | iconv -f UTF-8 -t UTF-16LE | base64 | tr -d '\r\n')
if [ -z "$remote_cleanup_encoded" ]; then
  echo 'ERROR: ffmpeg-windows-build: could not encode remote cleanup command' >&2
  exit 1
fi
remote_cleanup_armed=0
cleanup_remote_on_failure() {
  [ "$remote_cleanup_armed" -eq 1 ] || return 0
  "$SSH" "$WIN_REMOTE_HOST" "powershell -NoProfile -EncodedCommand $remote_cleanup_encoded"
  remote_cleanup_armed=0
}
remote_cleanup_armed=1
trap cleanup_remote_on_failure EXIT

"$SCP" "$source_archive" "$WIN_REMOTE_HOST:ffmpeg-source-$EXPECTED_WIN_COMMIT.tar.gz"
"$SCP" "$msys2_archive" "$WIN_REMOTE_HOST:ffmpeg-msys2-$EXPECTED_WIN_COMMIT.tar.xz"
"$SCP" "$make_archive" "$WIN_REMOTE_HOST:ffmpeg-make-$EXPECTED_WIN_COMMIT.pkg.tar.zst"
"$SCP" "$nasm_archive" "$WIN_REMOTE_HOST:ffmpeg-nasm-$EXPECTED_WIN_COMMIT.zip"
"$SCP" "$llvm_archive" "$WIN_REMOTE_HOST:ffmpeg-llvm-$EXPECTED_WIN_COMMIT.tar.xz"

cargo_lock_sha256=$(sha256sum core/Cargo.lock | awk '{ print $1 }')
remote_command="& '$remote_repo\\core\\distribution\\ffmpeg-windows-build.ps1' \
  -RepositoryRoot '$remote_repo' \
  -SourceArchive '$remote_source' \
  -Msys2Archive '$remote_msys2' \
  -MakeArchive '$remote_make' \
  -NasmArchive '$remote_nasm' \
  -LlvmArchive '$remote_llvm' \
  -WorkRoot '$remote_slot\\work' \
  -OutputRoot '$remote_slot\\output' \
  -ReportRoot '$remote_slot\\report' \
  -ExpectedProductCommit '$EXPECTED_WIN_COMMIT' \
  -ExpectedCargoLockSha256 '$cargo_lock_sha256' \
  -BuilderHost '$WIN_REMOTE_HOST'"
"$SSH" "$WIN_REMOTE_HOST" "powershell -NoProfile -ExecutionPolicy Bypass -Command \"$remote_command\""

mkdir -p "$local_slot/output/bin" "$local_slot/report"
"$SCP" \
  "$WIN_REMOTE_HOST:$remote_rel/output/bin/solstone-core.exe" \
  "$WIN_REMOTE_HOST:$remote_rel/output/bin/solstone-core-describe.exe" \
  "$local_slot/output/bin/"
"$SCP" \
  "$WIN_REMOTE_HOST:$remote_rel/report/ffmpeg-build-evidence.json" \
  "$WIN_REMOTE_HOST:$remote_rel/report/ffmpeg-build-receipt.json" \
  "$WIN_REMOTE_HOST:$remote_rel/report/ffmpeg-build-validation.log" \
  "$local_slot/report/"
cargo run --manifest-path core/Cargo.toml -p solstone-core-distribution --bin solstone-distribution --locked -- \
  ffmpeg-windows verify --receipt "$local_slot/report/ffmpeg-build-receipt.json" --output-root "$local_slot/output"

"$SSH" "$WIN_REMOTE_HOST" "powershell -NoProfile -EncodedCommand $remote_cleanup_encoded"
remote_cleanup_armed=0
trap - EXIT

echo "FFMPEG_WINDOWS_DRIVER_OK commit=$EXPECTED_WIN_COMMIT output=$local_slot/output/bin receipt=$local_slot/report/ffmpeg-build-receipt.json"
