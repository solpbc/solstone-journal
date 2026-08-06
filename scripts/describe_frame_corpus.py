#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Deterministic screencast corpus for the describe frame pipeline.

Generates the videos the native frame pipeline is pinned against, in the two
container/codec pairs the recorders actually produce:

* ``vp8`` in WebM at 1 fps -- what ``solstone-linux`` writes
  (``pipewiresrc ... framerate=1/1 ... vp8enc ! webmmux``).
* ``h264`` in QuickTime -- what ``solstone-macos`` writes
  (``AVVideoCodecKey: AVVideoCodecType.h264``).

Frame content is synthesised from a fixed seed so the same bytes come out on
every host: no clock, no randomness beyond the seed, no host fonts.

The encoded corpus is committed under ``core/fixtures/describe_corpus/`` (88 KB
total) and the companion oracle is ``core/fixtures/describe_frames.json``, which
records what the reference implementation decided for each case -- which frames
qualified, at what timestamps, and the run's dHash bookends.

Both are a FROZEN record. This script is provenance and a way to extend the
corpus; ⛔ **no gate runs it**, deliberately. Video encoding is not
bit-reproducible across ffmpeg builds, so re-encoding on a check would move the
hashes and redden a green tree for a reason that has nothing to do with the
decoder under test. A consumer reads the committed bytes.

Recorded provenance of the committed corpus: built 2026-08-06 on fedora with
system ffmpeg 7.1.2 (libvpx VP8, libopenh264); the oracle was produced by the
reference handler through PyAV 16.1.0 (libavcodec 62.11.100, i.e. FFmpeg 8.0),
which is the version skew a real host already has.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw

# Every case renders at this size; large enough that the 9x8 dHash resize is a
# real downsample and small enough to encode quickly.
WIDTH, HEIGHT = 640, 360

# A palette that produces well-separated luminance gradients, so a case can
# move the dHash by a controlled amount rather than by accident.
PALETTE = [
    (16, 16, 16),
    (232, 232, 232),
    (24, 96, 192),
    (200, 64, 32),
    (32, 168, 96),
    (176, 176, 32),
]


def _panel(index: int, jitter: int) -> Image.Image:
    """One synthetic screen.

    ``index`` selects the coarse layout (large dHash moves); ``jitter`` nudges a
    small region only (sub-threshold moves).
    """
    image = Image.new("RGB", (WIDTH, HEIGHT), PALETTE[index % len(PALETTE)])
    draw = ImageDraw.Draw(image)
    columns = 3 + (index % 4)
    for column in range(columns):
        left = int(column * WIDTH / columns)
        right = int((column + 1) * WIDTH / columns)
        shade = PALETTE[(index + column) % len(PALETTE)]
        draw.rectangle([left, 0, right, HEIGHT], fill=shade)
    # Horizontal band whose vertical position encodes the layout, so the dHash
    # row comparisons move when the layout does.
    band_top = int((index % 5) * HEIGHT / 6)
    draw.rectangle(
        [0, band_top, WIDTH, band_top + int(HEIGHT / 6)],
        fill=PALETTE[(index + 3) % len(PALETTE)],
    )
    # Small jitter square: confined to one dHash cell, so it moves the hash by
    # at most a bit or two.
    if jitter:
        size = 12
        x = 8 + (jitter % 3) * 4
        draw.rectangle([x, 8, x + size, 8 + size], fill=(255, 0, 255))
    return image


# Each case is a list of (layout_index, jitter) -- one entry per emitted frame,
# in order. At 1 fps the frame index is also the timestamp in seconds, which is
# what makes the 5.0s stride floor observable.
CASES: dict[str, list[tuple[int, int]]] = {
    # Twelve frames of the same layout with only sub-threshold jitter: after the
    # always-kept first frame, every later frame should fall to below_threshold.
    "static": [(0, i % 3) for i in range(12)],
    # Alternating far-apart layouts once per second: every frame is a large
    # move, so scene cuts should bypass the stride floor.
    "scene_cuts": [(i % 6, 0) for i in range(12)],
    # A moderate move every second. The first is kept; the next four arrive
    # inside the 5s floor and should be stride-dropped; the sixth clears it.
    "stride_floor": [(0, 0)] + [(1, i % 3) for i in range(1, 12)],
    # Long static runs punctuated by a change, so both the stride floor and the
    # below-threshold gate are exercised in one file.
    "mixed": (
        [(0, i % 3) for i in range(6)]
        + [(2, i % 3) for i in range(6)]
        + [(4, i % 3) for i in range(6)]
    ),
    # One frame only: the always-kept first frame and nothing to compare it to.
    "single_frame": [(3, 0)],
}


