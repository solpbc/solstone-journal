#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Assert the Rust media-format table still agrees with the Python one.

Two implementations classify a media file by its extension, and retention's release
predicate rests on the split between them: audio and video route to a handler whose
processing record can prove the raw was consumed, and an image routes to no handler at
all, which is what holds an image's original forever.

Drift here does not lose data -- an extension neither side claims is simply never
released -- but it silently stops the feature working for a whole format, and no test
in either language would notice. This is the cheap cross-language check that does.

⚠ Executes the Python side by import and parses the Rust side as text, because the
Rust table is a `const` with no runtime entry point. The parse is asserted non-empty,
so a table that moves cannot make this pass vacuously.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
RUST = ROOT / "core/crates/solstone-core-processing-record/src/media.rs"

sys.path.insert(0, str(ROOT))

from solstone.think.media import FORMATS  # noqa: E402

ROW = re.compile(r'\(\s*"([a-z0-9]+)"\s*,\s*MediaKind::(Audio|Video|Image)\s*\)')


def rust_table() -> dict[str, str]:
    text = RUST.read_text(encoding="utf-8")
    start = text.index("const FORMATS")
    end = text.index("];", start)
    rows = ROW.findall(text[start:end])
    if not rows:
        raise SystemExit(
            f"parsed no rows from {RUST}; the table moved or changed shape, so this "
            "check would have passed without comparing anything"
        )
    return {extension: kind.lower() for extension, kind in rows}


def python_table() -> dict[str, str]:
    return {extension.lstrip("."): kind for extension, _mime, kind in FORMATS}


def main() -> int:
    rust = rust_table()
    python = python_table()
    if rust == python:
        print(f"media format parity: {len(rust)} extensions agree")
        return 0

    print("media format tables disagree", file=sys.stderr)
    for extension in sorted(set(rust) | set(python)):
        left = python.get(extension)
        right = rust.get(extension)
        if left != right:
            print(
                f"  {extension:6} python={left or 'absent':7} rust={right or 'absent'}",
                file=sys.stderr,
            )
    print(
        "\nThe authority is solstone/think/media.py FORMATS. Update "
        f"{RUST.relative_to(ROOT)} to match, remembering that Rust keys carry no "
        "leading dot, and that adding an image extension does NOT make image raw "
        "releasable -- only a handler claiming it does.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
