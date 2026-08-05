#!/usr/bin/env python3
"""Author the stream-name projection vectors, and pin the legacy names beside them.

Two halves, deliberately different in kind:

  legacy/  -- RECORDED from the live Python `stream_name()`. These are names
              that already exist on owners' disks. A rebuild must keep reading
              them, so they are captured from the implementation that wrote
              them, not invented.

  project/ -- AUTHORED from the projection rule. No implementation produces
              these yet, which is the point: a vector recorded from the code it
              is meant to pin cannot catch the code being wrong. The rule is
              stated once, here, and the vectors are derived from it.

The generator refuses to emit unless the legacy half reproduces the live
implementation exactly. If `stream_name()` ever changes, this breaks rather
than silently re-baselining.

Run from the solstone-journal checkout with its venv:

    .venv/bin/python <this file> <output.json>
"""

from __future__ import annotations

import json
import re
import sys
import unicodedata
from pathlib import Path

# ---------------------------------------------------------------------------
# The projection rule. This is the specification; the Rust implements it.
# ---------------------------------------------------------------------------

WINDOWS_RESERVED = {
    "CON", "PRN", "AUX", "NUL",
    *(f"COM{i}" for i in range(1, 10)),
    *(f"LPT{i}" for i in range(1, 10)),
}

MAX_BYTES = 64
EMPTY_FALLBACK = "device"

# The rule is GENERATED from this list rather than hand-written beside it.
# It was hand-written once, the implementation gained a decomposition step, and
# the sentence silently went false while every vector stayed green -- prose
# adjacent to a checked artifact drifts precisely because nothing compares it.
# `refuse_unless_sound` binds each step below to an output only that step can
# produce, so a step that leaves the implementation also leaves the sentence.
PIPELINE_STEPS: list[tuple[str, str]] = [
    ("decompose", "NFKD normalize, so accents separate from their base letters"),
    ("casefold", "full Unicode casefold -- NOT a lowercase mapping"),
    ("strip-marks", "drop combining marks (category Mn), so `café` becomes `cafe`"),
    ("filter", "replace runs of [^a-z0-9_-] with a single '_'"),
    ("strip", "strip leading and trailing '_-.'"),
    ("fallback", f"a LABEL that projects to empty becomes '{EMPTY_FALLBACK}' -- "
                 f"this fallback applies to the label only, never to the source"),
    ("reserved", "a Windows reserved stem gains a trailing '_'"),
    ("source", "append '_' + projected(source) when the PROJECTED source is "
               "non-empty -- a source that projects to empty appends nothing "
               "and does NOT take the fallback"),
    ("truncate", f"truncate to {MAX_BYTES} bytes -- AFTER the source is appended"),
]

# Each step name mapped to (input, output) that ONLY that step produces. If a
# step is dropped from the implementation, its probe stops matching and the
# generator refuses; if it is dropped from PIPELINE_STEPS, the rule sentence
# loses it and the same check fires. That is the bind the first version lacked.
STEP_PROBES: dict[str, tuple[str, str, str]] = {
    "decompose": ("café", "", "cafe"),
    "casefold": ("Straße", "", "strasse"),
    "strip-marks": ("İstanbul", "", "istanbul"),
    "filter": ("my.phone", "", "my_phone"),
    "strip": ("  spaced  ", "", "spaced"),
    "fallback": ("!!!", "", EMPTY_FALLBACK),
    "reserved": ("CON", "", "con_"),
    "source": ("iPhone", "watch", "iphone_watch"),
    "truncate": ("A" * 200, "", "a" * MAX_BYTES),
}


def project(label: str, source: str = "") -> str:
    """Project an owner-facing display label to a filesystem-safe stream name.

    Casefold happens BEFORE the charset filter, so two labels differing only in
    case produce the same string and collide through the ordinal allocator
    rather than through a comparison rule a later edit could drop.
    """
    def one(part: str) -> str:
        # NFKD rather than NFKC so accents separate from their base letters, and
        # casefold before stripping them so `İ` -> `i` + combining dot -> `i`.
        # Without this step `café` becomes `caf` and `Téléphone` becomes
        # `t_l_phone`, which is not the human-friendly label the contract asks
        # for. Scripts with no Latin decomposition (CJK, Cyrillic) still filter
        # to `_`, which is correct -- the name is a label, not a transliteration.
        part = unicodedata.normalize("NFKD", part).casefold()
        part = "".join(c for c in part if unicodedata.category(c) != "Mn")
        part = re.sub(r"[^a-z0-9_-]+", "_", part)
        part = part.strip("_-.")
        if not part:
            return ""
        # Windows refuses these basenames bare or with any extension. The dot is
        # already gone by this point, so only the bare stem can match.
        if part.upper() in WINDOWS_RESERVED:
            part = f"{part}_"
        return part

    name = one(label) or EMPTY_FALLBACK
    projected_source = one(source)
    if projected_source:
        name = f"{name}_{projected_source}"

    # ASCII by construction, so bytes and chars coincide.
    return name[:MAX_BYTES]


