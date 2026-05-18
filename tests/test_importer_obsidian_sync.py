# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import glob
from pathlib import Path
from textwrap import dedent
from unittest.mock import patch

import pytest


def _write_note(
    vault_dir: Path,
    rel_path: str,
    content: str,
    mtime: float | None = None,
) -> Path:
    """Write a note file to the vault."""
    path = vault_dir / rel_path
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")
    if mtime is not None:
        import os

        os.utime(path, (mtime, mtime))
    return path


SAMPLE_NOTE = dedent("""\
    ---
    tags: [project, alpha]
    ---
    # Alpha Project

    This is a note about the [[Alpha Project]].
    See also [[Bob Smith]] and [[Design Doc]].
""")

SAMPLE_NOTE_2 = dedent("""\
    # Daily Note

    Today I worked on [[Beta Launch]].
""")

UPDATED_NOTE = dedent("""\
    ---
    tags: [project, alpha]
    ---
    # Alpha Project

    This is an updated note about the [[Alpha Project]].
    See also [[Bob Smith]], [[Design Doc]], and [[Launch Plan]].
""")


def test_obsidian_sync_protocol_conformance():
    """ObsidianSyncBackend satisfies SyncableBackend protocol."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend
    from solstone.think.importers.sync import SyncableBackend

    assert isinstance(ObsidianSyncBackend(), SyncableBackend)


def test_obsidian_sync_registry_discovery():
    """Registry discovery includes obsidian."""
    from solstone.think.importers.sync import get_syncable_backends

    backends = get_syncable_backends()
    assert "obsidian" in [backend.name for backend in backends]


def test_obsidian_sync_dry_run(tmp_path):
    """Dry-run catalogs notes and saves state."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend
    from solstone.think.importers.sync import load_sync_state

    vault = tmp_path / "vault"
    _write_note(vault, "Projects/Alpha.md", SAMPLE_NOTE, mtime=1_700_000_000)
    _write_note(vault, "Daily/2026-03-14.md", SAMPLE_NOTE_2, mtime=1_700_000_600)

    result = ObsidianSyncBackend().sync(tmp_path, source_path=vault, dry_run=True)

    assert result["total"] >= 2
    assert result["available"] == 2
    assert result["imported"] == 0
    assert result["downloaded"] == 0

    state = load_sync_state(tmp_path, "obsidian")
    assert state is not None
    assert state["files"]["Projects/Alpha.md"]["status"] == "available"
    assert state["files"]["Daily/2026-03-14.md"]["status"] == "available"


def test_obsidian_sync_import(tmp_path, monkeypatch):
    """Import mode writes note segments and updates state."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend
    from solstone.think.importers.sync import load_sync_state

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    vault = tmp_path / "vault"
    _write_note(vault, "Projects/Alpha.md", SAMPLE_NOTE, mtime=1_700_000_000)

    result = ObsidianSyncBackend().sync(tmp_path, source_path=vault, dry_run=False)

    assert result["downloaded"] >= 1
    assert result["imported"] >= 1

    segments = glob.glob(
        str(
            tmp_path
            / "chronicle"
            / "*"
            / "import.obsidian"
            / "*"
            / "note_transcript.md"
        )
    )
    assert len(segments) >= 1

    state = load_sync_state(tmp_path, "obsidian")
    assert state is not None
    assert state["files"]["Projects/Alpha.md"]["status"] == "imported"


def test_obsidian_sync_edit_creates_new_segments(tmp_path, monkeypatch):
    """Editing a note creates new segments and preserves old ones."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend
    from solstone.think.importers.sync import load_sync_state

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    vault = tmp_path / "vault"
    _write_note(vault, "Projects/Alpha.md", SAMPLE_NOTE, mtime=1_700_000_000)

    backend = ObsidianSyncBackend()
    first = backend.sync(tmp_path, source_path=vault, dry_run=False)
    assert first["downloaded"] == 1
    first_segments = sorted(
        glob.glob(
            str(
                tmp_path
                / "chronicle"
                / "*"
                / "import.obsidian"
                / "*"
                / "note_transcript.md"
            )
        )
    )
    assert len(first_segments) == 1

    _write_note(vault, "Projects/Alpha.md", UPDATED_NOTE, mtime=1_700_000_900)
    second = backend.sync(tmp_path, source_path=vault, dry_run=False)
    assert second["downloaded"] == 1

    all_segments = sorted(
        glob.glob(
            str(
                tmp_path
                / "chronicle"
                / "*"
                / "import.obsidian"
                / "*"
                / "note_transcript.md"
            )
        )
    )
    assert len(all_segments) == 2
    assert first_segments[0] in all_segments

    state = load_sync_state(tmp_path, "obsidian")
    assert state is not None
    assert state["files"]["Projects/Alpha.md"]["edit_count"] >= 2


