"""Extract the raw-media release oracle from the reference, by executing it.

The two readers that decide irreversibly about the owner's raw media do not
apply the same predicate, and they disagree in both directions. This emits the
disagreement as committed data so a port pins it rather than describing it.

For each constructed on-disk shape the fixture records three verdicts:

  reference_gate   what ``retention.resolve_segment_gate`` decides today.
                   ``eligible`` means the owner's raw media is unlinked.
  reference_proof  what ``processing_proof.has_terminal_processing_proof``
                   decides for the same bytes -- the conjunction a device is
                   told it may drop its only copy on.
  unified          what a single predicate must decide, derived mechanically
                   from the rule below rather than chosen per row.

and a ``disposition`` naming the relationship between the first and the third:

  agrees            the port keeps the reference's verdict
  narrowed          the port HOLDS raw the reference released. Safe direction:
                    the cost is disk, not the owner's data
  narrowed_legacy   as ``agrees``, but the release rests on pre-record row
                    evidence rather than on a processing record, so the port
                    must disclose which files were released on it
  widened           the port RELEASES raw the reference held. ANY row here is
                    a defect until argued otherwise, one row at a time

⛔ Verdicts are observed by running the reference on real files, never
hand-typed. The generator asserts ``widened`` is empty and exits non-zero if it
is not, so the fixture cannot silently record a loosening of an irreversible
path.

Usage:  python scripts/retention_release_oracle.py [--check]
"""

import json
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO))

from solstone.apps.observer.processing_proof import (  # noqa: E402
    has_terminal_processing_proof,
)
from solstone.observe.processing_record import (  # noqa: E402
    AUDIO_TRANSCRIPT_ROW_KEY,
    HANDLER_TRANSCRIBE,
    SCHEMA,
    STATE_ANALYZED,
    STATE_EMPTY,
    STATE_FAILED,
)
from solstone.think.retention import resolve_segment_gate  # noqa: E402

FIXTURE = REPO / "core" / "fixtures" / "retention_release_oracle.json"

MEDIA = "chunk_audio.flac"
MEDIA_BYTE = b"f"  # media content is arbitrary; only its LENGTH is load-bearing
IMAGE = "photo.png"
SIZE = 104

# The extension -> handler map is the caller's, not the predicate's. Retention
# receives it rather than carrying a copy; this is the smallest stand-in that
# covers the arms the fixture exercises. `None` means no handler in the closed
# set claims the content, so no proof is obtainable for it at any point.
HANDLER_BY_EXTENSION = {"flac": HANDLER_TRANSCRIBE}


def sidecar_name(media: str) -> str:
    """The same-stem sidecar both readers derive, per the reference."""
    return str(Path(media).with_suffix(".jsonl"))


def expected_handler_for(media: str) -> str | None:
    return HANDLER_BY_EXTENSION.get(Path(media).suffix.lower().lstrip("."))


TERMINAL_STATES = frozenset({STATE_ANALYZED, STATE_EMPTY})

# Cited so a reader can check any verdict against the line that decides it.
REFERENCE = {
    "segment_gate": "solstone/think/retention.py:190",
    "state_derivation": "solstone/think/data_state.py:121",
    "chunks_win": "solstone/think/data_state.py:145",
    "empty_arm": "solstone/think/data_state.py:147",
    "failed_arm": "solstone/think/data_state.py:139",
    "terminal_proof": "solstone/apps/observer/processing_proof.py:24",
    "raw_media_test": "solstone/think/retention.py:48",
    "image_sidecar_writer": "solstone/observe/depict.py:49",
    "extraction_globs": "solstone/think/retention.py:207",
}


def full_record(**overrides: object) -> dict:
    record = {
        "schema": SCHEMA,
        "state": STATE_ANALYZED,
        "reason_code": "ok",
        "handler": HANDLER_TRANSCRIBE,
        "attempted_at": "2026-08-05T00:00:00Z",
        "input_size": SIZE,
    }
    record.update(overrides)
    return record


def without(key: str) -> dict:
    record = full_record()
    del record[key]
    return record


ANALYSIS_ROW = {AUDIO_TRANSCRIPT_ROW_KEY: 0.0, "end": 1.0, "text": "hello"}
MARKER_ONLY_ROW = {AUDIO_TRANSCRIPT_ROW_KEY: 0.0}
NON_ANALYSIS_ROW = {"text": "hello"}