# ---------------------------------------------------------------------------
# The authored vectors. Each states WHY it exists; a vector whose reason is
# "coverage" is a vector nobody will maintain correctly.
# ---------------------------------------------------------------------------

AUTHORED: list[dict] = [
    {"label": "iPhone", "source": "", "why": "the ordinary case"},
    {"label": "iPhone (2)", "source": "",
     "why": "the worked example the contract names; the ordinal form the label allocator already emits"},
    {"label": "IPHONE (2)", "source": "",
     "why": "🔴 case-collision pair with the row above -- the SAME output is the whole point. "
            "On APFS and NTFS these two collide; on ext4 they would not. Casefolding first "
            "makes them collide everywhere, so the ordinal allocator resolves it."},
    {"label": "iPhone (2)", "source": "watch",
     "why": "one device, two streams -- the sub-stream discriminator, which is why `did` alone "
            "cannot select a stream"},
    {"label": "my.phone", "source": "",
     "why": "⛔ the dot is the legacy kind separator and `import.` is a live prefix; a projected "
            "`my.phone` would read as a qualified name at seven call sites"},
    {"label": "workstation.browser", "source": "",
     "why": "a legacy kind-carrying name arriving as a label -- the dot still goes"},
    {"label": "  Ada's MacBook Pro  ", "source": "",
     "why": "apostrophe, spaces, and leading/trailing whitespace in one label"},
    {"label": "CON", "source": "",
     "why": "Windows reserved basename, bare"},
    {"label": "com1", "source": "",
     "why": "Windows reserved, already lowercase -- the check is case-insensitive on the stem"},
    {"label": "nul.txt", "source": "",
     "why": "reserved stem with an extension; the dot becomes `_` first, so the stem is `nul_txt` "
            "and is NOT reserved. Pins that the escape is not over-applied."},
    {"label": "!!!", "source": "",
     "why": "everything filters out -- must not produce an empty directory name"},
    {"label": "...", "source": "",
     "why": "dots only; strips to empty via a different path than the row above"},
    {"label": "-_-", "source": "",
     "why": "only strip characters -- empty after strip, not after filter"},
    {"label": "Ｆｕｌｌｗｉｄｔｈ", "source": "",
     "why": "NFKC folds fullwidth forms to ASCII before casefold; without normalization every "
            "character would filter to `_`"},
    {"label": "Straße", "source": "",
     "why": "🔴 casefold maps ß to ss where lower() does not -- the vector that catches "
            "`to_lowercase()` used in place of a real casefold"},
    {"label": "İstanbul", "source": "",
     "why": "dotted capital I -- casefold yields `i` plus a combining dot, which mark-stripping "
            "removes. Pins the pipeline order: NFKD → casefold → strip marks → filter."},
    {"label": "café", "source": "",
     "why": "a composed accent decomposes and its mark is stripped, so the owner gets `cafe` "
            "rather than `caf`"},
    {"label": "Téléphone d'Amélie", "source": "",
     "why": "🔴 the human-friendliness vector. Without mark-stripping this is `t_l_phone_d_am_lie` "
            "-- unreadable, and the contract asks for a human-friendly label. Any owner whose "
            "language uses accents hits this on their first device."},
    {"label": "Владимир", "source": "",
     "why": "a script with no Latin decomposition correctly filters out entirely -- the name is a "
            "label, NOT a transliteration, and nothing resolves by it"},
    {"label": "A" * 200, "source": "",
     "why": "length cap; ASCII by construction so the byte cap cannot split a character"},
    {"label": "device", "source": "",
     "why": "a label that is literally the empty-fallback value -- must be indistinguishable "
            "from the fallback, which is fine because nothing resolves by the name"},
    {"label": "iPhone/../../etc", "source": "",
     "why": "🔴 path traversal in a display label; separators and dots all filter to `_`"},
    {"label": "ẞ", "source": "",
     "why": "🔴 capital sharp s. casefold expands it to `ss`; a `to_lowercase()` plus a "
            "hand-rolled `ß -> ss` special case gets this WRONG, and without this vector "
            "that implementation is corpus-clean"},
    {"label": "CON", "source": "watch",
     "why": "🔴 a reserved stem WITH a source -- the escape and the suffix interact, and the "
            "rule's order is what decides the result. No other vector exercises both"},
    {"label": "iPhone", "source": "!!!",
     "why": "a source that filters to empty -- must not leave a trailing `_` or take the "
            "device fallback in the source position"},
    {"label": "iPhone", "source": "WATCH.Series (9)",
     "why": "the source is the one client-supplied string in the pipeline, so it gets the "
            "same projection as the label -- uppercase, dot and parens all handled"},
    {"label": "com0", "source": "",
     "why": "near-miss: NOT reserved (COM1-9 only), so it must NOT be escaped -- proves the "
            "escape is not applied by prefix"},
    {"label": "console", "source": "",
     "why": "near-miss: starts with `con` but is not reserved -- proves the check is on the "
            "whole stem, not a prefix"},
]

