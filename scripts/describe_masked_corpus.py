#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Screencasts carrying the Convey-UI fiducials, observed through the whole loop.

The frame corpus (`describe_frames.json`) has no fiducials in it and the fiducial
corpus (`describe_fiducials.json`) is still images with no hashes. Between them
they pin the detector and they pin the winnow, and they pin **nothing about the
composition** -- which is the part that is easy to get wrong and impossible to
see, because the mask runs *before* the perceptual hash and a corpus with no
tags in it hashes identically whether the mask is applied before, after, or
never.

These are the missing oracle: real VP8/WebM screencasts with the shipped corner
tags composited in, observed through the reference `VideoProcessor.process()`
with masking live, recording the qualified frames, the dHash bookends and the
winnow counters.

Three cases, chosen so each one fails for a *different* wrong implementation:

* ``masked_inside`` -- tags below the 0.8 coverage gate, and the only thing that
  changes between frames is **inside** the masked polygon. Correct behaviour
  blacks that region out, so the frames look identical and do not qualify. An
  implementation that hashes before masking, or never fills, qualifies them.
* ``masked_outside`` -- the same tags and coverage, with the change **outside**
  the polygon. Those frames must still qualify. This is the twin: without it,
  ``masked_inside`` is satisfied by a mask that blacks out the entire frame.
* ``skipped`` -- tags above the gate, so every frame is dropped before hashing.

⛔ FROZEN. No gate re-encodes this; a re-encode moves the hashes.

Recorded provenance: built 2026-08-06 on fedora with system ffmpeg 7.1.2
(libvpx VP8); observed through PyAV 16.1.0 (libavcodec 62.11.100) and OpenCV
4.13.0.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw

WIDTH, HEIGHT = 1280, 720
TAGS_DIR = Path("solstone/convey/static/tags")
CORNERS = {6: "tl", 7: "tr", 4: "bl", 2: "br"}
TAG_SIZE = 48
FRAMES = 8


def _load_tag(tag_id: int, size: int) -> Image.Image:
    """The shipped tag flattened onto white; its quiet zone is transparency."""
    tag = Image.open(TAGS_DIR / f"tag-{tag_id}.png").convert("RGBA")
    backdrop = Image.new("RGBA", tag.size, (255, 255, 255, 255))
    return Image.alpha_composite(backdrop, tag).convert("RGB").resize(
        (size, size), Image.NEAREST
    )


def _inset_for_coverage(fraction: float) -> int:
    target = fraction * WIDTH * HEIGHT
    a, b, c = 4.0, -2.0 * (WIDTH + HEIGHT), WIDTH * HEIGHT - target
    disc = max(0.0, b * b - 4.0 * a * c)
    return max(0, int(round((-b - disc**0.5) / (2.0 * a))))