@dataclass(frozen=True)
class Shape:
    id: str
    record: dict | None
    row: dict | None
    note: str = ""
    # `record` absent from the header entirely vs. present-but-empty are
    # different on-disk shapes and the reference treats them differently.
    omit_record_key: bool = False
    # Non-audio media exercises the arm where no handler claims the content at
    # all, so no proof is obtainable for it at any point.
    media: str = MEDIA
    # Omit the sidecar entirely rather than writing a header-only one.
    omit_sidecar: bool = False


SHAPES = [
    Shape(
        "record_terminal_no_row",
        full_record(),
        None,
        "a record claiming analyzed with no analysis row on disk -- the derived "
        "output is missing and the raw is the only surviving copy",
    ),
    Shape(
        "record_terminal_with_row",
        full_record(),
        ANALYSIS_ROW,
        "the ordinary processed shape",
    ),
    Shape(
        "record_absent_with_row",
        None,
        ANALYSIS_ROW,
        "pre-record data: analysis rows, no processing record",
        omit_record_key=True,
    ),
    Shape("record_absent_no_row", None, None, "nothing has run", omit_record_key=True),
    Shape(
        "state_empty_bare",
        {"state": STATE_EMPTY},
        None,
        "the entire record is a state field",
    ),
    Shape(
        "state_empty_wrong_schema",
        full_record(state=STATE_EMPTY, schema="bogus.v9"),
        None,
    ),
    Shape(
        "state_empty_wrong_handler",
        full_record(state=STATE_EMPTY, handler="describe"),
        None,
    ),
    Shape(
        "state_empty_size_mismatch",
        full_record(state=STATE_EMPTY, input_size=999_999),
        None,
        "the bytes on disk are not the bytes that were processed",
    ),
    Shape(
        "state_empty_no_schema_key", without("schema") | {"state": STATE_EMPTY}, None
    ),
    Shape(
        "state_empty_no_handler_key", without("handler") | {"state": STATE_EMPTY}, None
    ),
    Shape(
        "state_empty_no_size_key", without("input_size") | {"state": STATE_EMPTY}, None
    ),
    Shape("state_analyzed_bare", {"state": STATE_ANALYZED}, None),
    Shape("state_unrecognized_with_row", {"state": "banana"}, ANALYSIS_ROW),
    Shape(
        "record_empty_object_with_row",
        {},
        ANALYSIS_ROW,
        "an empty record object still releases the owner's raw today",
    ),
    Shape(
        "marker_key_only_row",
        None,
        MARKER_ONLY_ROW,
        "a row carrying the marker key and nothing else; a real transcript row "
        "carries start, end and text",
        omit_record_key=True,
    ),
    Shape("non_analysis_row", None, NON_ANALYSIS_ROW, omit_record_key=True),
    Shape("state_failed", full_record(state=STATE_FAILED), None),
    Shape(
        "state_failed_with_row",
        full_record(state=STATE_FAILED),
        ANALYSIS_ROW,
        "a failed record beside rows that exist",
    ),
    Shape(
        "record_terminal_wrong_handler_with_row",
        full_record(handler="describe"),
        ANALYSIS_ROW,
        "the audio was consumed by the screen handler, which cannot be true",
    ),
    Shape(
        "record_terminal_size_mismatch_with_row",
        full_record(input_size=999_999),
        ANALYSIS_ROW,
        "rows exist but the file has changed size since it was processed",
    ),
    # The positive case the deletion ruling routes to retention: VAD found under
    # a second of speech, the handler wrote a terminal-empty record, and the raw
    # is handed over rather than unlinked in place. `empty` is the one terminal
    # state that legitimately carries no analysis rows.
    Shape(
        "record_empty_full_no_row",
        full_record(state=STATE_EMPTY, reason_code="no_decodable_audio"),
        None,
        "terminal-empty: the handler ran, determined there was nothing to "
        "transcribe, and recorded it. This is the shape the raw hand-off produces",
    ),
    # Still images: no handler in the closed set claims them, so no proof is
    # obtainable for them at any point.
    Shape(
        "image_with_record",
        full_record(state=STATE_EMPTY, handler="depict"),
        None,
        "an image whose sidecar names a handler that is not in the closed set",
        media=IMAGE,
    ),
    Shape(
        "image_no_sidecar",
        None,
        None,
        "an image alone. The reference releases it with no evidence it was ever "
        "processed and none that it was ever looked at",
        omit_record_key=True,
        media=IMAGE,
        omit_sidecar=True,
    ),
]