def test_obsidian_sync_unchanged_skip(tmp_path, monkeypatch):
    """Mtime-only changes are skipped when content hash matches."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    vault = tmp_path / "vault"
    _write_note(vault, "Projects/Alpha.md", SAMPLE_NOTE, mtime=1_700_000_000)

    backend = ObsidianSyncBackend()
    backend.sync(tmp_path, source_path=vault, dry_run=False)
    _write_note(vault, "Projects/Alpha.md", SAMPLE_NOTE, mtime=1_700_000_300)

    result = backend.sync(tmp_path, source_path=vault, dry_run=True)
    assert result["available"] == 0


def test_obsidian_sync_deleted_note(tmp_path, monkeypatch):
    """Deleted notes are marked removed in state."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend
    from solstone.think.importers.sync import load_sync_state

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    vault = tmp_path / "vault"
    note = _write_note(vault, "Projects/Alpha.md", SAMPLE_NOTE, mtime=1_700_000_000)

    backend = ObsidianSyncBackend()
    backend.sync(tmp_path, source_path=vault, dry_run=False)
    note.unlink()
    backend.sync(tmp_path, source_path=vault, dry_run=True)

    state = load_sync_state(tmp_path, "obsidian")
    assert state is not None
    assert state["files"]["Projects/Alpha.md"]["status"] == "removed"


def test_obsidian_sync_force(tmp_path, monkeypatch):
    """Force re-detects notes by clearing state."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    vault = tmp_path / "vault"
    _write_note(vault, "Projects/Alpha.md", SAMPLE_NOTE, mtime=1_700_000_000)

    backend = ObsidianSyncBackend()
    backend.sync(tmp_path, source_path=vault, dry_run=False)
    result = backend.sync(tmp_path, source_path=vault, dry_run=True, force=True)

    assert result["available"] >= 1


def test_obsidian_sync_vault_auto_detection(tmp_path, monkeypatch):
    """Raises when no vault can be auto-detected."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend

    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setattr("solstone.think.importers.obsidian.Path.home", lambda: home)

    with pytest.raises(
        ValueError,
        match="No Obsidian vault found. Use --path to specify your vault location.",
    ):
        ObsidianSyncBackend().sync(tmp_path)


def test_obsidian_sync_entity_seeding(tmp_path, monkeypatch):
    """Wikilinks are converted into Topic entities on import."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    vault = tmp_path / "vault"
    _write_note(vault, "Projects/Alpha.md", SAMPLE_NOTE, mtime=1_700_000_000)

    captured: list[tuple[str, str, list[dict[str, str]]]] = []

    def _fake_seed_entities(
        facet: str,
        day: str,
        entities: list[dict[str, str]],
    ) -> list[dict[str, str]]:
        captured.append((facet, day, entities))
        return entities

    with patch(
        "solstone.think.importers.obsidian.seed_entities",
        side_effect=_fake_seed_entities,
    ):
        ObsidianSyncBackend().sync(tmp_path, source_path=vault, dry_run=False)

    assert len(captured) == 1
    facet, _day, entities = captured[0]
    assert facet == "import.obsidian"
    assert entities == [
        {"name": "Alpha Project", "type": "Topic"},
        {"name": "Bob Smith", "type": "Topic"},
        {"name": "Design Doc", "type": "Topic"},
    ]


def test_obsidian_sync_incremental(tmp_path, monkeypatch):
    """Incremental sync imports only newly added notes."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    vault = tmp_path / "vault"
    _write_note(vault, "Projects/Alpha.md", SAMPLE_NOTE, mtime=1_700_000_000)

    backend = ObsidianSyncBackend()
    first = backend.sync(tmp_path, source_path=vault, dry_run=False)
    assert first["downloaded"] == 1

    _write_note(vault, "Daily/2026-03-14.md", SAMPLE_NOTE_2, mtime=1_700_000_600)
    second = backend.sync(tmp_path, source_path=vault, dry_run=False)

    assert second["downloaded"] == 1
    assert second["available"] == 0
    assert second["imported"] >= 1