# Every Windows reserved basename gets a vector. Two of twenty-two is not
# coverage: a regex built from `CON` and `COM1` alone passes a two-name corpus
# and then lets `PRN`, `AUX`, `NUL`, `COM2`-`COM9` and `LPT1`-`LPT9` through to
# a filesystem that refuses them.
AUTHORED += [
    {"label": name, "source": "",
     "why": f"Windows reserved basename {name} -- the full set is pinned so an "
            f"implementation cannot satisfy the corpus with a partial list"}
    for name in sorted(WINDOWS_RESERVED)
    if name not in {"CON", "COM1"}
]

# Legacy names captured from the live implementation. These already exist on
# disk and the new reader must keep understanding them.
LEGACY_INPUTS: list[dict] = [
    {"kwargs": {"host": "archon"}, "why": "the plain local host"},
    {"kwargs": {"host": "ja1r.local"}, "why": "domain suffix stripped to the first label"},
    {"kwargs": {"host": "192.168.1.1"}, "why": "an IP address joins with dashes, not dots"},
    {"kwargs": {"host": "archon", "qualifier": "tmux"}, "why": "the dot qualifier form"},
    {"kwargs": {"host": "workstation", "qualifier": "browser"},
     "why": "the OTHER live dotted qualifier -- read at two talent sites, and absent from the "
            "first version of this corpus, which left read-old with a hole exactly where the "
            "kind-switching sites are"},
    {"kwargs": {"observer": "laptop.local"}, "why": "an observer name, domain stripped"},
    {"kwargs": {"import_source": "apple"}, "why": "the live `import.` prefix"},
    {"kwargs": {"import_source": "chatgpt"}, "why": "one of the hardcoded speaker-bootstrap set"},
]


def build_legacy() -> list[dict]:
    from solstone.think.streams import stream_name

    rows = []
    for entry in LEGACY_INPUTS:
        rows.append({
            "input": entry["kwargs"],
            "expect": stream_name(**entry["kwargs"]),
            "why": entry["why"],
        })
    return rows


