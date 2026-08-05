# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance corpus for the entity identity store.

This corpus is extracted from the Python implementation and is the differential
oracle a native reimplementation is proven against. It exists because the entity
store fails by *refusing* rather than by degrading: the identity history tree and
``ambiguities.jsonl`` are self-validating, so a subtly-wrong writer makes an
entity permanently unmutatable rather than producing a worse result.

Three properties make the corpus trustworthy:

* **Real code paths.** Durable bytes are captured by running the actual writers
  against a temporary journal and reading the files back, never by restating a
  ``json.dumps`` call here. A restatement would drift the moment the writer did.
* **Full sweeps, digested.** The identity functions are swept over *every*
  Unicode scalar value and reduced to a digest, so a change anywhere in the
  1,112,064-codepoint space is caught. Explicit vectors accompany the digest for
  the cases worth reading.
* **Determinism.** Nothing here samples a clock, a UUID or the filesystem
  ordering. Inputs are fixed, so ``--check`` measures drift in the
  implementation rather than noise from the run.

⚠ The digests are a property of the interpreter's Unicode data and of the
transliteration tables, both of which are recorded alongside them. A digest that
changes when those version strings also changed is a dependency bump, not a
regression.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
import tempfile
import unicodedata
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Callable, Iterator

ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

# Surrogates are not scalar values; chr() accepts them but they cannot appear in
# well-formed text and are not representable in the target language's `char`.
_SURROGATE_LO = 0xD800
_SURROGATE_HI = 0xDFFF
_MAX_CODEPOINT = 0x110000


def _all_scalar_values() -> Iterator[int]:
    for cp in range(_MAX_CODEPOINT):
        if _SURROGATE_LO <= cp <= _SURROGATE_HI:
            continue
        yield cp


def _digest(lines: Iterator[str]) -> str:
    digest = hashlib.sha256()
    for line in lines:
        digest.update(line.encode("utf-8"))
        digest.update(b"\n")
    return digest.hexdigest()


@contextmanager
def _temp_journal() -> Iterator[Path]:
    """Point the journal at a scratch tree for the duration of the block.

    The writers resolve their root through ``get_journal()`` on every call, which
    reads ``SOLSTONE_JOURNAL`` first. Setting it here is what lets the corpus
    capture real writer output without touching a real journal.
    """
    previous = os.environ.get("SOLSTONE_JOURNAL")
    with tempfile.TemporaryDirectory(prefix="entity-corpus-") as tmp:
        os.environ["SOLSTONE_JOURNAL"] = tmp
        try:
            yield Path(tmp)
        finally:
            if previous is None:
                os.environ.pop("SOLSTONE_JOURNAL", None)
            else:
                os.environ["SOLSTONE_JOURNAL"] = previous


# --------------------------------------------------------------------------
# identity functions
# --------------------------------------------------------------------------

# Names chosen to stress one decision each, not to look like realistic input.
_SLUG_PROBES: tuple[str, ...] = (
    "Alice Johnson",
    "O'Brien",
    "AT&T",
    "C++",
    "PostgreSQL",
    "José García",
    unicodedata.normalize("NFD", "José García"),
    "Müller",
    unicodedata.normalize("NFD", "Müller"),
    "北京",
    "Москва",
    "Αθήνα",
    "İstanbul",
    "ırmak",
    "Straße",
    "STRASSE",
    "  Spaced  Out  ",
    "Tab\tSeparated",
    "New\nLine",
    "",
    "   ",
    "Ångström",
    "① Project",
    "café",
    unicodedata.normalize("NFD", "café"),
    "emoji 🎉 name",
    "Ann-Marie",
    "Jean_Luc",
    "van der Berg",
    "ﬁre",
    "Ⅻ legion",
    "ｆｕｌｌｗｉｄｔｈ",
    "x y",
    "a b",
    "Dr. Smith, Jr.",
    "École",
    "Ægis",
    "Œuvre",
    "ß only",
    # Above the truncation ceiling, where the md5 suffix is taken over the RAW
    # input rather than the slug — so the same name in two normalization forms
    # yields two different ids. Pinned deliberately: it is the behaviour, and a
    # reimplementation that normalizes before hashing would silently diverge.
    "Bartholomew Fitzgerald Müller " * 9,
    unicodedata.normalize("NFD", "Bartholomew Fitzgerald Müller " * 9),
    "A" * 250,
    "A" * 250 + " Alpha",
    "A" * 250 + " Beta",
)

