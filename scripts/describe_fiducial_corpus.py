#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Frozen native describe fiducial fixture; Python-reference oracle regeneration retired.

The convey web interface renders four corner tags so a screencast of the
journal's own interface can be recognised and blacked out before any frame is
described. This corpus is the detector's pinned surface: PNG screens carrying
the **real** tags from ``core/crates/solstone-core-convey-shell/assets/static/tags/``
at a range of sizes,
placements and rotations, plus the cases that must NOT produce a polygon.

The companion fixture is ``core/fixtures/describe_fiducials.json``.

Both are a FROZEN record. ⛔ No gate runs this script: PNG encoding is
deterministic, but the *detector* is OpenCV's and its sub-pixel corner
refinement moves between OpenCV releases, so re-observing on a check would
redden a green tree for a reason unrelated to the port under test. A consumer
reads the committed bytes.

Recorded provenance: built 2026-08-06 on fedora.
"""

from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image, ImageDraw

WIDTH, HEIGHT = 1280, 720
TAGS_DIR = Path("core/crates/solstone-core-convey-shell/assets/static/tags")
# 6=TL 7=TR 4=BL 2=BR -- the placement convention the interface renders.
CORNERS = {6: "tl", 7: "tr", 4: "bl", 2: "br"}


def _background() -> Image.Image:
    """A busy, non-uniform screen: a flat field would flatter the detector."""
    image = Image.new("RGB", (WIDTH, HEIGHT), (245, 245, 245))
    draw = ImageDraw.Draw(image)
    for row in range(0, HEIGHT, 40):
        shade = 200 + (row // 40) % 40
        draw.rectangle([0, row, WIDTH, row + 20], fill=(shade, shade - 12, shade - 24))
    for column in range(0, WIDTH, 160):
        draw.rectangle([column, 0, column + 6, HEIGHT], fill=(120, 130, 150))
    draw.rectangle([220, 140, 1060, 580], fill=(255, 255, 255), outline=(90, 90, 90))
    return image


def _place(image: Image.Image, tag: Image.Image, corner: str, inset: int) -> None:
    width, height = tag.size
    positions = {
        "tl": (inset, inset),
        "tr": (WIDTH - width - inset, inset),
        "bl": (inset, HEIGHT - height - inset),
        "br": (WIDTH - width - inset, HEIGHT - height - inset),
    }
    image.paste(tag, positions[corner])


def _inset_for_coverage(fraction: float, size: int) -> int:
    """Inset that makes the four outer tag corners span ``fraction`` of the frame.

    The polygon the detector derives runs corner-to-outer-corner, so the spanned
    area is ``(W - 2*inset) * (H - 2*inset)`` once the tags sit flush to it.
    """
    target = fraction * WIDTH * HEIGHT
    # 4i^2 - 2(W+H)i + (WH - target) = 0, smaller root.
    a = 4.0
    b = -2.0 * (WIDTH + HEIGHT)
    c = WIDTH * HEIGHT - target
    disc = max(0.0, b * b - 4.0 * a * c)
    inset = (-b - disc**0.5) / (2.0 * a)
    return max(0, int(round(inset)))


def _load_tag(tag_id: int, size: int) -> Image.Image:
    """The shipped tag, flattened onto white.

    ⚠ The tag PNGs are RGBA and their quiet zone is **transparent**, not white —
    in the interface the page background shows through it. A bare
    ``.convert("RGB")`` turns that transparency black, destroys the quiet zone,
    and the detector then finds nothing at all. Flatten onto white, which is
    what a light interface background gives the detector in practice.
    """
    tag = Image.open(TAGS_DIR / f"tag-{tag_id}.png").convert("RGBA")
    backdrop = Image.new("RGBA", tag.size, (255, 255, 255, 255))
    flat = Image.alpha_composite(backdrop, tag).convert("RGB")
    return flat.resize((size, size), Image.NEAREST)


def _screen(tag_size: int, present: set[int], inset: int, rotate: int = 0) -> Image.Image:
    image = _background()
    for tag_id, corner in CORNERS.items():
        if tag_id not in present:
            continue
        tag = _load_tag(tag_id, tag_size)
        if rotate:
            tag = tag.rotate(rotate, expand=True, fillcolor=(255, 255, 255))
        _place(image, tag, corner, inset)
    return image


ALL = set(CORNERS)

CASES: dict[str, Image.Image] = {}


def _build_cases() -> None:
    # The size the interface actually renders (16 CSS px) and two larger ones,
    # so a perimeter-rate regression shows up as a size-dependent miss.
    CASES["four_tags_16px"] = _screen(16, ALL, inset=8)
    CASES["four_tags_32px"] = _screen(32, ALL, inset=8)
    CASES["four_tags_64px"] = _screen(64, ALL, inset=8)
    # One tag missing, once per corner: exercises every branch of the
    # parallelogram extrapolation, including which corner it reports.
    for missing in sorted(ALL):
        CASES[f"three_tags_missing_{missing}"] = _screen(
            32, ALL - {missing}, inset=8
        )
    # Two tags: below the extrapolation floor, so no polygon may be derived.
    CASES["two_tags"] = _screen(32, {6, 7}, inset=8)
    # No tags at all: the detector must report nothing, not an empty polygon.
    CASES["no_tags"] = _background()
    # Coverage cases either side of the 0.8 skip threshold, placed close to it
    # so the gate has a tight measured witness in both directions rather than
    # one arbitrary example far from the boundary.
    CASES["coverage_far_under"] = _screen(
        32, ALL, inset=_inset_for_coverage(0.40, 32)
    )
    CASES["coverage_just_under"] = _screen(
        32, ALL, inset=_inset_for_coverage(0.78, 32)
    )
    CASES["coverage_just_over"] = _screen(
        32, ALL, inset=_inset_for_coverage(0.82, 32)
    )
    # Rotated tags: the interface can be scaled or transformed, and corner
    # ordering within a marker is what the polygon derivation depends on.
    CASES["four_tags_rotated"] = _screen(48, ALL, inset=12, rotate=12)


def build(root: Path) -> list[Path]:
    _build_cases()
    root.mkdir(parents=True, exist_ok=True)
    produced = []
    for name, image in CASES.items():
        path = root / f"{name}.png"
        image.save(path, format="PNG", optimize=True)
        produced.append(path)
    return produced


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path, help="directory to write the screens into")
    args = parser.parse_args()
    produced = build(args.root)
    for path in produced:
        print(path)


if __name__ == "__main__":
    main()