def _render_case(name: str, frames_dir: Path) -> int:
    frames_dir.mkdir(parents=True, exist_ok=True)
    plan = CASES[name]
    for position, (index, jitter) in enumerate(plan):
        _panel(index, jitter).save(frames_dir / f"frame_{position:04d}.png")
    return len(plan)


def _encode(frames_dir: Path, output: Path, codec: str) -> None:
    if codec == "vp8":
        # Mirrors the linux recorder: VP8 in WebM, one frame per second.
        codec_args = [
            "-c:v",
            "libvpx",
            "-b:v",
            "1M",
            "-crf",
            "10",
            "-deadline",
            "good",
            "-cpu-used",
            "2",
        ]
    elif codec == "h264":
        # Mirrors the macOS recorder's codec choice in a QuickTime container.
        # libopenh264 rather than libx264: the reference build host's ffmpeg
        # ships the former and not the latter, and the decoder under test does
        # not care which encoder produced a conformant H.264 stream.
        codec_args = [
            "-c:v",
            "libopenh264",
            "-b:v",
            "1M",
            "-pix_fmt",
            "yuv420p",
        ]
    else:
        raise ValueError(f"unknown codec {codec}")
    output.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            "ffmpeg",
            "-nostdin",
            "-y",
            "-loglevel",
            "error",
            "-framerate",
            "1",
            "-i",
            str(frames_dir / "frame_%04d.png"),
            *codec_args,
            str(output),
        ],
        check=True,
    )


def _write_degenerate(directory: Path) -> None:
    """The two decode-failure shapes the handler must classify, not crash on."""
    # A container with no video stream at all.
    subprocess.run(
        [
            "ffmpeg",
            "-nostdin",
            "-y",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=48000:cl=mono",
            "-t",
            "2",
            "-c:a",
            "aac",
            str(directory / "audio_only_screen.mov"),
        ],
        check=True,
    )
    # Bytes that are not a container at all.
    (directory / "not_a_video_screen.webm").write_bytes(b"not a container\n" * 64)


def build(root: Path) -> list[Path]:
    if shutil.which("ffmpeg") is None:
        raise SystemExit("ffmpeg is required to build the describe frame corpus")
    segment = root / "20260101" / "120000_300"
    segment.mkdir(parents=True, exist_ok=True)
    staging = root / ".frames"
    produced: list[Path] = []
    for name in CASES:
        frames_dir = staging / name
        _render_case(name, frames_dir)
        for codec, extension in (("vp8", "webm"), ("h264", "mov")):
            output = segment / f"{name}_{codec}_screen.{extension}"
            _encode(frames_dir, output, codec)
            produced.append(output)
    _write_degenerate(segment)
    produced.append(segment / "audio_only_screen.mov")
    produced.append(segment / "not_a_video_screen.webm")
    shutil.rmtree(staging, ignore_errors=True)
    return produced


def observe(paths: list[Path]) -> dict:
    """Run the reference frame pipeline over the corpus and record its verdicts."""
    from solstone.observe.describe import VideoProcessor

    cases = []
    for path in sorted(paths):
        processor = VideoProcessor(path)
        qualified = processor.process()
        cases.append(
            {
                "file": path.name,
                "width": processor.width,
                "height": processor.height,
                "decode_failed": processor.decode_failed,
                "qualified_count": processor.qualified_count,
                "first_hash": VideoProcessor._format_dhash(processor.first_hash),
                "last_hash": VideoProcessor._format_dhash(processor.last_hash),
                "winnow": processor.winnow_metrics,
                "frames": [
                    {"frame_id": f["frame_id"], "timestamp": round(f["timestamp"], 6)}
                    for f in qualified
                ],
            }
        )
    return {"fixture": "solstone-describe-frames", "fixture_version": 1, "cases": cases}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path, help="directory to build the corpus in")
    parser.add_argument(
        "--observe",
        action="store_true",
        help="also run the reference pipeline and print the oracle to stdout",
    )
    args = parser.parse_args()
    produced = build(args.root)
    if args.observe:
        json.dump(observe(produced), sys.stdout, indent=2, sort_keys=True)
        sys.stdout.write("\n")
    else:
        for path in produced:
            print(path)


if __name__ == "__main__":
    main()
