# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for mutation-safe entity resolution ambiguities."""

from __future__ import annotations

import ast
import hashlib
import inspect
import json
from contextlib import contextmanager
from importlib import import_module
from pathlib import Path
from typing import Iterator

import pytest

import solstone.think.entities.ambiguities as ambiguities
import solstone.think.entities.history as entity_history
from solstone.think.entities import (
    EntityAmbiguityError,
    EntityResolutionOutcome,
    MatchTier,
    ResolutionOrigin,
    ResolutionScope,
    delete_journal_entity,
    find_matching_entity,
    load_ambiguities,
    record_ambiguity_choice,
    record_entity_resolution,
)
from solstone.think.journal_io import LockTimeout


@pytest.fixture
def journal(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    """Use an isolated journal for ambiguity tests."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    return tmp_path


def _entity(
    name: str,
    entity_id: str | None = None,
    *,
    aka: list[str] | None = None,
    emails: list[str] | None = None,
    blocked: bool = False,
) -> dict:
    entity = {"id": entity_id or name.lower().replace(" ", "_"), "name": name}
    if aka is not None:
        entity["aka"] = aka
    if emails is not None:
        entity["emails"] = emails
    if blocked:
        entity["blocked"] = True
    return entity


def _origin(lane: str = "test", invocation_id: str = "run-1") -> ResolutionOrigin:
    return ResolutionOrigin(lane=lane, field="entity", invocation_id=invocation_id)


def _ambiguity_file(journal: Path) -> Path:
    return journal / "entities" / "ambiguities.jsonl"


def _tree_snapshot(root: Path) -> dict[str, str]:
    snapshot: dict[str, str] = {}
    for path in sorted(root.rglob("*")):
        if path.name.endswith(".lock") or path.is_dir():
            continue
        rel = path.relative_to(root).as_posix()
        if path.is_file():
            snapshot[rel] = hashlib.sha256(path.read_bytes()).hexdigest()
    return snapshot


def _record(
    query: str,
    entities: list[dict],
    *,
    scope: ResolutionScope | None = None,
    origin: ResolutionOrigin | None = None,
):
    return record_entity_resolution(
        query,
        entities,
        scope=scope or ResolutionScope.journal(),
        origin=origin or _origin(),
    )


def test_high_confidence_tiers_resolve_without_creating_ambiguities(
    journal: Path,
) -> None:
    cases = [
        ("Alice Smith", [_entity("Alice Smith")], MatchTier.EXACT),
        ("alice smith", [_entity("Alice Smith")], MatchTier.CASE_INSENSITIVE),
        (
            "alice@example.com",
            [_entity("Alice Smith", emails=["alice@example.com"])],
            MatchTier.EMAIL,
        ),
        (
            "Alice Smith",
            [_entity("Alice Jones", entity_id="alice_smith")],
            MatchTier.SLUG,
        ),
    ]

    for query, entities, expected_tier in cases:
        resolution = _record(query, entities, origin=_origin(invocation_id=query))
        assert resolution.outcome == EntityResolutionOutcome.RESOLVED
        assert resolution.entity is not None
        assert resolution.tier == expected_tier
        assert resolution.ambiguity_id is None

    assert not _ambiguity_file(journal).exists()


def test_tier_five_multi_candidate_legacy_none_becomes_ambiguous(
    journal: Path,
) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]

    assert find_matching_entity("Sarah", entities) is None
    resolution = _record("Sarah", entities)

    assert resolution.outcome == EntityResolutionOutcome.AMBIGUOUS
    assert resolution.entity is None
    assert resolution.tier == MatchTier.FIRST_WORD
    assert resolution.ambiguity_id
    assert {candidate.id for candidate in resolution.candidates} == {
        "sarah_connor",
        "sarah_lee",
    }
    assert list(resolution.candidates) == sorted(
        resolution.candidates,
        key=lambda candidate: (-candidate.score, candidate.name, candidate.id),
    )

    row = load_ambiguities()[0]
    assert row["observed_tier"] == int(MatchTier.FIRST_WORD)
    assert len(row["ranked_candidates"]) == 2


def test_record_entity_resolution_read_only_skips_trust_lock_and_ambiguity_observation(
    journal: Path,
) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    trust_lock = journal / "health" / "locks" / "entity-trust.lock"

    resolution = record_entity_resolution(
        "Sarah",
        entities,
        scope=ResolutionScope.journal(),
        origin=_origin(),
        read_only=True,
    )

    assert resolution.outcome == EntityResolutionOutcome.AMBIGUOUS
    assert resolution.ambiguity_id == ""
    assert {candidate.id for candidate in resolution.candidates} == {
        "sarah_connor",
        "sarah_lee",
    }
    assert not _ambiguity_file(journal).exists()
    assert not trust_lock.exists()


def test_tier_six_multi_candidate_legacy_none_becomes_ambiguous(
    journal: Path,
) -> None:
    entities = [_entity("Josh Jones Dilworth"), _entity("Mary Jones Dilworth")]

    assert find_matching_entity("Jones Dilworth", entities) is None
    resolution = _record("Jones Dilworth", entities)

    assert resolution.outcome == EntityResolutionOutcome.AMBIGUOUS
    assert resolution.entity is None
    assert resolution.tier == MatchTier.TOKEN_SUBSET
    assert {candidate.id for candidate in resolution.candidates} == {
        "josh_jones_dilworth",
        "mary_jones_dilworth",
    }


def test_tier_seven_multi_candidate_legacy_none_becomes_ambiguous(
    journal: Path,
) -> None:
    entities = [_entity("Jonathan Dilton"), _entity("Jonas Diltmore")]

    assert find_matching_entity("Jona Dilt", entities) is None
    resolution = _record("Jona Dilt", entities)

    assert resolution.outcome == EntityResolutionOutcome.AMBIGUOUS
    assert resolution.entity is None
    assert resolution.tier == MatchTier.PREFIX
    assert {candidate.id for candidate in resolution.candidates} == {
        "jonathan_dilton",
        "jonas_diltmore",
    }


def test_tier_eight_fuzzy_is_ambiguous(journal: Path) -> None:
    entities = [_entity("Robert Johnson")]

    resolution = _record("Robert Jonson", entities)

    assert resolution.outcome == EntityResolutionOutcome.AMBIGUOUS
    assert resolution.entity is None
    assert resolution.tier == MatchTier.FUZZY
    assert [candidate.id for candidate in resolution.candidates] == ["robert_johnson"]


def test_true_no_match_does_not_create_row(journal: Path) -> None:
    resolution = _record("Xy", [_entity("Alice Smith")])

    assert resolution.outcome == EntityResolutionOutcome.NO_MATCH
    assert resolution.entity is None
    assert resolution.ambiguity_id is None
    assert not _ambiguity_file(journal).exists()


def test_one_open_row_per_scope_query_with_deduplicated_origins(
    journal: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    times = iter(
        [
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:01Z",
            "2026-01-01T00:00:02Z",
            "2026-01-01T00:00:03Z",
        ]
    )
    monkeypatch.setattr(ambiguities, "utc_now_iso", lambda: next(times))
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    origin1 = _origin("participation", "run-1")
    origin2 = _origin("participation", "run-2")
    origin3 = _origin("schedule", "run-3")

    first = _record("Sarah", entities, origin=origin1)
    _record("  sarah  ", entities, origin=origin1)
    second = _record("SARAH", entities, origin=origin2)
    third = _record("Sarah", entities, origin=origin3)

    rows = load_ambiguities()
    assert len(rows) == 1
    row = rows[0]
    assert first.ambiguity_id == second.ambiguity_id == third.ambiguity_id
    assert row["first_seen"] == "2026-01-01T00:00:00Z"
    assert row["last_seen"] == "2026-01-01T00:00:03Z"
    assert row["occurrence_count"] == 3
    assert len(row["origins"]) == 3


def test_normalization_collapses_case_whitespace_and_unicode_not_punctuation(
    journal: Path,
) -> None:
    assert ambiguities.normalize_resolution_query("  Sarah\tLee  ") == "sarah lee"
    assert ambiguities.normalize_resolution_query("Ｓａｒａｈ") == "sarah"
    assert ambiguities.normalize_resolution_query("Sarah.") == "sarah."

    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    punct_entities = [_entity("Sarah. Connor"), _entity("Sarah. Lee")]

    first = _record("  Sarah  ", entities, origin=_origin("test", "run-1"))
    second = _record("sarah", entities, origin=_origin("test", "run-2"))
    third = _record("Sarah.", punct_entities, origin=_origin("test", "run-3"))

    rows = load_ambiguities()
    assert len(rows) == 2
    assert first.ambiguity_id == second.ambiguity_id
    assert third.ambiguity_id != first.ambiguity_id


def test_scope_discriminator_separates_facet_and_journal(journal: Path) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]

    journal_resolution = _record("Sarah", entities, scope=ResolutionScope.journal())
    facet_resolution = _record(
        "Sarah",
        entities,
        scope=ResolutionScope.facet_scope("work"),
        origin=_origin("test", "facet-run"),
    )

    rows = load_ambiguities()
    assert len(rows) == 2
    assert journal_resolution.ambiguity_id != facet_resolution.ambiguity_id
    assert {json.dumps(row["scope"], sort_keys=True) for row in rows} == {
        json.dumps({"kind": "journal"}, sort_keys=True),
        json.dumps({"kind": "facet", "facet": "work"}, sort_keys=True),
    }


def test_record_ambiguity_choice_returns_choice_without_reopening(
    journal: Path,
) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    scope = ResolutionScope.journal()

    ambiguous = _record("Sarah", entities, scope=scope)
    row = record_ambiguity_choice("Sarah", "sarah_lee", entities, scope=scope)

    assert row["ambiguity_id"] == ambiguous.ambiguity_id
    assert row["status"] == "resolved"
    assert row["resolved_entity_id"] == "sarah_lee"

    resolved = _record("Sarah", entities, scope=scope, origin=_origin("test", "run-2"))
    rows = load_ambiguities()
    assert resolved.outcome == EntityResolutionOutcome.RESOLVED
    assert resolved.entity is not None
    assert resolved.entity["id"] == "sarah_lee"
    assert rows[0]["occurrence_count"] == 1
    assert rows[0]["status"] == "resolved"


def test_invalid_ambiguity_choice_fails_with_row_byte_unchanged(
    journal: Path,
) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    scope = ResolutionScope.journal()
    _record("Sarah", entities, scope=scope)
    path = _ambiguity_file(journal)
    before = path.read_bytes()

    with pytest.raises(EntityAmbiguityError):
        record_ambiguity_choice("Sarah", "missing", entities, scope=scope)
    assert path.read_bytes() == before

    with pytest.raises(EntityAmbiguityError):
        record_ambiguity_choice(
            "Sarah",
            "blocked_sarah",
            [*entities, _entity("Blocked Sarah", "blocked_sarah", blocked=True)],
            scope=scope,
        )
    assert path.read_bytes() == before

    with pytest.raises(EntityAmbiguityError):
        record_ambiguity_choice(
            "Sarah",
            "sarah_lee",
            entities,
            scope=ResolutionScope.facet_scope("work"),
        )
    assert path.read_bytes() == before


def test_reresolve_retains_prior_choice_in_audit(journal: Path) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    scope = ResolutionScope.journal()

    _record("Sarah", entities, scope=scope)
    record_ambiguity_choice("Sarah", "sarah_connor", entities, scope=scope)
    row = record_ambiguity_choice(
        "Sarah",
        "sarah_lee",
        entities,
        scope=scope,
        origin=_origin("repair", "repair-run"),
    )

    assert row["resolved_entity_id"] == "sarah_lee"
    prior_choices = row["audit"]["prior_choices"]
    assert len(prior_choices) == 1
    assert prior_choices[0]["resolved_entity_id"] == "sarah_connor"
    assert prior_choices[0]["replaced_by_origin"]["lane"] == "repair"


def test_stale_resolved_choice_fails_loudly_without_fallback(journal: Path) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    scope = ResolutionScope.journal()

    _record("Sarah", entities, scope=scope)
    record_ambiguity_choice("Sarah", "sarah_lee", entities, scope=scope)

    with pytest.raises(EntityAmbiguityError):
        _record("Sarah", [_entity("Sarah Connor")], scope=scope)

    with pytest.raises(EntityAmbiguityError):
        _record(
            "Sarah",
            [_entity("Sarah Lee", entity_id="sarah_lee", blocked=True)],
            scope=scope,
        )


def test_resolved_choice_with_empty_entity_set_fails_loudly(
    journal: Path,
) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    scope = ResolutionScope.journal()

    _record("Sarah", entities, scope=scope)
    record_ambiguity_choice("Sarah", "sarah_lee", entities, scope=scope)

    with pytest.raises(EntityAmbiguityError):
        resolution = _record("Sarah", [], scope=scope)
        assert resolution.outcome != EntityResolutionOutcome.NO_MATCH

    assert not (journal / "entities" / "sarah" / "entity.json").exists()


def test_lock_timeout_during_persistence_fails_closed(
    journal: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def raise_timeout(path: Path):
        raise LockTimeout(path, 0.01)

    monkeypatch.setattr(ambiguities, "hold_lock", raise_timeout)
    before = _tree_snapshot(journal)

    with pytest.raises(LockTimeout):
        _record("Sarah", [_entity("Sarah Connor"), _entity("Sarah Lee")])

    assert _tree_snapshot(journal) == before
    assert not _ambiguity_file(journal).exists()


def test_resolution_and_choice_use_trust_then_ambiguity_lock_order(
    journal: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    scope = ResolutionScope.journal()
    events: list[str] = []

    @contextmanager
    def trust_lock(path: Path) -> Iterator[None]:
        events.append("trust-enter")
        try:
            yield
        finally:
            events.append("trust-exit")

    @contextmanager
    def ambiguity_lock(path: Path) -> Iterator[None]:
        events.append("ambiguity-enter")
        try:
            yield
        finally:
            events.append("ambiguity-exit")

    monkeypatch.setattr(entity_history, "hold_lock", trust_lock)
    monkeypatch.setattr(ambiguities, "hold_lock", ambiguity_lock)

    _record("Sarah", entities, scope=scope)
    assert events == [
        "trust-enter",
        "ambiguity-enter",
        "ambiguity-exit",
        "trust-exit",
    ]

    events.clear()
    record_ambiguity_choice("Sarah", "sarah_lee", entities, scope=scope)
    assert events == [
        "trust-enter",
        "ambiguity-enter",
        "ambiguity-exit",
        "trust-exit",
    ]


def test_entity_delete_uses_trust_boundary(
    journal: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entity_dir = journal / "entities" / "sarah_lee"
    entity_dir.mkdir(parents=True)
    (entity_dir / "entity.json").write_text(
        json.dumps(_entity("Sarah Lee")),
        encoding="utf-8",
    )
    events: list[str] = []

    @contextmanager
    def trust_lock(path: Path) -> Iterator[None]:
        events.append("trust-enter")
        try:
            yield
        finally:
            events.append("trust-exit")

    monkeypatch.setattr(entity_history, "hold_lock", trust_lock)

    result = delete_journal_entity("sarah_lee")

    assert result == {"success": True, "facets_deleted": []}
    assert events == ["trust-enter", "trust-exit"]
    assert not entity_dir.exists()


@pytest.mark.parametrize(
    "corrupt_line",
    [
        "{not-json",
        "[]",
        json.dumps({"schema_version": 1}),
    ],
)
@pytest.mark.parametrize("position", ["before", "after", "only"])
def test_mutation_resolution_fails_closed_on_corrupt_store_rows(
    journal: Path,
    corrupt_line: str,
    position: str,
) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    _record("Sarah", entities)
    path = _ambiguity_file(journal)
    valid = path.read_text(encoding="utf-8").rstrip("\n")
    if position == "before":
        content = f"{corrupt_line}\n{valid}\n"
    elif position == "after":
        content = f"{valid}\n{corrupt_line}\n"
    else:
        content = f"{corrupt_line}\n"
    path.write_text(content, encoding="utf-8")
    before = path.read_bytes()

    with pytest.raises(EntityAmbiguityError):
        _record("Sarah", entities, origin=_origin("retry", "retry-1"))

    assert path.read_bytes() == before


def test_read_only_list_remains_lenient_for_malformed_lines(journal: Path) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    _record("Sarah", entities)
    path = _ambiguity_file(journal)
    valid = path.read_text(encoding="utf-8")
    path.write_text(f"{{not-json\n[]\n{valid}", encoding="utf-8")
    before = path.read_bytes()

    rows = load_ambiguities()

    assert len(rows) == 1
    assert rows[0]["normalized_query"] == "sarah"
    assert path.read_bytes() == before


def test_serialization_failure_is_typed_and_preserves_store(
    journal: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    scope = ResolutionScope.journal()
    _record("Sarah", entities, scope=scope)
    path = _ambiguity_file(journal)
    before = path.read_bytes()

    def fail_serialization(*args, **kwargs):
        raise TypeError("injected serialization failure")

    monkeypatch.setattr(ambiguities.json, "dumps", fail_serialization)

    with pytest.raises(EntityAmbiguityError, match="cannot write"):
        record_ambiguity_choice("Sarah", "sarah_lee", entities, scope=scope)

    assert path.read_bytes() == before


def test_strict_choice_read_oserror_is_typed_and_preserves_store(
    journal: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    path = _ambiguity_file(journal)
    path.parent.mkdir(parents=True)
    path.write_text("sentinel\n", encoding="utf-8")
    before = path.read_bytes()

    def fail_read(*args, **kwargs):
        raise PermissionError("injected read failure")

    monkeypatch.setattr(ambiguities, "open", fail_read, raising=False)

    with pytest.raises(EntityAmbiguityError, match="cannot read"):
        _record("Sarah", entities)

    assert path.read_bytes() == before


def test_trust_lock_timeout_fails_before_ambiguity_mutation(
    journal: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def raise_timeout(path: Path):
        raise LockTimeout(path, 0.01)

    monkeypatch.setattr(entity_history, "hold_lock", raise_timeout)
    before = _tree_snapshot(journal)

    with pytest.raises(LockTimeout):
        _record("Sarah", [_entity("Sarah Connor"), _entity("Sarah Lee")])

    assert _tree_snapshot(journal) == before
    assert not _ambiguity_file(journal).exists()


def test_per_invocation_memoization_keeps_occurrence_count_one(
    journal: Path,
) -> None:
    entities = [_entity("Sarah Connor"), _entity("Sarah Lee")]
    origin = _origin("participation", "one-talent-run")

    for _ in range(5):
        resolution = _record("Sarah", entities, origin=origin)
        assert resolution.outcome == EntityResolutionOutcome.AMBIGUOUS

    rows = load_ambiguities()
    assert len(rows) == 1
    assert rows[0]["occurrence_count"] == 1
    assert len(rows[0]["origins"]) == 1


def _function_calls_record_entity_resolution(
    module_name: str, function_name: str
) -> bool:
    module = import_module(module_name)
    tree = ast.parse(inspect.getsource(module))
    for node in ast.walk(tree):
        if not isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        if node.name != function_name:
            continue
        for child in ast.walk(node):
            if isinstance(child, ast.Call):
                func = child.func
                if isinstance(func, ast.Name) and func.id == "record_entity_resolution":
                    return True
                if (
                    isinstance(func, ast.Attribute)
                    and func.attr == "record_entity_resolution"
                ):
                    return True
    return False


def test_audited_mutation_owners_use_record_entity_resolution() -> None:
    owners = [
        ("solstone.talent.participation", "post_process"),
        ("solstone.talent.schedule", "post_process"),
        ("solstone.talent.story", "_resolve_entity_id"),
        ("solstone.talent.speaker_attribution", "post_process"),
        ("solstone.apps.activities.routes", "_resolve_participation_entity_ids"),
        ("solstone.apps.entities.routes", "detect_entity_route"),
        ("solstone.apps.entities.talent.entity_observer", "_clean_relation"),
        ("solstone.apps.import.ingest", "ingest_entities"),
        ("solstone.apps.speakers.attribution", "attribute_segment"),
        ("solstone.apps.speakers.bootstrap", "merge_names"),
        ("solstone.think.entities.seeding", "seed_entities"),
        ("solstone.think.merge", "_merge_entities"),
    ]
    # The native bootstrap-voiceprints and seed-from-imports verbs call the
    # Rust record_entity_resolution equivalent, including ambiguity recording.

    missing = [
        f"{module_name}:{function_name}"
        for module_name, function_name in owners
        if not _function_calls_record_entity_resolution(module_name, function_name)
    ]
    assert missing == []