def sidecar_lines(shape: Shape) -> list[str] | None:
    """The sidecar's literal lines, or None when there is no sidecar file.

    This is the single source for both the on-disk file `build()` writes and the
    `sidecar_lines` the fixture publishes, so a consumer rebuilding a row cannot
    construct a different file than the reference was measured against. The
    metadata-header wrapper is the reason it matters: the record is a VALUE UNDER
    `_solstone_processing`, not the whole first line, and a consumer that guesses
    otherwise gets header-only semantics on every record-bearing row -- which
    most rows already report as held, so the mistake stays green.
    """
    if shape.omit_sidecar:
        return None
    header: dict = {"segment": shape.id}
    if not shape.omit_record_key:
        header["_solstone_processing"] = shape.record
    lines = [json.dumps(header)]
    if shape.row is not None:
        lines.append(json.dumps(shape.row))
    return lines


def build(root: Path, shape: Shape) -> Path:
    segment = root / shape.id
    segment.mkdir(parents=True)
    (segment / shape.media).write_bytes(MEDIA_BYTE * SIZE)
    lines = sidecar_lines(shape)
    if lines is not None:
        (segment / sidecar_name(shape.media)).write_text("\n".join(lines) + "\n")
    return segment


def unified(shape: Shape) -> dict:
    """The single predicate, applied mechanically.

    Five conditions, in order. The first four are the terminal-proof
    conjunction the resolve path already runs; the fifth is the lesson the
    reference's state derivation encodes and a naive unification would drop.
    """
    record = None if shape.omit_record_key else shape.record
    has_row = shape.row is not None and AUDIO_TRANSCRIPT_ROW_KEY in shape.row

    # 0. A handler must claim the content, or no proof is obtainable for it at
    #    any point. This arm is what makes a still image unreleasable until its
    #    handler joins the closed set, and it is the whole reason the map is
    #    injected rather than carried here.
    expected_handler = expected_handler_for(shape.media)
    if expected_handler is None:
        return {
            "verdict": "held",
            "blocker": "unprovable",
            "because": "no handler in the closed set claims this content, so no "
            "proof is obtainable for it at any point",
        }

    if record is None:
        # Read old: analysis rows are real evidence the media was consumed,
        # weaker than a record. Honoured, tagged, disclosed.
        if has_row:
            return {
                "verdict": "releasable",
                "evidence": "legacy_rows",
                "because": "no processing record; analysis rows present",
            }
        return {
            "verdict": "held",
            "blocker": "incomplete",
            "because": "no processing record and no analysis rows",
        }

    if not isinstance(record, dict):
        return {
            "verdict": "held",
            "blocker": "incomplete",
            "because": "the record is not an object",
        }

    # 1-4: the conjunction, evaluated exactly as the resolve path evaluates it.
    #      The size supplied is the size ON DISK, not the manifest size -- this
    #      caller is about to delete these bytes, so it must prove these bytes.
    if record.get("schema") != SCHEMA:
        return {
            "verdict": "held",
            "blocker": "incomplete",
            "because": "schema is absent or unrecognized",
        }
    state = record.get("state")
    if state == STATE_FAILED:
        return {
            "verdict": "held",
            "blocker": "failed",
            "because": "processing failed; disclose, never release",
        }
    if state not in TERMINAL_STATES:
        return {
            "verdict": "held",
            "blocker": "incomplete",
            "because": "state is not terminal",
        }
    if record.get("handler") != expected_handler:
        return {
            "verdict": "held",
            "blocker": "incomplete",
            "because": "the recorded handler is not the one that claims this content",
        }
    if record.get("input_size") != SIZE:
        return {
            "verdict": "held",
            "blocker": "incomplete",
            "because": "the bytes on disk are not the bytes that were processed",
        }

    # 5. A record claiming `analyzed` asserts derived output exists. If the rows
    #    are gone the raw is the only surviving copy of that content, and
    #    releasing it is unrecoverable. `empty` is the terminal state that
    #    legitimately has no rows.
    #    This is what the reference's chunks_win ordering encodes, and a
    #    unification built only from the conjunction drops it.
    if state == STATE_ANALYZED and not has_row:
        return {
            "verdict": "held",
            "blocker": "incomplete",
            "because": "the record claims analyzed and the analysis rows are gone; "
            "the raw is the only surviving copy",
        }

    if state == STATE_EMPTY:
        because = (
            "the conjunction held and the handler determined there was nothing to "
            "derive, so no analysis rows are expected"
        )
    else:
        because = "the conjunction held and the derived output is on disk"
    return {"verdict": "releasable", "evidence": "record", "because": because}