def test_infer_entity_type_from_path():
    """Folder path entity type inference."""
    from solstone.think.importers.obsidian import infer_entity_type_from_path

    # Direct folder match
    assert infer_entity_type_from_path("People/Jane Smith.md") == "Person"
    assert infer_entity_type_from_path("Contacts/John.md") == "Person"
    assert infer_entity_type_from_path("Projects/Alpha.md") == "Project"
    assert infer_entity_type_from_path("Companies/Acme.md") == "Organization"
    assert infer_entity_type_from_path("Organizations/UN.md") == "Organization"
    assert infer_entity_type_from_path("Places/Paris.md") == "Place"
    assert infer_entity_type_from_path("Locations/HQ.md") == "Place"

    # Case insensitive
    assert infer_entity_type_from_path("people/jane.md") == "Person"
    assert infer_entity_type_from_path("PEOPLE/Jane.md") == "Person"

    # Any depth in path
    assert infer_entity_type_from_path("00 knowledge/People/Jane Smith.md") == "Person"
    assert infer_entity_type_from_path("Atlas/References/People/Jane.md") == "Person"

    # Numeric prefix stripping
    assert infer_entity_type_from_path("00 People/Jane.md") == "Person"
    assert infer_entity_type_from_path("01 Projects/Alpha.md") == "Project"

    # No match → None
    assert infer_entity_type_from_path("Notes/random.md") is None
    assert infer_entity_type_from_path("Daily/2026-03-14.md") is None
    assert infer_entity_type_from_path("random.md") is None


def test_clean_at_prefix():
    """@ prefix stripping from entity names."""
    from solstone.think.importers.obsidian import _clean_at_prefix

    assert _clean_at_prefix("@JaneSmith") == ("JaneSmith", True)
    assert _clean_at_prefix("@ Jane Smith") == ("Jane Smith", True)
    assert _clean_at_prefix("@") == ("", True)
    assert _clean_at_prefix("Jane Smith") == ("Jane Smith", False)
    assert _clean_at_prefix("") == ("", False)


def test_build_entity_dicts_precedence():
    """Entity type inference precedence: @ > folder-path > Topic."""
    from solstone.think.importers.obsidian import _build_entity_dicts

    # Basic Topic fallback
    result = _build_entity_dicts({"Design Doc"}, {})
    assert result == [{"name": "Design Doc", "type": "Topic"}]

    # Folder-path type
    result = _build_entity_dicts({"Jane Smith"}, {"Jane Smith": "Person"})
    assert result == [{"name": "Jane Smith", "type": "Person"}]

    # @ prefix → Person
    result = _build_entity_dicts({"@Jane Smith"}, {})
    assert result == [{"name": "Jane Smith", "type": "Person"}]

    # @ wins over folder-path
    result = _build_entity_dicts(
        {"@Jane Smith"},
        {"Jane Smith": "Organization"},
    )
    assert result == [{"name": "Jane Smith", "type": "Person"}]

    # @ filename added even without wikilink
    result = _build_entity_dicts(set(), {}, at_filenames={"Jane Smith"})
    assert result == [{"name": "Jane Smith", "type": "Person"}]

    # Dedup: both @Jane and Jane as wikilinks — @ wins
    result = _build_entity_dicts({"@Jane Smith", "Jane Smith"}, {})
    assert result == [{"name": "Jane Smith", "type": "Person"}]