# Whitespace-adjacent codepoints. The three splitters in play here do not agree:
# Python's ``str.split``, rapidfuzz's tokenizer, and the target language's
# ``split_whitespace`` each treat a different set as separators.
_WHITESPACE_PROBES: tuple[int, ...] = (
    0x09, 0x0A, 0x0B, 0x0C, 0x0D,
    0x1C, 0x1D, 0x1E, 0x1F,
    0x20, 0x85, 0xA0,
    0x1680, 0x2000, 0x2028, 0x2029, 0x202F, 0x205F, 0x3000,
)


def _slug_sweep_lines() -> Iterator[str]:
    from solstone.think.entities.core import entity_slug

    for cp in _all_scalar_values():
        yield entity_slug("A" + chr(cp) + "B")


def _normalize_sweep_lines() -> Iterator[str]:
    from solstone.think.entities.ambiguities import normalize_resolution_query

    for cp in _all_scalar_values():
        yield normalize_resolution_query("A" + chr(cp) + "B")


def _versions() -> dict[str, str]:
    import importlib.metadata

    def _dist(name: str) -> str:
        try:
            return importlib.metadata.version(name)
        except importlib.metadata.PackageNotFoundError:  # pragma: no cover
            return "unknown"

    return {
        "unicodedata": unicodedata.unidata_version,
        "text_unidecode": _dist("text-unidecode"),
        "python_slugify": _dist("python-slugify"),
        "rapidfuzz": _dist("rapidfuzz"),
    }


def build_entity_identity_fixture() -> dict[str, Any]:
    """entity_slug and normalize_resolution_query, swept and pinned."""
    from solstone.think.entities.ambiguities import (
        ResolutionScope,
        ambiguity_id_for_key,
        ambiguity_key,
        normalize_resolution_query,
    )
    from solstone.think.entities.core import MAX_ENTITY_SLUG_LENGTH, entity_slug

    slug_vectors = [
        {"name": name, "slug": entity_slug(name)} for name in _SLUG_PROBES
    ]
    slug_vectors += [
        {
            "name": "A" + chr(cp) + "B",
            "codepoint": f"U+{cp:04X}",
            "slug": entity_slug("A" + chr(cp) + "B"),
        }
        for cp in _WHITESPACE_PROBES
    ]

    normalize_vectors = [
        {"query": name, "normalized": normalize_resolution_query(name)}
        for name in _SLUG_PROBES
    ]
    normalize_vectors += [
        {
            "query": "A" + chr(cp) + "B",
            "codepoint": f"U+{cp:04X}",
            "normalized": normalize_resolution_query("A" + chr(cp) + "B"),
        }
        for cp in _WHITESPACE_PROBES
    ]

    scopes = (ResolutionScope.journal(), ResolutionScope.facet_scope("work"))
    ambiguity_vectors = []
    for scope in scopes:
        for query in ("Alice Johnson", "Straße", "ΟΔΥΣΣΕΥΣ", "ﬁle", "  spaced  out  "):
            normalized = normalize_resolution_query(query)
            key = ambiguity_key(scope, normalized)
            ambiguity_vectors.append(
                {
                    "scope": scope.to_dict(),
                    "query": query,
                    "normalized_query": normalized,
                    "key": key,
                    "ambiguity_id": ambiguity_id_for_key(key),
                }
            )

    return {
        "generated_by": "make core-fixtures",
        "versions": _versions(),
        "entity_slug": {
            "max_length": MAX_ENTITY_SLUG_LENGTH,
            "sweep": {
                "description": (
                    "entity_slug('A' + chr(cp) + 'B') for every Unicode scalar "
                    "value, in ascending codepoint order, one result per line"
                ),
                "scalar_values": sum(1 for _ in _all_scalar_values()),
                "sha256": _digest(_slug_sweep_lines()),
            },
            "vectors": slug_vectors,
        },
        "normalize_resolution_query": {
            "sweep": {
                "description": (
                    "normalize_resolution_query('A' + chr(cp) + 'B') for every "
                    "Unicode scalar value, in ascending codepoint order"
                ),
                "scalar_values": sum(1 for _ in _all_scalar_values()),
                "sha256": _digest(_normalize_sweep_lines()),
            },
            "vectors": normalize_vectors,
        },
        "ambiguity_id": {
            "description": (
                "amb_ + sha256(scope_key|normalized_query)[:24]; the normalized "
                "query folds case, so the fold is inside an identity"
            ),
            "vectors": ambiguity_vectors,
        },
    }