def disposition(gate: str, decision: dict) -> str:
    released_by_reference = gate == "eligible"
    releases = decision["verdict"] == "releasable"
    if released_by_reference == releases:
        if releases and decision.get("evidence") == "legacy_rows":
            return "narrowed_legacy"
        return "agrees"
    return "narrowed" if released_by_reference else "widened"


def rebuild_check(document: dict) -> list[str]:
    """Rebuild every row using ONLY the published JSON and re-measure it.

    This is what makes the fixture a usable acceptance input rather than a
    description of one. A consumer that has never read this script must be able
    to reconstruct each row's on-disk shape and get `reference_gate` and
    `reference_proof` back; if it cannot, any test built on this file is
    asserting against a shape the reference never saw.

    \u26a0 The reference verdicts are the self-check. Measured: a consumer that
    reconstructs the sidecar from the `sidecar` field instead of `sidecar_lines`
    -- the natural wrong guess, since `sidecar` looks like the file's first line
    -- reproduces 11 of 23 rows and fails 12 once both reference verdicts are
    compared. Comparing only a port's own verdict would leave most of them green.
    """
    failures = []
    root = Path(tempfile.mkdtemp(prefix="retention-oracle-rebuild-"))
    try:
        for row in document["rows"]:
            shape = row["shape"]
            segment = root / row["id"]
            segment.mkdir(parents=True)
            media = segment / shape["media"]
            media.write_bytes(shape["media_byte"].encode() * shape["size"])
            if shape["sidecar_lines"] is not None:
                (segment / shape["sidecar_file"]).write_text(
                    "\n".join(shape["sidecar_lines"]) + "\n"
                )
            gate = resolve_segment_gate(segment).verdict
            proof = has_terminal_processing_proof(media, shape["size"])
            if gate != row["reference_gate"]:
                failures.append(
                    f"{row['id']}: reference_gate {row['reference_gate']!r} "
                    f"but a JSON-only rebuild measured {gate!r}"
                )
            if proof != row["reference_proof"]:
                failures.append(
                    f"{row['id']}: reference_proof {row['reference_proof']!r} "
                    f"but a JSON-only rebuild measured {proof!r}"
                )
    finally:
        shutil.rmtree(root, ignore_errors=True)
    return failures


