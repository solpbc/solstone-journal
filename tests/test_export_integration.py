# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from argparse import Namespace
from importlib import import_module
from pathlib import Path
from urllib.parse import urlparse

import pytest
from flask import Flask

import solstone.convey.state as convey_state
import solstone.think.utils as think_utils
from solstone.observe.export import (
    ExportResult,
    export_config,
    export_entities,
    export_facets,
    export_imports,
    export_segments,
    main,
)
from solstone.think.entities.journal import (
    clear_journal_entity_cache,
    save_journal_entity,
)

journal_sources = import_module("solstone.apps.import.journal_sources")
import_routes = import_module("solstone.apps.import.routes")

create_state_directory = journal_sources.create_state_directory
generate_key = journal_sources.generate_key
get_state_directory = journal_sources.get_state_directory
save_journal_source = journal_sources.save_journal_source
import_bp = import_routes.import_bp


def _set_active_journal(monkeypatch, journal: Path) -> None:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))
    think_utils._journal_path_cache = None
    clear_journal_entity_cache()


def _extract_path(url: str) -> str:
    """Extract path from full URL."""
    return urlparse(url).path


class _FlaskResponse:
    """Wrap Flask test responses to match the requests.Response interface we use."""

    def __init__(self, flask_response):
        self._resp = flask_response
        self.status_code = flask_response.status_code
        self.text = flask_response.get_data(as_text=True)

    def json(self):
        return self._resp.get_json()


class _FlaskSessionAdapter:
    """Wrap a Flask test client to behave like requests.Session for export functions."""

    def __init__(
        self, client, *, source_journal: Path, target_journal: Path, monkeypatch
    ):
        self.client = client
        self.source_journal = source_journal
        self.target_journal = target_journal
        self.monkeypatch = monkeypatch
        self.headers: dict[str, str] = {}

    def get(self, url, **kwargs):
        del kwargs
        _set_active_journal(self.monkeypatch, self.target_journal)
        try:
            resp = self.client.get(_extract_path(url), headers=self.headers)
        finally:
            _set_active_journal(self.monkeypatch, self.source_journal)
        return _FlaskResponse(resp)

    def post(self, url, **kwargs):
        path = _extract_path(url)
        headers = {**self.headers}
        _set_active_journal(self.monkeypatch, self.target_journal)
        try:
            if "files" in kwargs:
                data = {}
                if "data" in kwargs:
                    data.update(kwargs["data"])
                for field_name, file_tuple in kwargs["files"]:
                    if isinstance(file_tuple, tuple):
                        filename, file_obj = file_tuple[0], file_tuple[1]
                        value = (file_obj, filename)
                    else:
                        value = file_tuple
                    if field_name in data:
                        existing = data[field_name]
                        if isinstance(existing, list):
                            existing.append(value)
                        else:
                            data[field_name] = [existing, value]
                    else:
                        data[field_name] = value
                resp = self.client.post(
                    path,
                    data=data,
                    content_type="multipart/form-data",
                    headers=headers,
                )
            elif "json" in kwargs:
                resp = self.client.post(path, json=kwargs["json"], headers=headers)
            else:
                resp = self.client.post(path, headers=headers)
        finally:
            _set_active_journal(self.monkeypatch, self.source_journal)
        return _FlaskResponse(resp)

    def close(self):
        pass


@pytest.fixture
def export_integration_env(tmp_path, monkeypatch):
    """Set up source journal + target Flask app for integration testing."""
    source_journal = tmp_path / "source"
    source_journal.mkdir()
    _set_active_journal(monkeypatch, source_journal)

    target_journal = tmp_path / "target"
    target_journal.mkdir()
    monkeypatch.setattr(
        convey_state, "journal_root", str(target_journal), raising=False
    )
    (target_journal / "apps" / "import" / "journal_sources").mkdir(parents=True)

    key = generate_key()
    source = {
        "name": "integration-test",
        "key": key,
        "created_at": 1000,
        "enabled": True,
        "revoked": False,
        "revoked_at": None,
        "stats": {
            "segments_received": 0,
            "entities_received": 0,
            "facets_received": 0,
            "imports_received": 0,
            "config_received": 0,
        },
    }
    save_journal_source(source)
    key_prefix = key[:8]
    create_state_directory(target_journal, key_prefix)

    app = Flask(__name__)
    app.config["TESTING"] = True
    app.register_blueprint(import_bp)

    client = app.test_client()
    adapter = _FlaskSessionAdapter(
        client,
        source_journal=source_journal,
        target_journal=target_journal,
        monkeypatch=monkeypatch,
    )
    adapter.headers["Authorization"] = f"Bearer {key}"

    yield {
        "source": source_journal,
        "target": target_journal,
        "key": key,
        "key_prefix": key_prefix,
        "client": client,
        "adapter": adapter,
        "base_url": "http://localhost:5000",
        "monkeypatch": monkeypatch,
    }

    think_utils._journal_path_cache = None
    clear_journal_entity_cache()