# --------------------------------------------------------------------------
# matching
# --------------------------------------------------------------------------

_MATCH_NAMES: tuple[str, ...] = (
    "Robert Johnson",
    "Robert Smith",
    "Roberta Kim",
    "Alice Chen",
    "Alice Chan",
    "Jose Garcia Marquez",
    "Jose Garcia",
    "Maria Elena Sanchez",
    "Maria Sanchez",
    "İstanbul Metal Works",
    "Istanbul Metal Works",
    "Straße Handels GmbH",
)


def _candidate(name: str, *, with_id: bool = True, aka: tuple[str, ...] = (),
               emails: tuple[str, ...] = ()) -> dict[str, Any]:
    from solstone.think.entities.core import entity_slug

    entity: dict[str, Any] = {"name": name, "type": "Person"}
    if with_id:
        entity["id"] = entity_slug(name)
    if aka:
        entity["aka"] = list(aka)
    if emails:
        entity["emails"] = list(emails)
    return entity


def _match_cases() -> list[dict[str, Any]]:
    """Cases chosen to reach every tier and, deliberately, every refusal."""
    cases: list[dict[str, Any]] = []

    def case(query: str, names: list[dict[str, Any]], note: str) -> None:
        cases.append({"query": query, "candidates": names, "note": note})

    exact = _candidate("Robert Johnson", aka=("Bob", "Bobby"),
                       emails=("bob@example.com",))
    case("Robert Johnson", [exact], "tier 1 — exact name")
    case("Bob", [exact], "tier 1 — exact aka")
    case("robert_johnson", [exact], "tier 1 — exact id")
    case("robert johnson", [exact], "tier 2 — case-insensitive")
    case("BOB@EXAMPLE.COM", [exact], "tier 3 — email, case-folded")
    case("Robert  Johnson!", [exact], "tier 4 — slugified query against id")
    case("Roberta", [_candidate("Roberta Kim"), _candidate("Alice Chen")],
         "tier 5 — first word, unambiguous")
    case("Robert", [_candidate("Robert Johnson"), _candidate("Robert Smith")],
         "REFUSAL — first word is ambiguous, so no match")
    case("Jose Garcia", [_candidate("Jose Garcia Marquez")],
         "tier 6 — token subset")
    case("Maria Sanchez",
         [_candidate("Maria Elena Sanchez"), _candidate("Maria Sanchez Lopez")],
         "REFUSAL — token subset is ambiguous")
    case("Robe", [_candidate("Robert Johnson")], "tier 7 — prefix token")
    case("Alic", [_candidate("Alice Chen"), _candidate("Alice Chan")],
         "REFUSAL — prefix is ambiguous")
    case("Alicia Johnson", [_candidate("Alice Johnson")],
         "tier 8 — fuzzy at or above the threshold")
    case("Completely Different", [_candidate("Robert Johnson")],
         "no match at any tier")
    case("Jo", [_candidate("Jo Nesbo")],
         "first word below the 3-char floor")
    case("Robert Johnson", [], "empty candidate set")
    case("", [exact], "empty query")
    # An id-less candidate: the id map falls back to the slug of the name.
    case("alice_chen", [_candidate("Alice Chen", with_id=False)],
         "id absent — slug of the name backs the id map")
    # Unicode pairs where the tier reached is the whole question.
    for a, b in (
        ("İstanbul Metal Works", "Istanbul Metal Works"),
        ("Straße Handels GmbH", "Strasse Handels GmbH"),
        (unicodedata.normalize("NFD", "Müller Werke"), "Müller Werke"),
    ):
        case(a, [_candidate(b)], f"unicode pair — {a!r} against {b!r}")
    # Larger populations, so ambiguity detection sees real competition.
    for name in _MATCH_NAMES:
        case(name, [_candidate(n) for n in _MATCH_NAMES], "full population")
        case(name.split()[0], [_candidate(n) for n in _MATCH_NAMES],
             "first word against the full population")
    return cases