def test_obsidian_sync_folder_path_entity_typing(tmp_path, monkeypatch):
    """Notes in typed folders produce typed entities."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    vault = tmp_path / "vault"

    _write_note(
        vault, "People/Jane Smith.md", "# Jane Smith\nA person.", mtime=1_700_000_000
    )
    _write_note(
        vault,
        "Notes/meeting.md",
        "# Meeting\nMet with [[Jane Smith]] about [[Design Doc]].",
        mtime=1_700_000_100,
    )

    captured: list[tuple[str, str, list[dict[str, str]]]] = []

    def _fake_seed(facet, day, entities):
        captured.append((facet, day, entities))
        return entities

    with patch(
        "solstone.think.importers.obsidian.seed_entities", side_effect=_fake_seed
    ):
        ObsidianSyncBackend().sync(tmp_path, source_path=vault, dry_run=False)

    all_entities = {}
    for _, _, entities in captured:
        for e in entities:
            all_entities[e["name"]] = e["type"]

    assert all_entities["Jane Smith"] == "Person"
    assert all_entities["Design Doc"] == "Topic"


def test_obsidian_sync_at_prefix_entity_typing(tmp_path, monkeypatch):
    """Wikilinks with @ prefix produce Person entities."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    vault = tmp_path / "vault"

    _write_note(
        vault,
        "Notes/meeting.md",
        "# Meeting\nMet with [[@Bob Jones]] and [[Design Doc]].",
        mtime=1_700_000_000,
    )

    captured: list[tuple[str, str, list[dict[str, str]]]] = []

    def _fake_seed(facet, day, entities):
        captured.append((facet, day, entities))
        return entities

    with patch(
        "solstone.think.importers.obsidian.seed_entities", side_effect=_fake_seed
    ):
        ObsidianSyncBackend().sync(tmp_path, source_path=vault, dry_run=False)

    all_entities = {}
    for _, _, entities in captured:
        for e in entities:
            all_entities[e["name"]] = e["type"]

    assert all_entities["Bob Jones"] == "Person"
    assert all_entities["Design Doc"] == "Topic"


