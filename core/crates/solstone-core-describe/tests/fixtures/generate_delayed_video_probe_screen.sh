#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

set -euo pipefail

readonly SOURCE_REPOSITORY='solpbc/field_journal'
readonly SOURCE_COMMIT='c1edcc8909f907075916e9ad0f63701da7b607b5'
readonly SOURCE_RELATIVE_PATH='journal/20260201/field.screen/094500_300/screen.mp4'
readonly SOURCE_SIZE=4808653
readonly SOURCE_SHA256='09fa691b99e4d0450922ae39d5be9c16231dd2fa00328d6057c5e0e92e4df6d1'
readonly REFERENCE_FFMPEG_VERSION='n8.0.1-48-g0592be14ff-20260116'
readonly OUTPUT_SIZE=73642
readonly OUTPUT_SHA256='091fa2d732148a0c1e611a72bd320d9db200a790a0fcdf17cfc83d7280d2c17d'

if [ "$#" -gt 1 ]; then
    echo "usage: $0 [field_journal_checkout]" >&2
    exit 2
fi

source_root=${1:-${SOLSTONE_FIELD_JOURNAL_ROOT:-/home/jer/projects/field_journal}}
source_file="$source_root/$SOURCE_RELATIVE_PATH"
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
output_file="$script_dir/delayed_video_probe_screen.mp4"
temporary_dir=$(mktemp -d "${TMPDIR:-/var/tmp}/solstone-delayed-video-probe.XXXXXX")
temporary_output="$temporary_dir/delayed_video_probe_screen.mp4"

cleanup() {
    rm -rf -- "$temporary_dir"
}
trap cleanup EXIT

if [ ! -f "$source_file" ]; then
    echo "source recording not found: $source_file" >&2
    exit 1
fi
if [ "$(wc -c < "$source_file" | tr -d '[:space:]')" != "$SOURCE_SIZE" ]; then
    echo "source size mismatch for $SOURCE_REPOSITORY@$SOURCE_COMMIT: $source_file" >&2
    exit 1
fi
if [ "$(sha256sum "$source_file" | awk '{print $1}')" != "$SOURCE_SHA256" ]; then
    echo "source digest mismatch for $SOURCE_REPOSITORY@$SOURCE_COMMIT: $source_file" >&2
    exit 1
fi

running_ffmpeg_version=$(ffmpeg -version | sed -n '1p')
if [[ "$running_ffmpeg_version" != *"$REFERENCE_FFMPEG_VERSION"* ]]; then
    echo "fixture reference FFmpeg is $REFERENCE_FFMPEG_VERSION; running $running_ffmpeg_version" >&2
    exit 1
fi

ffmpeg -hide_banner -loglevel error -y -i "$source_file" -map 0 -c copy -t 9.4 -map_metadata -1 -fflags +bitexact "$temporary_output"

if [ "$(wc -c < "$temporary_output" | tr -d '[:space:]')" != "$OUTPUT_SIZE" ]; then
    echo "refusing to publish fixture: output size mismatch" >&2
    exit 1
fi
if [ "$(sha256sum "$temporary_output" | awk '{print $1}')" != "$OUTPUT_SHA256" ]; then
    echo "refusing to publish fixture: output digest mismatch" >&2
    exit 1
fi

mv -- "$temporary_output" "$output_file"
echo "wrote $output_file"