def _write_bytes(path: Path, content: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)


def _write_json(path: Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def _write_jsonl(path: Path, items: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(item, ensure_ascii=False) + "\n" for item in items),
        encoding="utf-8",
    )


def _load_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def _setup_segments(
    journal_root: Path, *, day: str = "20260413", include_default: bool = False
) -> list[str]:
    chronicle_day = journal_root / "chronicle" / day
    segment_dir = chronicle_day / "laptop" / "143022_300"
    _write_bytes(segment_dir / "audio.flac", b"audio-data")
    _write_bytes(segment_dir / "transcript.jsonl", b'{"text":"hello"}\n')
    if include_default:
        default_segment_dir = chronicle_day / "180000_300"
        _write_bytes(default_segment_dir / "audio.flac", b"default-audio")
    return [day]


def _setup_entities(journal_root: Path) -> None:
    _write_json(
        journal_root / "entities" / "source_entity" / "entity.json",
        {
            "id": "source_entity",
            "name": "Source Entity",
            "type": "Person",
            "created_at": 1000,
        },
    )


def _setup_facet_with_entity(
    journal_root: Path, *, entity_id: str = "source_entity"
) -> None:
    facet_root = journal_root / "facets" / "work"
    _write_json(facet_root / "facet.json", {"title": "Work"})
    _write_json(
        facet_root / "entities" / entity_id / "entity.json",
        {"description": "Relationship"},
    )
    _write_jsonl(
        facet_root / "entities" / entity_id / "observations.jsonl",
        [{"content": "Observed", "observed_at": 1}],
    )
    _write_jsonl(
        facet_root / "entities" / "20260413.jsonl",
        [{"id": entity_id, "name": "Source Entity", "type": "Person"}],
    )
    _write_jsonl(
        facet_root / "todos" / "20260413.jsonl",
        [{"text": "Follow up", "created_at": 1}],
    )


def _setup_simple_facet(journal_root: Path) -> None:
    facet_root = journal_root / "facets" / "personal"
    _write_json(facet_root / "facet.json", {"title": "Personal"})
    (facet_root / "news").mkdir(parents=True, exist_ok=True)
    (facet_root / "news" / "20260413.md").write_text("# News\n", encoding="utf-8")


def _setup_imports(journal_root: Path) -> None:
    import_root = journal_root / "imports" / "20260101_090000"
    _write_json(
        import_root / "import.json", {"original_filename": "cal.zip", "file_size": 100}
    )
    _write_json(
        import_root / "imported.json",
        {"processed_timestamp": "20260101_090000", "total_files_created": 1},
    )
    _write_jsonl(
        import_root / "content_manifest.jsonl",
        [{"id": "event-0", "title": "Test Event"}],
    )


def _setup_config(journal_root: Path) -> None:
    _write_json(
        journal_root / "config" / "journal.json",
        {
            "identity": {"name": "Remote User"},
            "retention": {"days": 30},
            "convey": {
                "allow_network_access": False,
                "trust_localhost": True,
                "secret": "shhh",
            },
            "env": {"API_KEY": "xyz"},
        },
    )


def test_full_export_cycle(export_integration_env):
    env = export_integration_env
    _setup_segments(env["source"])
    _setup_imports(env["source"])
    _setup_entities(env["source"])
    _setup_facet_with_entity(env["source"])
    _setup_config(env["source"])

    segment_result = export_segments(
        env["base_url"], env["key"], ["20260413"], False, session=env["adapter"]
    )
    import_result = export_imports(
        env["base_url"], env["key"], False, session=env["adapter"]
    )
    entity_result = export_entities(
        env["base_url"], env["key"], False, session=env["adapter"]
    )
    facet_result = export_facets(
        env["base_url"], env["key"], False, session=env["adapter"]
    )
    config_result = export_config(
        env["base_url"], env["key"], False, session=env["adapter"]
    )

    assert segment_result == ExportResult(area="segments", sent=1)
    assert import_result == ExportResult(area="imports", sent=1)
    assert entity_result == ExportResult(area="entities", sent=1)
    assert facet_result == ExportResult(area="facets", sent=1)
    assert config_result == ExportResult(area="config", staged=1)

    assert (
        env["target"]
        / "chronicle"
        / "20260413"
        / "laptop"
        / "143022_300"
        / "audio.flac"
    ).exists()
    assert (env["target"] / "entities" / "source_entity" / "entity.json").exists()
    assert (env["target"] / "imports" / "20260101_090000" / "import.json").exists()
    assert (env["target"] / "facets" / "work" / "facet.json").exists()
    assert (
        get_state_directory(env["key_prefix"]) / "config" / "source_config.json"
    ).exists()