def main() -> int:
    check = "--check" in sys.argv[1:]
    root = Path(tempfile.mkdtemp(prefix="retention-oracle-"))
    rows = []
    try:
        for shape in SHAPES:
            segment = build(root, shape)
            gate = resolve_segment_gate(segment).verdict
            proof = has_terminal_processing_proof(segment / MEDIA, SIZE)
            decision = unified(shape)
            row = {
                "id": shape.id,
                "shape": {
                    "media": shape.media,
                    "size": SIZE,
                    # Everything a consumer needs to rebuild this row byte for
                    # byte. `sidecar_lines` is the literal file content, newline
                    # separated with a trailing newline; null means no file.
                    "sidecar_file": sidecar_name(shape.media),
                    "sidecar_lines": sidecar_lines(shape),
                    "media_byte": MEDIA_BYTE.decode(),
                    # Three distinguishable states, because two of them are
                    # different on disk and a consumer rebuilding this fixture
                    # must be able to tell them apart:
                    #   sidecar "absent"          -> no <stem>.jsonl file at all
                    #   sidecar "header-only"     -> the file exists, no record key
                    #   sidecar {...}             -> the file exists with that record
                    "sidecar": (
                        "absent"
                        if shape.omit_sidecar
                        else "header-only"
                        if shape.omit_record_key
                        else shape.record
                    ),
                    "analysis_row": shape.row,
                },
                "reference_gate": gate,
                "reference_releases_raw": gate == "eligible",
                "reference_proof": proof,
                "unified": decision,
                "disposition": disposition(gate, decision),
                "provenance": "observed",
            }
            if shape.note:
                row["note"] = shape.note
            rows.append(row)
    finally:
        shutil.rmtree(root, ignore_errors=True)

    widened = [row["id"] for row in rows if row["disposition"] == "widened"]

    document = {
        "_provenance": {
            "generator": "scripts/retention_release_oracle.py",
            "method": (
                "the reference verdicts are OBSERVED by running the reference over "
                "constructed segments on a real filesystem; the unified verdicts are "
                "derived mechanically from one rule, not chosen per row"
            ),
            "source_revision": revision(),
            "reference": REFERENCE,
            "rebuild": (
                "each row is fully self-describing: write `size` copies of "
                "`media_byte` to `media`, and if `sidecar_lines` is non-null write "
                "those lines to `sidecar_file`, newline separated with a trailing "
                "newline. \u26d4 Do NOT reconstruct the sidecar from `sidecar` -- "
                "that field is the record VALUE, and the record lives under the "
                "`_solstone_processing` key of a metadata header, not as the whole "
                "first line. Guessing wrong yields header-only semantics on every "
                "record-bearing row, and most of those already report held, so the "
                "mistake stays green."
            ),
            "reading": (
                "reference_gate 'eligible' means the reference unlinks the owner's raw "
                "media. reference_proof is the four-condition conjunction the resolve "
                "path runs on the same bytes. They disagree in both directions, which is "
                "why this file exists."
            ),
            "invariant": (
                "no row may carry disposition 'widened'. A port that releases raw the "
                "reference held has loosened an irreversible path, and each such row "
                "needs its own argument before it is recorded here."
            ),
        },
        "summary": tally(rows),
        "rows": rows,
    }

    rendered = json.dumps(document, indent=2, sort_keys=False) + "\n"

    if widened:
        print(
            f"FAIL: rows widen the irreversible path: {', '.join(widened)}",
            file=sys.stderr,
        )
        return 1

    rebuild_failures = rebuild_check(document)
    if rebuild_failures:
        print(
            "FAIL: rows do not rebuild from the published JSON alone:", file=sys.stderr
        )
        for failure in rebuild_failures:
            print(f"  {failure}", file=sys.stderr)
        return 1

    if check:
        if not FIXTURE.exists():
            print(f"FAIL: {FIXTURE} does not exist", file=sys.stderr)
            return 1
        current = FIXTURE.read_text()
        if current != rendered:
            print(f"FAIL: {FIXTURE} is stale; regenerate it", file=sys.stderr)
            return 1
        print(
            f"ok: {FIXTURE.relative_to(REPO)} matches the reference, "
            f"and all {len(rows)} rows rebuild from the JSON alone"
        )
        return 0

    FIXTURE.parent.mkdir(parents=True, exist_ok=True)
    FIXTURE.write_text(rendered)
    print(
        f"wrote {FIXTURE.relative_to(REPO)}: {len(rows)} rows, "
        f"all rebuildable from the JSON alone"
    )
    for name, count in tally(rows).items():
        print(f"  {name}: {count}")
    return 0


def tally(rows: list[dict]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for row in rows:
        counts[row["disposition"]] = counts.get(row["disposition"], 0) + 1
    return dict(sorted(counts.items()))


def revision() -> str:
    """The revision of the reference this oracle was read from.

    ⛔ Deliberately NOT ``rev-parse HEAD``. Pinning HEAD makes the committed
    fixture stale on every unrelated commit in the repository, so the gate reds
    for a reason that has nothing to do with the reference it guards -- which
    means the gate is the defect. Pinning the last commit that touched the
    reference files goes stale exactly when the fixture should be regenerated,
    and not otherwise.
    """
    sources = sorted({citation.split(":", 1)[0] for citation in REFERENCE.values()})
    try:
        return subprocess.run(
            ["git", "log", "-1", "--format=%H", "--", *sources],
            cwd=REPO,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


if __name__ == "__main__":
    sys.exit(main())