def _frame(index: int, inset: int, *, change: str) -> Image.Image:
    """One screen: a static backdrop, the four tags, and one moving element."""
    image = Image.new("RGB", (WIDTH, HEIGHT), (238, 238, 240))
    draw = ImageDraw.Draw(image)
    for row in range(0, HEIGHT, 48):
        draw.rectangle([0, row, WIDTH, row + 24], fill=(206, 210, 218))

    # The moving element. "inside" places it deep within the tag-bounded
    # polygon, so masking erases it; "outside" paints the margin band the
    # polygon never covers.
    #
    # ⚠ The outside change has to be LARGE. The hash is computed on a 9x8
    # downsample of the whole frame, so a change confined to a few dozen pixels
    # of a 1280x720 screen lands inside one cell and moves no bit -- measured:
    # a growing 60..340 px box in the bottom margin qualified exactly one frame,
    # the same as the masked case, which would have made the twin vacuous.
    if change == "inside":
        band = 90 + index * 40
        draw.rectangle(
            [inset + 120, inset + 60, inset + 520, inset + 60 + band],
            fill=(20, 30, 40),
        )
    else:
        # ⚠ The change must vary HORIZONTALLY. dHash compares each pixel with
        # its right-hand neighbour, so a full-width flat band -- however
        # brightly coloured -- produces all-zero bits in its rows and is
        # completely invisible to the hash. Measured: two runs whose margins
        # differed only in flat colour produced the IDENTICAL 64-bit hash.
        # Phase-shifted vertical stripes move the edges, which is what the
        # comparison can actually see.
        # One stripe is about one column of the 9-wide downsample, and the
        # phase advances a full stripe per frame, so consecutive frames
        # invert the pattern in every affected row rather than nudging it.
        stripe = 142
        phase = index * stripe
        for band_top, band_height in (
            (0, max(1, inset - 8)),
            (HEIGHT - max(1, inset - 8), max(1, inset - 8)),
        ):
            for start in range(-stripe, WIDTH + stripe, stripe * 2):
                left = start + phase
                draw.rectangle(
                    [left, band_top, left + stripe, band_top + band_height],
                    fill=(12, 12, 12),
                )

    for tag_id, corner in CORNERS.items():
        tag = _load_tag(tag_id, TAG_SIZE)
        positions = {
            "tl": (inset, inset),
            "tr": (WIDTH - TAG_SIZE - inset, inset),
            "bl": (inset, HEIGHT - TAG_SIZE - inset),
            "br": (WIDTH - TAG_SIZE - inset, HEIGHT - TAG_SIZE - inset),
        }
        image.paste(tag, positions[corner])
    return image


CASES = {
    # Below the 0.8 gate, change inside the polygon.
    "masked_inside": (0.45, "inside"),
    # Below the gate, change outside the polygon -- the twin.
    "masked_outside": (0.45, "outside"),
    # Above the gate: every frame is dropped before hashing.
    "skipped": (0.92, "outside"),
}


def build(root: Path) -> list[Path]:
    if shutil.which("ffmpeg") is None:
        raise SystemExit("ffmpeg is required to build the masked corpus")
    segment = root / "20260101" / "120000_300"
    segment.mkdir(parents=True, exist_ok=True)
    staging = root / ".frames"
    produced: list[Path] = []
    for name, (fraction, change) in CASES.items():
        inset = _inset_for_coverage(fraction)
        frames_dir = staging / name
        frames_dir.mkdir(parents=True, exist_ok=True)
        for index in range(FRAMES):
            _frame(index, inset, change=change).save(
                frames_dir / f"frame_{index:04d}.png"
            )
        output = segment / f"convey_{name}_screen.webm"
        subprocess.run(
            [
                "ffmpeg", "-nostdin", "-y", "-loglevel", "error",
                "-framerate", "1",
                "-i", str(frames_dir / "frame_%04d.png"),
                "-c:v", "libvpx", "-b:v", "2M", "-crf", "4",
                "-deadline", "good", "-cpu-used", "1",
                str(output),
            ],
            check=True,
        )
        produced.append(output)
    shutil.rmtree(staging, ignore_errors=True)
    return produced


def observe(paths: list[Path]) -> dict:
    from solstone.observe.describe import VideoProcessor

    cases = []
    for path in sorted(paths):
        processor = VideoProcessor(path)
        qualified = processor.process()
        cases.append(
            {
                "file": path.name,
                "decode_failed": processor.decode_failed,
                "qualified_count": processor.qualified_count,
                "first_hash": VideoProcessor._format_dhash(processor.first_hash),
                "last_hash": VideoProcessor._format_dhash(processor.last_hash),
                "winnow": processor.winnow_metrics,
                "frames": [
                    {
                        "frame_id": frame["frame_id"],
                        "timestamp": round(frame["timestamp"], 6),
                        "aruco": frame.get("aruco"),
                    }
                    for frame in qualified
                ],
            }
        )
    return {
        "fixture": "solstone-describe-masked-frames",
        "fixture_version": 1,
        "frame_size": {"width": WIDTH, "height": HEIGHT},
        "cases": cases,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--observe", action="store_true")
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