def build_entity_matching_fixture() -> dict[str, Any]:
    """find_matching_entity decisions, including every refusal."""
    from solstone.think.entities.matching import find_matching_entity

    vectors = []
    for case in _match_cases():
        result = find_matching_entity(case["query"], case["candidates"], 90)
        if result is None:
            outcome: dict[str, Any] = {"matched": False}
        else:
            index = next(
                i
                for i, c in enumerate(case["candidates"])
                if c.get("name") == result.get("name")
                and c.get("id") == result.get("id")
            )
            outcome = {
                "matched": True,
                "candidate_index": index,
                "tier": int(result.tier),
                "high_confidence": bool(result.is_high_confidence),
            }
        vectors.append({**case, "outcome": outcome})

    return {
        "generated_by": "make core-fixtures",
        "versions": _versions(),
        "fuzzy_threshold": 90,
        "tiers": {
            "1": "exact name, id or aka",
            "2": "case-insensitive name, id or aka",
            "3": "email",
            "4": "slugified query against id",
            "5": "first word, bidirectional, unambiguous only, min 3 chars",
            "6": "token subset, unambiguous only, min 2 tokens in the shorter",
            "7": "prefix token, unambiguous only, 4-char minimum prefix",
            "8": "fuzzy, rapidfuzz token_sort_ratio at or above the threshold",
        },
        "high_confidence_max_tier": 4,
        "note": (
            "Tiers 1-4 are high confidence and resolve silently. Tiers 5-8 are "
            "low confidence, and the caller surfaces them to the owner rather "
            "than guessing. A reimplementation that moves a case across the "
            "tier-4 boundary changes whether an owner is asked at all."
        ),
        "vectors": vectors,
    }


# --------------------------------------------------------------------------
# durable formats
# --------------------------------------------------------------------------

# Fixed identities. No clock, no uuid — the corpus pins serialization, and a
# generated timestamp would make every regeneration look like drift.
_ENTITY_FIXED: dict[str, Any] = {
    "id": "alice_johnson",
    "name": "Alice Johnson",
    "type": "Person",
    "created_at": 1785889922582,
    "aka": ["Ali", "AJ"],
    "emails": ["alice@example.com"],
}

_ENTITY_UNICODE: dict[str, Any] = {
    "id": "jose_garcia",
    "name": "José García",
    "type": "Person",
    "created_at": 1785889922583,
    "description": "raw UTF-8 on disk, never \\u escapes",
}


def _capture_identity_bytes(entity: dict[str, Any]) -> str:
    from solstone.think.entities.history import (
        _identity_path,
        _write_identity_snapshot,
    )

    _write_identity_snapshot(str(entity["id"]), entity)
    return _identity_path(str(entity["id"])).read_text(encoding="utf-8")


def _capture_event_bytes(event: dict[str, Any]) -> str:
    from solstone.think.entities.history import _events_dir, _write_json

    path = _events_dir("alice_johnson") / "probe.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    _write_json(path, event)
    return path.read_text(encoding="utf-8")