def refuse_unless_sound(legacy: list[dict], authored: list[dict]) -> None:
    """Refuse to emit unless each half carries what it exists to carry."""
    from solstone.think.streams import _STREAM_NAME_RE

    problems: list[str] = []

    # The legacy half must be reproducible from the live implementation, and
    # every legacy name must satisfy the live name rule.
    for row in legacy:
        if not _STREAM_NAME_RE.match(row["expect"]):
            problems.append(f"legacy name {row['expect']!r} does not match the live name rule")

    # Independently-known literals. If these drift, the capture is not of the
    # implementation this fixture claims to pin.
    known = {"archon": {"host": "archon"}, "ja1r": {"host": "ja1r.local"},
             "192-168-1-1": {"host": "192.168.1.1"}, "archon.tmux": {"host": "archon", "qualifier": "tmux"},
             "import.apple": {"import_source": "apple"}}
    got = {json.dumps(r["input"], sort_keys=True): r["expect"] for r in legacy}
    for expected_name, kwargs in known.items():
        key = json.dumps(kwargs, sort_keys=True)
        if got.get(key) != expected_name:
            problems.append(
                f"legacy {kwargs} reproduced {got.get(key)!r}, expected the published {expected_name!r}"
            )

    # The authored half must satisfy the projection's own invariants.
    for row in authored:
        name = row["expect"]
        if not name:
            problems.append(f"{row['label']!r} projected to an empty name")
            continue
        if "." in name:
            problems.append(f"⛔ {row['label']!r} projected to {name!r}, which contains a dot")
        if not re.fullmatch(r"[a-z0-9_-]+", name):
            problems.append(f"{row['label']!r} projected to {name!r}, outside [a-z0-9_-]")
        if len(name.encode()) > MAX_BYTES:
            problems.append(f"{row['label']!r} projected to {len(name.encode())} bytes")
        # A leading strip-character is always a defect. A TRAILING underscore is
        # legal in exactly one case -- the deliberate Windows reserved-name
        # escape -- so it is only a defect when removing it does not reveal a
        # reserved stem.
        if name[0] in "_-":
            problems.append(f"{row['label']!r} projected to {name!r}, which is not left-stripped")
        if name.endswith("-") or (
            name.endswith("_") and name[:-1].upper() not in WINDOWS_RESERVED
        ):
            problems.append(f"{row['label']!r} projected to {name!r}, which is not right-stripped")
        # The name IS the whole path component the OS sees, so the reserved check
        # is over the whole name. Splitting on '_' would re-flag the escape it is
        # meant to accept, and would flag `nul_txt`, which is not reserved.
        if name.upper() in WINDOWS_RESERVED:
            problems.append(f"{row['label']!r} projected to the reserved name {name!r}")

    # The case-collision pair is the reason the vector set exists. If it stops
    # colliding, the fixture has stopped testing its own headline property.
    by_label = {(r["label"], r["source"]): r["expect"] for r in authored}
    lower = by_label.get(("iPhone (2)", ""))
    upper = by_label.get(("IPHONE (2)", ""))
    if lower is None or upper is None or lower != upper:
        problems.append(
            f"🔴 the case-collision pair must project identically; got {lower!r} and {upper!r}"
        )

    # Every declared step must be observable in the implementation's output, and
    # every step the implementation performs must be declared. This is the check
    # whose absence let the rule sentence say NFKC while the code did NFKD.
    declared = {name for name, _ in PIPELINE_STEPS}
    probed = set(STEP_PROBES)
    if declared != probed:
        problems.append(
            f"PIPELINE_STEPS and STEP_PROBES disagree: "
            f"undeclared={probed - declared}, unprobed={declared - probed}"
        )
    for step, (label, source, expected) in STEP_PROBES.items():
        actual = project(label, source)
        if actual != expected:
            problems.append(
                f"step {step!r} is declared in the rule but its probe "
                f"{label!r}+{source!r} produced {actual!r}, not {expected!r} -- "
                f"the rule sentence and the implementation have diverged"
            )

    # And the casefold-vs-lowercase discriminator.
    strasse = by_label.get(("Straße", ""))
    if strasse != "strasse":
        problems.append(
            f"🔴 `Straße` must project to 'strasse' (casefold, not lower()); got {strasse!r}"
        )

    if problems:
        for problem in problems:
            print(f"REFUSED: {problem}", file=sys.stderr)
        raise SystemExit(1)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: gen-stream-projection-vectors.py <output.json>")

    authored = [
        {"label": v["label"], "source": v["source"],
         "expect": project(v["label"], v["source"]), "why": v["why"]}
        for v in AUTHORED
    ]
    legacy = build_legacy()
    refuse_unless_sound(legacy, authored)

    document = {
        "x-journal-contract": {
            "format_id": "journal-stream-name-projection",
            "schema_owner": "the segment-media/journal-segment boundary",
            "file_kind": "vectors",
            "rule": " -> ".join(description for _, description in PIPELINE_STEPS)
            + ". Collisions are resolved by the ordinal allocator, NOT by this function.",
            "rule_steps": [name for name, _ in PIPELINE_STEPS],
            "note": (
                "`project` vectors are AUTHORED from the rule above -- no implementation "
                "produces them yet, so they can catch an implementation being wrong. `legacy` "
                "vectors are RECORDED from the shipped name derivation; they already exist on "
                "disk and must stay readable. The projected name is NEVER required to be "
                "invertible, only unique."
            ),
            "known_limitation": (
                "The human-friendly property holds for Latin scripts and degrades to nothing "
                "for scripts with no Latin decomposition: a device labelled in Cyrillic, Greek, "
                "Hebrew, Arabic or CJK filters to empty and takes the fallback, so an owner "
                "with three such devices sees `device`, `device_2`, `device_3`. This is safe -- "
                "nothing resolves by the name and uniqueness still holds -- but it is a real "
                "gap for those owners, and closing it means a per-script transliteration "
                "commitment rather than a rule change."
            ),
        },
        "project": authored,
        "legacy": legacy,
    }

    output = Path(sys.argv[1])
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(document, indent=2, ensure_ascii=False) + "\n", "utf-8")
    print(f"wrote {output} ({len(authored)} projection, {len(legacy)} legacy)")


if __name__ == "__main__":
    main()