def test_idempotent_reexport(export_integration_env):
    env = export_integration_env
    _setup_segments(env["source"])
    _setup_imports(env["source"])
    _setup_entities(env["source"])
    _setup_facet_with_entity(env["source"])
    _setup_config(env["source"])

    export_segments(
        env["base_url"], env["key"], ["20260413"], False, session=env["adapter"]
    )
    export_imports(env["base_url"], env["key"], False, session=env["adapter"])
    export_entities(env["base_url"], env["key"], False, session=env["adapter"])
    export_facets(env["base_url"], env["key"], False, session=env["adapter"])
    export_config(env["base_url"], env["key"], False, session=env["adapter"])

    second_segments = export_segments(
        env["base_url"], env["key"], ["20260413"], False, session=env["adapter"]
    )
    second_imports = export_imports(
        env["base_url"], env["key"], False, session=env["adapter"]
    )
    second_entities = export_entities(
        env["base_url"], env["key"], False, session=env["adapter"]
    )
    second_facets = export_facets(
        env["base_url"], env["key"], False, session=env["adapter"]
    )
    second_config = export_config(
        env["base_url"], env["key"], False, session=env["adapter"]
    )

    assert second_segments.sent == 0 and second_segments.skipped == 1
    assert second_imports.sent == 0 and second_imports.skipped == 1
    assert second_entities.sent == 0 and second_entities.skipped == 1
    assert second_facets.sent == 0 and second_facets.skipped == 1
    assert second_config.sent == 0 and second_config.skipped == 1


def test_idempotent_reexport_default_stream(export_integration_env):
    env = export_integration_env
    _setup_segments(env["source"], include_default=True)

    first = export_segments(
        env["base_url"], env["key"], ["20260413"], False, session=env["adapter"]
    )
    second = export_segments(
        env["base_url"], env["key"], ["20260413"], False, session=env["adapter"]
    )

    assert first.sent == 2
    assert (
        env["target"]
        / "chronicle"
        / "20260413"
        / "_default"
        / "180000_300"
        / "audio.flac"
    ).exists()
    assert second.sent == 0
    assert second.skipped == 2


def test_partial_only_segments(export_integration_env):
    env = export_integration_env
    _setup_segments(env["source"])

    result = export_segments(
        env["base_url"], env["key"], ["20260413"], False, session=env["adapter"]
    )

    assert result.sent == 1
    assert (
        env["target"]
        / "chronicle"
        / "20260413"
        / "laptop"
        / "143022_300"
        / "transcript.jsonl"
    ).exists()


def test_partial_only_entities(export_integration_env):
    env = export_integration_env
    _setup_entities(env["source"])

    result = export_entities(env["base_url"], env["key"], False, session=env["adapter"])

    assert result.sent == 1
    assert (env["target"] / "entities" / "source_entity" / "entity.json").exists()


def test_partial_only_facets(export_integration_env):
    env = export_integration_env
    _setup_simple_facet(env["source"])

    result = export_facets(env["base_url"], env["key"], False, session=env["adapter"])

    assert result.sent == 1
    assert (env["target"] / "facets" / "personal" / "news" / "20260413.md").exists()


def test_partial_only_imports(export_integration_env):
    env = export_integration_env
    _setup_imports(env["source"])

    result = export_imports(env["base_url"], env["key"], False, session=env["adapter"])

    assert result.sent == 1
    assert (env["target"] / "imports" / "20260101_090000" / "imported.json").exists()


def test_partial_only_config(export_integration_env):
    env = export_integration_env
    _setup_config(env["source"])

    result = export_config(env["base_url"], env["key"], False, session=env["adapter"])

    assert result.staged == 1
    assert (get_state_directory(env["key_prefix"]) / "config" / "diff.json").exists()


def test_staged_items_entity_collision(export_integration_env):
    env = export_integration_env
    _write_json(
        env["source"] / "entities" / "test" / "entity.json",
        {"id": "test", "name": "Completely Different Name", "type": "Person"},
    )

    _set_active_journal(env["monkeypatch"], env["target"])
    save_journal_entity({"id": "test", "name": "Test Entity", "type": "Tool"})
    _set_active_journal(env["monkeypatch"], env["source"])

    result = export_entities(env["base_url"], env["key"], False, session=env["adapter"])

    staged_path = (
        get_state_directory(env["key_prefix"]) / "entities" / "staged" / "test.json"
    )
    assert result.sent == 0
    assert result.staged == 1
    assert staged_path.exists()
    assert _load_json(staged_path)["reason"] == "id_collision"


def test_config_always_staged(export_integration_env):
    env = export_integration_env
    _setup_config(env["source"])
    _write_json(
        env["target"] / "config" / "journal.json", {"identity": {"name": "Local User"}}
    )

    result = export_config(env["base_url"], env["key"], False, session=env["adapter"])

    assert result.staged == 1
    assert result.skipped == 0