_HISTORY_EVENT_FIXED: dict[str, Any] = {
    "schema_version": 1,
    "version_id": "vh_49d7adcbf786461cb11c00081afa9780",
    "seq": 1,
    "ts": "2026-08-05T00:32:02.582506Z",
    "entity_id": "alice_johnson",
    "kind": "create",
    "caller": None,
    "actor": None,
    "identity_before": None,
    "identity_after": _ENTITY_FIXED,
    "operation": {},
}


def _reconciliation_cases() -> list[dict[str, Any]]:
    """The three outcomes of prepared-event reconciliation.

    This is the mechanism that makes the store fail by refusing. A prepared
    event is published when the identity on disk equals its ``identity_after``,
    discarded when it equals ``identity_before``, and otherwise the entity is
    left permanently unmutatable until a human repairs it.

    ⚠ Equality here is the *interpreter's* structural equality, which treats
    ``1785889922582`` and ``1785889922582.0`` as equal. A reimplementation whose
    value equality is stricter refuses where this one recovers.
    """
    before = dict(_ENTITY_FIXED)
    after = {**_ENTITY_FIXED, "description": "a friend"}
    numeric = {**_ENTITY_FIXED, "created_at": float(_ENTITY_FIXED["created_at"])}
    reordered = {k: _ENTITY_FIXED[k] for k in reversed(list(_ENTITY_FIXED))}
    return [
        {
            "note": "disk equals identity_after — publish",
            "disk": after, "before": before, "after": after,
            "outcome": "publish",
        },
        {
            "note": "disk equals identity_before — discard",
            "disk": before, "before": before, "after": after,
            "outcome": "discard",
        },
        {
            "note": "disk matches neither — repair required, journal unchanged",
            "disk": {**before, "schema_version": 2},
            "before": before, "after": after,
            "outcome": "repair_required",
        },
        {
            "note": (
                "int vs float on created_at — equal under this interpreter's "
                "structural equality, so it DISCARDS rather than refusing"
            ),
            "disk": numeric, "before": before, "after": after,
            "outcome": "discard",
        },
        {
            "note": "key order is not part of equality — discard",
            "disk": reordered, "before": before, "after": after,
            "outcome": "discard",
        },
        {
            "note": "a missing optional field is not the same as an empty one",
            "disk": {k: v for k, v in before.items() if k != "aka"},
            "before": before, "after": after,
            "outcome": "repair_required",
        },
    ]


def _resolve_reconciliation(case: dict[str, Any]) -> str:
    """Compute the outcome with the same predicate the store uses."""
    disk, before, after = case["disk"], case["before"], case["after"]
    if disk == after:
        return "publish"
    if disk == before:
        return "discard"
    return "repair_required"