def test_obsidian_sync_numeric_prefix_folder(tmp_path, monkeypatch):
    """Numeric-prefixed folder names are matched after stripping."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    vault = tmp_path / "vault"

    _write_note(vault, "00 People/Jane.md", "# Jane\nA person.", mtime=1_700_000_000)
    _write_note(
        vault,
        "Notes/ref.md",
        "# Ref\nSee [[Jane]].",
        mtime=1_700_000_100,
    )

    captured: list[tuple[str, str, list[dict[str, str]]]] = []

    def _fake_seed(facet, day, entities):
        captured.append((facet, day, entities))
        return entities

    with patch(
        "solstone.think.importers.obsidian.seed_entities", side_effect=_fake_seed
    ):
        ObsidianSyncBackend().sync(tmp_path, source_path=vault, dry_run=False)

    all_entities = {}
    for _, _, entities in captured:
        for e in entities:
            all_entities[e["name"]] = e["type"]

    assert all_entities["Jane"] == "Person"


def test_obsidian_walk_excludes_templates(tmp_path):
    """templates/ and _templates/ folders are excluded from file walking."""
    from solstone.think.importers.obsidian import _walk_md_files

    vault = tmp_path / "vault"
    _write_note(vault, "Notes/real.md", "# Real note", mtime=1_700_000_000)
    _write_note(vault, "templates/daily.md", "# Template", mtime=1_700_000_000)
    _write_note(vault, "_templates/weekly.md", "# Template", mtime=1_700_000_000)

    files = _walk_md_files(vault)
    rel_paths = [str(f.relative_to(vault)) for f in files]

    assert "Notes/real.md" in rel_paths
    assert "templates/daily.md" not in rel_paths
    assert "_templates/weekly.md" not in rel_paths


def test_obsidian_walk_excludes_templates_case_insensitive(tmp_path):
    """Template folder exclusion is case-insensitive."""
    from solstone.think.importers.obsidian import _walk_md_files

    vault = tmp_path / "vault"
    _write_note(vault, "Notes/real.md", "# Real note", mtime=1_700_000_000)
    _write_note(vault, "Templates/daily.md", "# Template", mtime=1_700_000_000)
    _write_note(vault, "TEMPLATES/weekly.md", "# Template", mtime=1_700_000_000)
    _write_note(vault, "_Templates/meeting.md", "# Template", mtime=1_700_000_000)

    files = _walk_md_files(vault)
    rel_paths = [str(f.relative_to(vault)) for f in files]

    assert "Notes/real.md" in rel_paths
    assert "Templates/daily.md" not in rel_paths
    assert "TEMPLATES/weekly.md" not in rel_paths
    assert "_Templates/meeting.md" not in rel_paths


def test_obsidian_walk_excludes_hidden_dirs(tmp_path):
    """Hidden dirs (.obsidian, .trash) are excluded by _is_hidden."""
    from solstone.think.importers.obsidian import _walk_md_files

    vault = tmp_path / "vault"
    _write_note(vault, "Notes/real.md", "# Real note", mtime=1_700_000_000)
    _write_note(vault, ".obsidian/plugins/list.md", "# Plugin", mtime=1_700_000_000)
    _write_note(vault, ".trash/deleted.md", "# Deleted", mtime=1_700_000_000)

    files = _walk_md_files(vault)
    rel_paths = [str(f.relative_to(vault)) for f in files]

    assert "Notes/real.md" in rel_paths
    assert ".obsidian/plugins/list.md" not in rel_paths
    assert ".trash/deleted.md" not in rel_paths


def test_obsidian_walk_excludes_nested_templates(tmp_path):
    """Templates folders nested inside other folders are also excluded."""
    from solstone.think.importers.obsidian import _walk_md_files

    vault = tmp_path / "vault"
    _write_note(vault, "Notes/real.md", "# Real note", mtime=1_700_000_000)
    _write_note(
        vault, "Areas/Work/templates/standup.md", "# Template", mtime=1_700_000_000
    )

    files = _walk_md_files(vault)
    rel_paths = [str(f.relative_to(vault)) for f in files]

    assert "Notes/real.md" in rel_paths
    assert "Areas/Work/templates/standup.md" not in rel_paths


def test_obsidian_sync_excludes_templates(tmp_path, monkeypatch):
    """Incremental sync skips template folders."""
    from solstone.think.importers.obsidian import ObsidianSyncBackend
    from solstone.think.importers.sync import load_sync_state

    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    vault = tmp_path / "vault"
    _write_note(vault, "Notes/real.md", SAMPLE_NOTE, mtime=1_700_000_000)
    _write_note(
        vault, "templates/daily.md", "# Daily Template\n{{date}}", mtime=1_700_000_000
    )
    _write_note(
        vault,
        "Templates/weekly.md",
        "# Weekly Template\n{{date}}",
        mtime=1_700_000_000,
    )

    result = ObsidianSyncBackend().sync(tmp_path, source_path=vault, dry_run=True)

    assert result["available"] == 1

    state = load_sync_state(tmp_path, "obsidian")
    assert "Notes/real.md" in state["files"]
    assert "templates/daily.md" not in state["files"]
    assert "Templates/weekly.md" not in state["files"]


def test_obsidian_backends_cli_flag(capsys, monkeypatch, tmp_path):
    """sol import --backends lists obsidian."""
    import sys

    from solstone.think.importers.cli import main

    monkeypatch.setattr(sys, "argv", ["sol import", "--backends"])
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path / "journal"))

    main()
    captured = capsys.readouterr()
    assert "obsidian" in captured.out