def test_processing_order_entities_before_facets(export_integration_env):
    env = export_integration_env
    _write_json(
        env["source"] / "entities" / "source_entity" / "entity.json",
        {"id": "source_entity", "name": "Alice Johnson", "type": "Person"},
    )
    _setup_facet_with_entity(env["source"], entity_id="source_entity")

    _set_active_journal(env["monkeypatch"], env["target"])
    save_journal_entity(
        {"id": "target_entity", "name": "Alice Johnson", "type": "Person"}
    )
    _set_active_journal(env["monkeypatch"], env["source"])

    entity_result = export_entities(
        env["base_url"], env["key"], False, session=env["adapter"]
    )
    facet_result = export_facets(
        env["base_url"], env["key"], False, session=env["adapter"]
    )

    entity_state = _load_json(
        get_state_directory(env["key_prefix"]) / "entities" / "state.json"
    )
    detected_entities = (
        env["target"] / "facets" / "work" / "entities" / "20260413.jsonl"
    ).read_text(encoding="utf-8")

    assert entity_result.sent == 1
    assert facet_result.sent == 1
    assert entity_state["id_map"]["source_entity"] == "target_entity"
    assert (
        env["target"] / "facets" / "work" / "entities" / "target_entity" / "entity.json"
    ).exists()
    assert not (
        env["target"] / "facets" / "work" / "entities" / "source_entity"
    ).exists()
    assert '"id": "target_entity"' in detected_entities


def test_error_resilience(monkeypatch, capsys):
    class _DummySession:
        def __init__(self):
            self.headers = {}

        def close(self):
            pass

    calls: list[str] = []

    monkeypatch.setattr(
        "solstone.observe.export.setup_cli",
        lambda parser: Namespace(
            to="https://localhost:5000",
            key="test-key-123456",
            only=None,
            dry_run=False,
            day=None,
        ),
    )
    monkeypatch.setattr(
        "solstone.observe.export._parse_day_spec", lambda day, root: ["20260413"]
    )
    monkeypatch.setattr(
        "solstone.observe.export._query_manifest", lambda session, base_url, key: {}
    )
    monkeypatch.setattr("solstone.observe.export.requests.Session", _DummySession)
    monkeypatch.setattr(
        "solstone.observe.export.export_segments",
        lambda base_url, key, days, dry_run, session=None: (
            calls.append("segments") or ExportResult(area="segments", sent=1)
        ),
    )
    monkeypatch.setattr(
        "solstone.observe.export.export_imports",
        lambda base_url, key, dry_run, session=None: (
            calls.append("imports") or ExportResult(area="imports", sent=1)
        ),
    )

    def _explode(*args, **kwargs):
        calls.append("entities")
        raise RuntimeError("boom")

    monkeypatch.setattr("solstone.observe.export.export_entities", _explode)
    monkeypatch.setattr(
        "solstone.observe.export.export_facets",
        lambda base_url, key, dry_run, session=None: (
            calls.append("facets") or ExportResult(area="facets", sent=1)
        ),
    )
    monkeypatch.setattr(
        "solstone.observe.export.export_config",
        lambda base_url, key, dry_run, session=None: (
            calls.append("config") or ExportResult(area="config", staged=1)
        ),
    )

    with pytest.raises(SystemExit, match="1"):
        main()

    output = capsys.readouterr().out
    assert calls == ["segments", "imports", "entities", "facets", "config"]
    assert "Warning: entity export failed" in output
    assert "entities: FAILED" in output
    assert "segments: 1 sent" in output
    assert "facets: 1 sent" in output


def test_dry_run_full(export_integration_env):
    env = export_integration_env
    _setup_segments(env["source"])
    _setup_imports(env["source"])
    _setup_entities(env["source"])
    _setup_simple_facet(env["source"])
    _setup_config(env["source"])

    segment_result = export_segments(
        env["base_url"], env["key"], ["20260413"], True, session=env["adapter"]
    )
    import_result = export_imports(
        env["base_url"], env["key"], True, session=env["adapter"]
    )
    entity_result = export_entities(
        env["base_url"], env["key"], True, session=env["adapter"]
    )
    facet_result = export_facets(
        env["base_url"], env["key"], True, session=env["adapter"]
    )
    config_result = export_config(
        env["base_url"], env["key"], True, session=env["adapter"]
    )

    assert segment_result.sent == 1
    assert import_result.sent == 1
    assert entity_result.sent == 1
    assert facet_result.sent == 1
    assert config_result.staged == 1
    assert not (env["target"] / "chronicle" / "20260413").exists()
    assert not (env["target"] / "entities" / "source_entity" / "entity.json").exists()