def _ambiguity_rows() -> list[dict[str, Any]]:
    """One open row and one resolved row, built through the real primitives.

    ⚠ ``origin_keys`` entries are a **byte-exact** serialized form — sorted
    keys, compact separators, and ASCII escaping left on — persisted and later
    compared with plain string equality. A space after a colon, or raw UTF-8
    where an escape is expected, silently duplicates an origin instead of
    matching it. They are produced here by the real ``key()`` for that reason.
    """
    from solstone.think.entities.ambiguities import (
        AMBIGUITY_SCHEMA_VERSION,
        ResolutionOrigin,
        ResolutionScope,
        ambiguity_id_for_key,
        ambiguity_key,
        normalize_resolution_query,
    )

    specs = (
        (
            ResolutionScope.journal(),
            "Alice",
            5,
            ResolutionOrigin(lane="segment", day="20260804", segment_id="s1"),
            None,
        ),
        (
            ResolutionScope.facet_scope("work"),
            "Straße",
            8,
            ResolutionOrigin(lane="import", source_id="kindle", field="author"),
            "strasse_handels_gmbh",
        ),
    )

    rows = []
    for scope, query, tier, origin, resolved in specs:
        normalized = normalize_resolution_query(query)
        key = ambiguity_key(scope, normalized)
        row: dict[str, Any] = {
            "schema_version": AMBIGUITY_SCHEMA_VERSION,
            "ambiguity_id": ambiguity_id_for_key(key),
            "scope": scope.to_dict(),
            "normalized_query": normalized,
            "original_query": query,
            "latest_query": query,
            "first_seen": "2026-08-04T00:00:00Z",
            "last_seen": "2026-08-04T00:00:00Z",
            "observed_tier": tier,
            "occurrence_count": 1,
            "status": "resolved" if resolved else "open",
            "ranked_candidates": [
                {
                    "id": "alice_chen",
                    "name": "Alice Chen",
                    "tier": tier,
                    "score": 92.3076923076923,
                },
                {
                    "id": "alice_chan",
                    "name": "Alice Chan",
                    "tier": tier,
                    "score": 90.0,
                },
            ],
            "origins": [origin.to_dict()],
            "origin_keys": [origin.key()],
            "audit": {"prior_choices": []},
        }
        if resolved:
            row["resolved_entity_id"] = resolved
            row["resolved_at"] = "2026-08-04T00:00:00Z"
        rows.append(row)
    return rows


def _negative_cases() -> list[dict[str, Any]]:
    """One deliberately malformed input per validation rule.

    A validator that has never rejected anything has not been tested. Each entry
    carries the refusal the real validator actually produced, so a
    reimplementation can assert not merely that it refused, but that it refused
    *for the same reason*.

    ⚠ The capture below is not decoration. If a mutation stops being rejected —
    because a rule was relaxed, or because the mutation stopped reaching it —
    building the corpus fails loudly rather than shipping a case that proves
    nothing. The valid rows are checked the same way, in the other direction.
    """
    from pathlib import Path as _Path

    from solstone.think.entities.ambiguities import (
        EntityAmbiguityError,
        _validate_row,
    )

    probe = _Path("<corpus>")

    for lineno, row in enumerate(_ambiguity_rows(), start=1):
        try:
            _validate_row(row, probe, lineno)
        except EntityAmbiguityError as exc:  # pragma: no cover - corpus guard
            raise AssertionError(
                f"corpus row {lineno} is supposed to be valid but was refused: {exc}"
            ) from exc

    valid = _ambiguity_rows()[0]

    def broken(mutate: Callable[[dict[str, Any]], None], rule: str) -> dict[str, Any]:
        row = json.loads(json.dumps(valid))
        mutate(row)
        try:
            _validate_row(row, probe, 1)
        except EntityAmbiguityError as exc:
            # The trailing clause is the validator's own wording for the rule.
            refusal = str(exc).rsplit(": ", 1)[-1]
            return {"rule": rule, "refusal": refusal, "row": row}
        raise AssertionError(  # pragma: no cover - corpus guard
            f"negative case {rule!r} was accepted; it no longer tests anything"
        )

    return [
        broken(lambda r: r.pop("schema_version"), "schema_version is required"),
        broken(lambda r: r.update(schema_version=99), "schema_version must match"),
        broken(lambda r: r.update(schema_version=True), "a bool is not a version"),
        broken(lambda r: r.pop("ambiguity_id"), "ambiguity_id is required"),
        broken(lambda r: r.update(ambiguity_id="amb_wrong"),
               "ambiguity_id must equal the recomputed scope|query digest"),
        broken(lambda r: r.update(scope="journal"), "scope must be an object"),
        broken(lambda r: r.update(scope={"kind": "elsewhere"}),
               "scope kind must be journal or facet"),
        broken(lambda r: r.update(scope={"kind": "journal", "facet": "work"}),
               "a journal scope carries no facet"),
        broken(lambda r: r.update(scope={"kind": "facet"}),
               "a facet scope requires a facet"),
        broken(lambda r: r.update(normalized_query=""),
               "normalized_query must be non-empty"),
        broken(lambda r: r.pop("original_query"), "original_query is required"),
        broken(lambda r: r.update(observed_tier=4),
               "observed_tier is one of the low-confidence tiers 5-8"),
    ]


