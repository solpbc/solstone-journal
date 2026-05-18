# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for app agent discovery, loading, and route helpers."""

import json
from pathlib import Path

import pytest

from solstone.apps.sol.routes import _resolve_output_path
from solstone.think.talent import _resolve_talent_path, get_talent, get_talent_configs


@pytest.fixture
def fixture_journal(monkeypatch):
    """Set SOLSTONE_JOURNAL to tests/fixtures/journal for testing."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", "tests/fixtures/journal")
    yield


@pytest.fixture
def app_with_agent(tmp_path, monkeypatch):
    """Create a temporary app with an agent for testing.

    Creates apps/testapp/talent/myhelper.md with frontmatter in a temp directory,
    then monkeypatches the apps directory path.
    """
    # Create app structure
    app_dir = tmp_path / "apps" / "testapp"
    talent_dir = app_dir / "talent"
    talent_dir.mkdir(parents=True)

    # Create workspace.html (required for app discovery, though not used here)
    (app_dir / "workspace.html").write_text("<h1>Test App</h1>")

    # Create agent file with frontmatter
    metadata = {
        "type": "cogitate",
        "title": "My Test Helper",
        "provider": "openai",
        "tools": "journal",
        "schedule": "daily",
        "priority": 42,
    }
    json_str = json.dumps(metadata, indent=2)
    (talent_dir / "myhelper.md").write_text(
        f"{{\n{json_str[1:-1]}\n}}\n\nYou are a test helper agent.\n\n## Purpose\nHelp with testing."
    )

    # Create another agent without metadata (defaults only)
    (talent_dir / "simple.md").write_text("A simple test agent with no metadata.")

    # Monkeypatch the parent directory so apps discovery finds our temp apps
    monkeypatch.setattr(
        "solstone.think.utils.Path.__file__",
        str(tmp_path / "think" / "utils.py"),
    )

    # Actually we need to patch where get_agents looks for apps
    # It uses the package root / "apps"
    # Let's patch it differently - create a mock apps dir structure
    yield {
        "tmp_path": tmp_path,
        "app_dir": app_dir,
        "talent_dir": talent_dir,
    }


def test_resolve_agent_path_system_agent():
    """Test _resolve_talent_path returns correct path for system agents."""
    agent_dir, agent_name = _resolve_talent_path("chat")

    assert agent_name == "chat"
    assert agent_dir.name == "talent"


def test_resolve_agent_path_app_agent():
    """Test _resolve_talent_path returns correct path for app agents."""
    agent_dir, agent_name = _resolve_talent_path("support:support")

    assert agent_name == "support"
    assert agent_dir.name == "talent"
    assert agent_dir.parent.name == "support"
    assert "apps" in str(agent_dir)


def test_resolve_agent_path_app_agent_with_underscores():
    """Test _resolve_talent_path handles app names with underscores."""
    agent_dir, agent_name = _resolve_talent_path("my_app:my_agent")

    assert agent_name == "my_agent"
    assert agent_dir.parent.name == "my_app"


def test_get_agent_system_agent(fixture_journal):
    """Test get_talent loads system agents correctly."""
    config = get_talent("chat")

    assert config["name"] == "chat"
    assert "user_instruction" in config
    assert len(config["user_instruction"]) > 0


def test_get_agent_nonexistent_raises():
    """Test get_talent raises FileNotFoundError for nonexistent agents."""
    with pytest.raises(FileNotFoundError) as exc_info:
        get_talent("nonexistent_agent_xyz")

    assert "nonexistent_agent_xyz" in str(exc_info.value)


def test_get_agent_legacy_alias_raises():
    """The legacy chat alias is removed in the chat backend cutover."""
    with pytest.raises(FileNotFoundError):
        get_talent("uni" + "fied")


def test_get_agent_nonexistent_app_agent_raises():
    """Test get_talent raises FileNotFoundError for nonexistent app agents."""
    with pytest.raises(FileNotFoundError) as exc_info:
        get_talent("fakeapp:fakeagent")

    assert "fakeapp:fakeagent" in str(exc_info.value)


def test_get_talent_configs_includes_system_agents(fixture_journal):
    """Test get_talent_configs returns system agents with metadata."""
    agents = get_talent_configs(type="cogitate")

    # Should include known system agents with frontmatter metadata
    assert "exec" in agents
    assert agents["exec"]["source"] == "system"
    assert "title" in agents["exec"]
    assert "path" in agents["exec"]


def test_get_talent_configs_system_agents_have_metadata(fixture_journal):
    """Test system agents have proper metadata fields."""
    agents = get_talent_configs(type="cogitate")

    # Check a known system agent
    exec_talent = agents.get("exec")
    assert exec_talent is not None
    assert exec_talent["source"] == "system"
    assert "title" in exec_talent
    assert "color" in exec_talent


def test_digest_talent_discovery_and_schedule_exclusion(fixture_journal):
    agents = get_talent_configs(type="cogitate")

    assert "digest" in agents
    assert agents["digest"]["schedule"] == "none"

    for schedule in ("daily", "segment", "activity", "weekly"):
        assert "digest" not in get_talent_configs(type="cogitate", schedule=schedule)


def test_get_talent_configs_excludes_private_apps(
    fixture_journal, tmp_path, monkeypatch
):
    """Test get_talent_configs skips apps starting with underscore."""
    # Create a private app with an agent
    private_app = tmp_path / "_private_app" / "talents"
    private_app.mkdir(parents=True)
    (private_app / "secret.md").write_text("Secret agent")

    # This is tricky to test without modifying the actual apps directory
    # The current implementation filters by app_path.name.startswith("_")
    # We verify this by checking the code behavior with get_talent_configs()

    agents = get_talent_configs(type="cogitate")

    # No agents should have keys starting with "_"
    for key in agents:
        assert not key.startswith("_"), f"Private app agent found: {key}"


def test_app_agent_namespace_format(fixture_journal):
    """Test app agent keys follow {app}:{agent} format."""
    agents = get_talent_configs(type="cogitate")

    for key, config in agents.items():
        if config.get("source") == "app":
            # App agents must have colon in key
            assert ":" in key, f"App agent key missing namespace: {key}"
            app_name, agent_name = key.split(":", 1)
            assert config.get("app") == app_name


# --- _resolve_output_path tests ---


class TestResolveOutputPath:
    """Tests for _resolve_output_path route helper."""

    def test_explicit_output_path_returned_directly(self):
        """When output_path is set, return it as-is without derivation."""
        event = {
            "output_path": "/journal/facets/work/activities/20260214/coding_100/summary.md"
        }
        result = _resolve_output_path(event, "/journal")
        assert result == Path(
            "/journal/facets/work/activities/20260214/coding_100/summary.md"
        )

    def test_derives_path_from_request_fields(self, fixture_journal):
        """Without output_path, derives from day/name/segment fields."""
        event = {
            "day": "20260214",
            "name": "chat",
            "segment": "100",
            "facet": "health",
        }
        result = _resolve_output_path(event, "tests/fixtures/journal")
        assert result is not None
        assert "20260214" in str(result)
        assert result.suffix in (".md", ".json")

    def test_returns_none_without_day_or_output_path(self):
        """Returns None when neither output_path nor day is present."""
        event = {"name": "chat"}
        result = _resolve_output_path(event, "/journal")
        assert result is None

    def test_empty_output_path_falls_through(self, fixture_journal):
        """Empty string output_path falls through to derivation."""
        event = {"output_path": "", "day": "20260214", "name": "chat"}
        result = _resolve_output_path(event, "tests/fixtures/journal")
        # Empty string is falsy, so falls through to derivation
        assert result is not None

    def test_uses_env_stream_name(self, fixture_journal):
        """SOL_STREAM from env is passed through to get_output_path."""
        event = {
            "day": "20260214",
            "name": "chat",
            "env": {"SOL_STREAM": "mystream"},
        }
        result = _resolve_output_path(event, "tests/fixtures/journal")
        assert result is not None

    def test_explicit_path_ignores_other_fields(self):
        """When output_path is set, day/name/segment are ignored."""
        event = {
            "output_path": "/custom/path/output.md",
            "day": "20260214",
            "name": "chat",
            "segment": "100",
        }
        result = _resolve_output_path(event, "/journal")
        assert result == Path("/custom/path/output.md")


# --- api_output_file endpoint tests ---


@pytest.fixture
def agents_client(tmp_path):
    """Create a Flask test client with agents blueprint and tmp journal."""
    from flask import Flask

    from solstone.apps.sol.routes import sol_bp
    from solstone.convey import state

    app = Flask(__name__)
    app.register_blueprint(sol_bp)

    # Point state at our tmp journal
    state.journal_root = str(tmp_path)

    # Create test files
    day_dir = tmp_path / "chronicle" / "20260214"
    day_dir.mkdir(parents=True)
    (day_dir / "talents" / "flow.md").parent.mkdir(parents=True)
    (day_dir / "talents" / "flow.md").write_text("# Day agent output")

    facet_dir = tmp_path / "facets" / "work" / "activities" / "20260214" / "coding_100"
    facet_dir.mkdir(parents=True)
    (facet_dir / "summary.md").write_text("# Activity summary")

    yield app.test_client()


class TestApiOutputFile:
    """Tests for api_output_file endpoint."""

    def test_serves_day_relative_file(self, agents_client):
        """Day-relative paths resolve under {journal}/{day}/."""
        resp = agents_client.get("/app/sol/api/output/20260214/talents/flow.md")
        assert resp.status_code == 200
        data = resp.get_json()
        assert data["content"] == "# Day agent output"
        assert data["format"] == "md"
        assert data["filename"] == "flow.md"

    def test_serves_facet_scoped_activity_file(self, agents_client):
        """Paths starting with facets/ resolve from journal root."""
        resp = agents_client.get(
            "/app/sol/api/output/20260214/"
            "facets/work/activities/20260214/coding_100/summary.md"
        )
        assert resp.status_code == 200
        data = resp.get_json()
        assert data["content"] == "# Activity summary"
        assert data["format"] == "md"

    def test_rejects_invalid_day_format(self, agents_client):
        """Non-YYYYMMDD day returns 400."""
        resp = agents_client.get("/app/sol/api/output/bad-day/talents/flow.md")
        assert resp.status_code == 400

    def test_rejects_path_traversal(self, agents_client):
        """Path traversal attempts return 403."""
        resp = agents_client.get("/app/sol/api/output/20260214/../../etc/passwd")
        assert resp.status_code in (403, 404)

    def test_missing_file_returns_404(self, agents_client):
        """Non-existent file returns 404."""
        resp = agents_client.get("/app/sol/api/output/20260214/talents/nonexistent.md")
        assert resp.status_code == 404


@pytest.fixture
def sol_listing_client(tmp_path, monkeypatch):
    """Create a sol app client backed by a temporary journal."""
    from flask import Flask

    from solstone.apps.sol.routes import sol_bp
    from solstone.convey import state

    app = Flask(__name__)
    app.register_blueprint(sol_bp)

    talents_dir = tmp_path / "talents"
    talents_dir.mkdir()

    monkeypatch.setattr(state, "journal_root", str(tmp_path))
    monkeypatch.setattr("solstone.apps.sol.routes.get_facets", lambda: {})
    monkeypatch.setattr("solstone.apps.sol.routes._build_talents_meta", lambda: {})

    return app.test_client(), talents_dir


def _write_day_index(talents_dir: Path, day: str, entries: list[dict]) -> Path:
    path = talents_dir / f"{day}.jsonl"
    lines = [json.dumps(entry) + "\n" for entry in entries]
    path.write_text("".join(lines), encoding="utf-8")
    return path


class TestApiTalentsDayListing:
    """Tests for day-index-backed talent listing."""

    def test_index_only_entry_returns_full_summary(self, sol_listing_client):
        """A complete day-index entry is enough without a per-use file."""
        client, talents_dir = sol_listing_client
        day = "20990101"
        entry = {
            "use_id": "4070908800001",
            "name": "flow",
            "day": day,
            "facet": "work",
            "ts": 4070908800000,
            "status": "error",
            "runtime_seconds": 12.3,
            "provider": "google",
            "model": "gemini-2.5-flash",
            "schedule": "daily",
            "thinking_count": 4,
            "tool_count": 2,
            "cost": 0.0123,
            "error_message": "rate limited",
            "output_file": "talents/flow.md",
            "prompt": "Summarize the day",
        }
        _write_day_index(talents_dir, day, [entry])

        resp = client.get(f"/app/sol/api/talents/{day}")

        assert resp.status_code == 200
        uses = resp.get_json()["uses"]
        assert len(uses) == 1
        assert uses[0] == {
            "id": "4070908800001",
            "name": "flow",
            "start": 4070908800000,
            "status": "error",
            "prompt": "Summarize the day",
            "facet": "work",
            "failed": True,
            "runtime_seconds": 12.3,
            "thinking_count": 4,
            "tool_count": 2,
            "cost": 0.0123,
            "model": "gemini-2.5-flash",
            "provider": "google",
            "error_message": "rate limited",
            "output_file": "talents/flow.md",
        }

    def test_legacy_agent_id_entry_returns_with_blank_new_fields(
        self, sol_listing_client
    ):
        """Legacy agent_id day-index entries are visible with missing fields blank."""
        client, talents_dir = sol_listing_client
        day = "20990102"
        agent_id = "4070995200001"
        _write_day_index(
            talents_dir,
            day,
            [
                {
                    "agent_id": agent_id,
                    "name": "entities",
                    "day": day,
                    "facet": "personal",
                    "ts": 4070995200000,
                    "status": "completed",
                    "runtime_seconds": 8.4,
                    "provider": "google",
                    "model": "gemini-2.5-flash-lite",
                }
            ],
        )

        resp = client.get(f"/app/sol/api/talents/{day}")

        assert resp.status_code == 200
        use = resp.get_json()["uses"][0]
        assert use["id"] == agent_id
        assert use["failed"] is False
        for field in (
            "thinking_count",
            "tool_count",
            "cost",
            "error_message",
            "output_file",
            "prompt",
        ):
            assert use[field] is None

    def test_current_thin_entry_returns_without_rewriting_index(
        self, sol_listing_client
    ):
        """Current thin use_id entries return with missing fields blank."""
        client, talents_dir = sol_listing_client
        day = "20990103"
        index_path = _write_day_index(
            talents_dir,
            day,
            [
                {
                    "use_id": "4071081600001",
                    "name": "knowledge_graph",
                    "day": day,
                    "facet": None,
                    "ts": 4071081600000,
                    "status": "completed",
                    "runtime_seconds": 9.1,
                    "provider": "anthropic",
                    "model": "claude-sonnet-4-5",
                    "schedule": "daily",
                }
            ],
        )
        before = index_path.read_bytes()

        resp = client.get(f"/app/sol/api/talents/{day}")

        assert resp.status_code == 200
        use = resp.get_json()["uses"][0]
        assert use["id"] == "4071081600001"
        for field in (
            "thinking_count",
            "tool_count",
            "cost",
            "error_message",
            "output_file",
            "prompt",
        ):
            assert use[field] is None
        assert index_path.read_bytes() == before


class TestApiUpdatedDays:
    """Tests for api_updated_days endpoint."""

    def test_logs_and_returns_500_on_failure(self, agents_client, monkeypatch):
        """updated_days failures return a detectable error envelope."""

        def boom(**_kwargs):
            raise RuntimeError("simulated")

        monkeypatch.setattr("solstone.apps.sol.routes.updated_days", boom)
        resp = agents_client.get("/app/sol/api/updated-days")
        assert resp.status_code == 500
        payload = resp.get_json()
        assert "error" in payload