def build_entity_store_fixture() -> dict[str, Any]:
    """Durable bytes and refusal semantics, captured from the real writers."""
    from solstone.think.entities.ambiguities import _save_jsonl_rows, ambiguities_path

    with _temp_journal():
        identity_bytes = _capture_identity_bytes(dict(_ENTITY_FIXED))
        unicode_bytes = _capture_identity_bytes(dict(_ENTITY_UNICODE))
        event_bytes = _capture_event_bytes(dict(_HISTORY_EVENT_FIXED))
        rows = _ambiguity_rows()
        _save_jsonl_rows(ambiguities_path(), rows)
        ambiguities_bytes = ambiguities_path().read_text(encoding="utf-8")

    reconciliation = [
        {**case, "outcome": _resolve_reconciliation(case)}
        for case in _reconciliation_cases()
    ]
    # The table above states its own expected outcome; recomputing it here and
    # asserting agreement is what keeps the prose honest as the code moves.
    for case in reconciliation:
        expected = next(
            c["outcome"] for c in _reconciliation_cases() if c["note"] == case["note"]
        )
        if case["outcome"] != expected:  # pragma: no cover - corpus guard
            raise AssertionError(
                f"reconciliation case drifted: {case['note']!r} "
                f"expected {expected}, computed {case['outcome']}"
            )

    return {
        "generated_by": "make core-fixtures",
        "versions": _versions(),
        "serialization": {
            "note": (
                "Two conventions live one directory apart and both are load "
                "bearing: the identity file preserves insertion order, history "
                "events sort their keys. Both write raw UTF-8 and one trailing "
                "newline, and both land at mode 0600 through an atomic replace."
            ),
            "identity": {
                "flags": "indent=2, ensure_ascii=False, sort_keys absent",
                "mode": "0600",
            },
            "history_event": {
                "flags": "indent=2, ensure_ascii=False, sort_keys=True",
                "mode": "0600",
                "filename": "{seq:020d}-{version_id}.json",
                "filename_note": "zero-padded, so lexical order is chronological order",
            },
            "ambiguities": {
                "flags": "ensure_ascii=False, one compact object per line",
                "empty_file": "an empty row set writes a zero-byte file rather than removing it",
            },
        },
        "timestamps": {
            "created_at": "epoch milliseconds, integer",
            "history_ts": (
                "ISO-8601 UTC ending in Z, VARIABLE WIDTH — the fractional part "
                "is omitted entirely when the microsecond component is zero, so "
                "a reader that requires six digits fails on those records"
            ),
            "ambiguity_ts": "ISO-8601 UTC ending in Z, second precision",
        },
        "artifacts": {
            "entities/{id}/entity.json": identity_bytes,
            "entities/{id}/entity.json (unicode)": unicode_bytes,
            "entities/{id}/history/events/{seq}-{version_id}.json": event_bytes,
            "entities/ambiguities.jsonl": ambiguities_bytes,
        },
        "inputs": {
            "identity": _ENTITY_FIXED,
            "identity_unicode": _ENTITY_UNICODE,
            "history_event": _HISTORY_EVENT_FIXED,
            "ambiguity_rows": rows,
        },
        "reconciliation": {
            "note": (
                "Run before every mutation. Publish when the identity on disk "
                "equals identity_after, discard when it equals identity_before, "
                "otherwise refuse and leave the journal unchanged. The refusal "
                "is permanent for that entity until a human repairs it, and "
                "reads keep working throughout."
            ),
            "cases": reconciliation,
        },
        "negative": {
            "note": (
                "One malformed row per validation rule. A row set is validated "
                "in full before any bytes are written, and both mutation entry "
                "points re-read strictly under the lock first — so one bad row "
                "makes the whole file unwritable while it stays readable."
            ),
            "ambiguity_rows": _negative_cases(),
        },
    }
