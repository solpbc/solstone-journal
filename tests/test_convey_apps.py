# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Tests for convey app placeholder and attention behavior."""

import time
from pathlib import Path

import pytest
from flask import Flask

from solstone.apps import (
    App,
    AppConfigError,
    AppRegistry,
    _parse_date_nav,
    _parse_facets,
)
from solstone.convey.apps import register_app_context
from solstone.convey.chat_stream import append_chat_event
from solstone.convey.icons import lucide_svg
from solstone.convey.sol_initiated.copy import CATEGORIES, KIND_SOL_CHAT_REQUEST


@pytest.fixture(autouse=True)
def _temp_journal(monkeypatch, tmp_path):
    """Ensure journaling defaults remain isolated from developer data."""
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    monkeypatch.setattr(
        "solstone.think.indexer.journal.index_file", lambda *_args: True
    )


def _context(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path,
    awareness: dict,
    *,
    day_count: int = 5,
) -> dict:
    import solstone.convey.state as convey_state

    app = Flask(__name__)
    registry = AppRegistry()
    monkeypatch.setattr(convey_state, "journal_root", str(tmp_path))
    monkeypatch.setattr("solstone.convey.shell_data._get_facets_data", lambda: [])
    monkeypatch.setattr("solstone.convey.shell_data._get_selected_facet", lambda: None)
    monkeypatch.setattr("solstone.think.awareness.get_current", lambda: awareness)
    monkeypatch.setattr(
        "solstone.think.utils.day_dirs",
        lambda: {str(index): str(index) for index in range(day_count)},
    )
    register_app_context(app, registry)
    with app.test_request_context("/"):
        context: dict = {}
        app.update_template_context(context)
    return context


def _append_request(request_id: str = "req", *, ts: int | None = None) -> None:
    fields = {
        "request_id": request_id,
        "summary": "Notice this",
        "message": None,
        "category": CATEGORIES[0],
        "dedupe": request_id,
        "dedupe_window": "24h",
        "since_ts": 1,
        "trigger_talent": "reflection",
    }
    if ts is not None:
        fields["ts"] = ts
    append_chat_event(KIND_SOL_CHAT_REQUEST, **fields)


def _now_ms() -> int:
    return int(time.time() * 1000)


def test_parse_date_nav_normalizes_content_configs():
    assert _parse_date_nav({}) is None
    assert _parse_date_nav({"date_nav": False}) is None

    unit = {"one": "segment", "other": "segments", "none": "no segments"}
    result = _parse_date_nav({"date_nav": {"unit": unit, "allow_future": True}})
    assert result == {
        "unit": unit,
        "allow_future": True,
        "step": None,
    }
    assert "mount" not in result

    assert _parse_date_nav(
        {"date_nav": {"unit": unit}, "allow_future_dates": True}
    ) == {
        "unit": unit,
        "allow_future": False,
        "step": None,
    }
    assert _parse_date_nav({"date_nav": {"unit": {"kind": "currency"}}}) == {
        "unit": {"kind": "currency"},
        "allow_future": False,
        "step": None,
    }
    assert _parse_date_nav(
        {
            "date_nav": {
                "unit": {
                    "one": "reflection",
                    "other": "reflections",
                    "none": "no reflection",
                },
                "step": "week",
            }
        }
    ) == {
        "unit": {
            "one": "reflection",
            "other": "reflections",
            "none": "no reflection",
        },
        "allow_future": False,
        "step": "week",
    }

    with pytest.raises(AppConfigError):
        _parse_date_nav({"date_nav": True})
    with pytest.raises(AppConfigError):
        _parse_date_nav({"date_nav": {"allow_future": True}})
    with pytest.raises(AppConfigError):
        _parse_date_nav({"date_nav": {"unit": {"one": "log"}}})
    with pytest.raises(AppConfigError):
        _parse_date_nav(
            {
                "date_nav": {
                    "unit": {"one": "log", "other": "logs", "none": "no logs"},
                    "step": "month",
                }
            }
        )


def test_parse_facets_false_disables_facets():
    facets_config = _parse_facets({"facets": False})

    assert facets_config == {"disabled": True}
    assert (
        App(
            name="synthetic",
            icon="",
            label="synthetic",
            facets_config=facets_config,
        ).facets_enabled()
        is False
    )


def test_parse_facets_dict_passes_through():
    facets_config = _parse_facets({"facets": {"disabled": True}})

    assert facets_config == {"disabled": True}
    assert (
        App(
            name="synthetic",
            icon="",
            label="synthetic",
            facets_config=facets_config,
        ).facets_enabled()
        is False
    )


def test_parse_facets_true_and_absent_enable_facets():
    for facets_config in (
        _parse_facets({"facets": True}),
        _parse_facets({}),
    ):
        assert facets_config == {}
        assert (
            App(
                name="synthetic",
                icon="",
                label="synthetic",
                facets_config=facets_config,
            ).facets_enabled()
            is True
        )


def test_parse_facets_non_boolean_non_dict_falls_back_to_empty():
    assert _parse_facets({"facets": "nope"}) == {}


def test_load_app_accepts_boolean_facets(tmp_path):
    (tmp_path / "workspace.html").write_text("", encoding="utf-8")
    (tmp_path / "app.json").write_text('{"facets": false}\n', encoding="utf-8")

    app = AppRegistry()._load_app("synthetic_app", tmp_path)

    assert app.facets_config == {"disabled": True}
    assert app.facets_enabled() is False


def test_discover_registers_every_workspace_app():
    import solstone.apps as apps_pkg

    apps_dir = Path(apps_pkg.__file__).parent
    expected = {
        path.name
        for path in apps_dir.iterdir()
        if path.is_dir()
        and not path.name.startswith("_")
        and (path / "workspace.html").exists()
    }

    registry = AppRegistry()
    registry.discover()

    assert set(registry.apps) == expected


def test_body_native_surface_is_absent_from_python_discovery(caplog):
    registry = AppRegistry()

    with caplog.at_level("DEBUG", logger="solstone.apps"):
        registry.discover()

    assert "body" not in registry.apps
    assert [blueprint.name for blueprint in registry.api_blueprints] == [
        "app:awareness"
    ]
    assert "Skipping body/ - no workspace.html found" in caplog.text


def test_shell_payload_emits_normalized_date_nav(monkeypatch):
    from solstone.convey.shell_data import build_shell_data

    monkeypatch.setattr("solstone.convey.shell_data.load_convey_config", lambda: {})
    monkeypatch.setattr("solstone.convey.shell_data._get_facets_data", lambda: [])
    monkeypatch.setattr("solstone.convey.shell_data._get_selected_facet", lambda: None)
    monkeypatch.setattr("solstone.convey.shell_data._build_chat_bar", lambda: {})

    registry = AppRegistry()
    registry.discover()

    apps = {app["name"]: app for app in build_shell_data(registry)["apps"]}
    date_nav_apps = [name for name, app in apps.items() if app["date_nav"] is not None]

    assert sorted(date_nav_apps) == [
        "activities",
        "chat",
        "news",
        "reflections",
        "sol",
        "timeline",
        "tokens",
        "transcripts",
    ]
    for name, app in apps.items():
        assert "allow_future_dates" not in app
        if app["date_nav"]:
            assert "mount" not in app["date_nav"]
            assert app["date_nav"]["allow_future"] is (name == "activities")
            assert app["date_nav"]["step"] == (
                "week" if name == "reflections" else None
            )


def test_get_facets_data_adds_icon_svg_and_preserves_emoji(monkeypatch):
    from solstone.convey.shell_data import _get_facets_data

    monkeypatch.setattr(
        "solstone.think.facets.get_facets",
        lambda: {
            "library": {
                "title": "Library",
                "color": "#123456",
                "emoji": "📚",
            },
            "comb": {
                "title": "Comb",
                "color": "#654321",
                "emoji": "🪮",
            },
            "override": {
                "title": "Override",
                "color": "#abcdef",
                "emoji": "📚",
                "icon": "brain",
            },
            "dangling": {
                "title": "Dangling",
                "color": "#fedcba",
                "emoji": "📚",
                "icon": "definitely-not-an-icon",
            },
        },
    )
    monkeypatch.setattr("solstone.convey.shell_data.load_convey_config", lambda: {})

    facets = _get_facets_data()
    by_name = {facet["name"]: facet for facet in facets}

    assert by_name["library"]["emoji"] == "📚"
    assert by_name["library"]["icon"] == ""
    assert "<svg" in by_name["library"]["icon_svg"]
    assert by_name["comb"]["emoji"] == "🪮"
    assert by_name["comb"]["icon"] == ""
    assert by_name["comb"]["icon_svg"] is None
    assert by_name["override"]["icon"] == "brain"
    assert by_name["override"]["icon_svg"] == lucide_svg("brain")
    assert by_name["override"]["icon_svg"] != lucide_svg("library")
    assert by_name["dangling"]["icon"] == "definitely-not-an-icon"
    assert by_name["dangling"]["icon_svg"] == lucide_svg("library")


# --- Placeholder resolution ---


class TestPlaceholderResolution:
    def test_no_imports_young(self):
        from solstone.convey.shell_data import _resolve_placeholder

        result = _resolve_placeholder({}, 0)
        assert "bring in past conversations" in result

    def test_no_daily(self):
        from solstone.convey.shell_data import _resolve_placeholder

        current = {"imports": {"has_imported": True}}
        result = _resolve_placeholder(current, 0)
        assert "sol is keeping your journal" in result

    def test_first_daily_young(self):
        from solstone.convey.shell_data import _resolve_placeholder

        current = {
            "imports": {"has_imported": True},
            "journal": {"first_daily_ready": True},
        }
        result = _resolve_placeholder(current, 1)
        assert "first daily analysis is ready" in result

    def test_first_daily_mid(self):
        from solstone.convey.shell_data import _resolve_placeholder

        current = {"journal": {"first_daily_ready": True}}
        result = _resolve_placeholder(current, 3)
        assert "daily analysis is ready" in result
        assert "first" not in result

    def test_first_daily_mature(self):
        from solstone.convey.shell_data import _resolve_placeholder

        current = {"journal": {"first_daily_ready": True}}
        result = _resolve_placeholder(current, 10)
        assert "ask me about your day" in result

    def test_default_fallback(self):
        from solstone.convey.shell_data import _resolve_placeholder

        result = _resolve_placeholder({}, 5)
        assert "sol is keeping your journal" in result


class TestInjectedChatBarContext:
    def test_no_attention_or_sol_request_uses_fallback_context(
        self, monkeypatch, tmp_path
    ):
        context = _context(monkeypatch, tmp_path, {"imports": {"has_imported": True}})

        assert context["chat_bar_placeholder"] == (
            "sol is keeping your journal. your first daily analysis will be ready soon…"
        )
        assert context["chat_bar_attention"] is None
        assert context["chat_bar_sol_request"] is None

    def test_attention_surfaces_structured_copy_and_keeps_fallback_placeholder(
        self, monkeypatch, tmp_path
    ):
        from datetime import datetime

        context = _context(
            monkeypatch,
            tmp_path,
            {
                "imports": {
                    "has_imported": True,
                    "last_completed": datetime.now().isoformat(),
                    "last_result_summary": "142 Calendar events",
                }
            },
        )

        assert context["chat_bar_attention"] == {
            "placeholder_text": "import complete: 142 Calendar events. ask me about it"
        }
        assert context["chat_bar_sol_request"] is None
        assert context["chat_bar_placeholder"] == (
            "sol is keeping your journal. your first daily analysis will be ready soon…"
        )

    def test_sol_request_surfaces_structured_state(self, monkeypatch, tmp_path):
        from datetime import date

        _append_request("req")

        context = _context(monkeypatch, tmp_path, {"imports": {"has_imported": True}})

        assert context["chat_bar_sol_request"]["request_id"] == "req"
        assert context["chat_bar_sol_request"]["summary"] == "Notice this"
        assert isinstance(context["chat_bar_sol_request"]["ts"], int)
        assert context["chat_bar_sol_request"]["event_index"] == 0
        assert context["chat_bar_sol_request"]["day"] == date.today().strftime("%Y%m%d")
        assert set(context["chat_bar_sol_request"]) == {
            "request_id",
            "summary",
            "ts",
            "event_index",
            "day",
        }
        assert context["chat_bar_attention"] is None
        assert context["chat_bar_placeholder"] == (
            "sol is keeping your journal. your first daily analysis will be ready soon…"
        )

    def test_past_day_request_does_not_surface(self, monkeypatch, tmp_path):
        from datetime import date, datetime, time, timedelta

        from solstone.think.utils import get_owner_timezone

        yesterday = date.today() - timedelta(days=1)
        yesterday_dt = datetime.combine(
            yesterday,
            time(hour=12),
            tzinfo=get_owner_timezone(),
        )
        _append_request("past", ts=int(yesterday_dt.timestamp() * 1000))

        context = _context(monkeypatch, tmp_path, {"imports": {"has_imported": True}})

        assert context["chat_bar_sol_request"] is None

    def test_awareness_context_failure_logs_and_uses_fallback(
        self, monkeypatch, tmp_path, caplog
    ):
        import solstone.convey.state as convey_state

        def fail_current():
            raise RuntimeError("boom")

        app = Flask(__name__)
        registry = AppRegistry()
        monkeypatch.setattr(convey_state, "journal_root", str(tmp_path))
        monkeypatch.setattr("solstone.convey.shell_data._get_facets_data", lambda: [])
        monkeypatch.setattr(
            "solstone.convey.shell_data._get_selected_facet", lambda: None
        )
        monkeypatch.setattr("solstone.think.awareness.get_current", fail_current)
        register_app_context(app, registry)

        with caplog.at_level("WARNING", logger="solstone.convey.shell_data"):
            with app.test_request_context("/"):
                context: dict = {}
                app.update_template_context(context)

        assert context["chat_bar_placeholder"] == "send a message…"
        assert context["chat_bar_attention"] is None
        assert "failed to resolve chat bar shell context" in caplog.text


class TestAttentionResolution:
    """Tests for _resolve_attention() and attention-aware placeholder resolution."""

    def test_no_attention_returns_none(self):
        from solstone.convey.shell_data import _resolve_attention

        assert _resolve_attention({}) is None

    def test_no_attention_empty_sections(self):
        from solstone.convey.shell_data import _resolve_attention

        current = {"imports": {"has_imported": True}, "journal": {}}
        assert _resolve_attention(current) is None

    def test_cortex_attention_failure_logs_and_falls_through(self, monkeypatch, caplog):
        from solstone.convey.shell_data import _resolve_attention

        def fail_scan():
            raise RuntimeError("boom")

        monkeypatch.setattr(
            "solstone.convey.shell_data.read_unresolved_agent_failures", fail_scan
        )

        with caplog.at_level("WARNING", logger="solstone.convey.shell_data"):
            assert _resolve_attention({}) is None

        assert "failed to resolve chat bar cortex attention" in caplog.text

    def test_p1_recent_import(self):
        from datetime import datetime

        from solstone.convey.shell_data import _resolve_attention

        current = {
            "imports": {
                "has_imported": True,
                "last_completed": datetime.now().isoformat(),
                "last_result_summary": "142 Calendar events",
            }
        }
        result = _resolve_attention(current)
        assert result is not None
        assert "import" in result.placeholder_text.lower()
        assert len(result.placeholder_text) <= 90

    def test_p2_old_import_no_attention(self):
        from datetime import datetime, timedelta

        from solstone.convey.shell_data import _resolve_attention

        old_time = (datetime.now() - timedelta(hours=2)).isoformat()
        current = {
            "imports": {
                "has_imported": True,
                "last_completed": old_time,
                "last_result_summary": "142 Calendar events",
            }
        }
        assert _resolve_attention(current) is None

    def test_p0_cortex_errors(self, tmp_path, monkeypatch):
        """Cortex errors are P0 — highest priority."""
        import json
        from datetime import datetime

        from solstone.convey.shell_data import _resolve_attention

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        today = datetime.now().strftime("%Y%m%d")
        now_ms = _now_ms()
        agents_dir = tmp_path / "talents"
        agents_dir.mkdir()
        day_index = agents_dir / f"{today}.jsonl"
        day_index.write_text(
            json.dumps(
                {
                    "use_id": "1",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms,
                    "status": "error",
                }
            )
            + "\n"
            + json.dumps(
                {
                    "use_id": "2",
                    "name": "meetings",
                    "day": today,
                    "ts": now_ms + 1,
                    "status": "completed",
                }
            )
            + "\n"
        )

        result = _resolve_attention({})
        assert result is not None
        assert result.placeholder_text == "1 agent error today — ask what happened"
        assert "error" in result.placeholder_text.lower()
        assert "1" in result.placeholder_text
        assert len(result.placeholder_text) <= 90

    def test_p0_cortex_error_executed_today_from_old_index(self, tmp_path, monkeypatch):
        """A run executed today surfaces even if recorded under an older journal day."""
        import json
        from datetime import datetime, timedelta

        from solstone.convey.shell_data import _resolve_attention

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        old_day = (datetime.now() - timedelta(days=10)).strftime("%Y%m%d")
        agents_dir = tmp_path / "talents"
        agents_dir.mkdir()
        (agents_dir / f"{old_day}.jsonl").write_text(
            json.dumps(
                {
                    "use_id": "old-index-error",
                    "name": "flow",
                    "day": old_day,
                    "ts": _now_ms(),
                    "status": "error",
                }
            )
            + "\n"
        )

        result = _resolve_attention({})

        assert result is not None
        assert result.placeholder_text == "1 agent error today — ask what happened"
        assert len(result.placeholder_text) <= 90

    def test_degraded_scan_preempts_recent_import_attention(
        self, tmp_path, monkeypatch
    ):
        """A degraded talent-error scan outranks lower-priority import attention."""
        import json
        from datetime import datetime
        from pathlib import Path

        from solstone.convey.shell_data import _resolve_attention

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        today = datetime.now().strftime("%Y%m%d")
        agents_dir = tmp_path / "talents"
        agents_dir.mkdir()
        unreadable = agents_dir / f"{today}.jsonl"
        unreadable.write_text(
            json.dumps(
                {
                    "use_id": "unreadable",
                    "name": "flow",
                    "day": today,
                    "ts": _now_ms(),
                    "status": "error",
                }
            )
            + "\n"
        )
        original_read_text = Path.read_text

        def fake_read_text(self: Path, *args, **kwargs) -> str:
            if self == unreadable:
                raise OSError("cannot read")
            return original_read_text(self, *args, **kwargs)

        monkeypatch.setattr(Path, "read_text", fake_read_text)

        current = {
            "imports": {
                "has_imported": True,
                "last_completed": datetime.now().isoformat(),
                "last_result_summary": "10 items",
            }
        }
        result = _resolve_attention(current)

        assert result is not None
        assert result.placeholder_text == (
            "couldn't check talent errors today. ask what needs attention"
        )
        assert len(result.placeholder_text) <= 90
        assert "import" not in result.placeholder_text
        assert any("incomplete" in line for line in result.context_lines)
        assert any(
            "Do not report a zero error count" in line for line in result.context_lines
        )

    def test_p0_readiness_error_prefers_setup_guidance(self, tmp_path, monkeypatch):
        """Readiness blockers get setup guidance instead of generic error copy."""
        import json
        from datetime import datetime

        from solstone.convey.shell_data import _resolve_attention

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        today = datetime.now().strftime("%Y%m%d")
        now_ms = _now_ms()
        agents_dir = tmp_path / "talents"
        agents_dir.mkdir()
        day_index = agents_dir / f"{today}.jsonl"
        day_index.write_text(
            json.dumps(
                {
                    "use_id": "1",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms,
                    "status": "error",
                    "reason_code": "provider_key_missing",
                    "provider": "anthropic",
                    "model": "claude-test",
                }
            )
            + "\n"
        )

        result = _resolve_attention({})

        assert result is not None
        assert "agent error" not in result.placeholder_text
        assert "Anthropic needs credentials" in result.placeholder_text
        assert len(result.placeholder_text) <= 90
        assert any("provider setup" in line for line in result.context_lines)
        assert any(
            "reason_code=provider_key_missing" in line for line in result.context_lines
        )
        assert any("provider=anthropic" in line for line in result.context_lines)
        assert any("model=claude-test" in line for line in result.context_lines)

    def test_p0_self_healing(self, tmp_path, monkeypatch):
        """An error followed by a success for the same agent is resolved."""
        import json
        from datetime import datetime

        from solstone.convey.shell_data import _resolve_attention

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        today = datetime.now().strftime("%Y%m%d")
        now_ms = _now_ms()
        agents_dir = tmp_path / "talents"
        agents_dir.mkdir()
        day_index = agents_dir / f"{today}.jsonl"
        day_index.write_text(
            json.dumps(
                {
                    "use_id": "1",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms,
                    "status": "error",
                    "reason_code": "provider_key_missing",
                    "provider": "anthropic",
                    "model": "claude-test",
                }
            )
            + "\n"
            + json.dumps(
                {
                    "use_id": "3",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms + 1,
                    "status": "completed",
                }
            )
            + "\n"
        )

        result = _resolve_attention({})
        assert result is None

    def test_p0_counts_unresolved_occurrences_not_distinct_names(
        self, tmp_path, monkeypatch
    ):
        """Multiple unresolved errors for one agent count as multiple occurrences."""
        import json
        from datetime import datetime

        from solstone.convey.shell_data import _resolve_attention

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        today = datetime.now().strftime("%Y%m%d")
        now_ms = _now_ms()
        agents_dir = tmp_path / "talents"
        agents_dir.mkdir()
        day_index = agents_dir / f"{today}.jsonl"
        day_index.write_text(
            json.dumps(
                {
                    "use_id": "1",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms,
                    "status": "error",
                }
            )
            + "\n"
            + json.dumps(
                {
                    "use_id": "2",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms + 1,
                    "status": "error",
                }
            )
            + "\n"
        )

        result = _resolve_attention({})
        assert result is not None
        assert result.placeholder_text == "2 agent errors today — ask what happened"
        assert result.context_lines == [
            "System health: 2 unresolved agent error(s) today: flow. If user asks "
            "what needs attention, summarize which agents failed."
        ]

    def test_p0_later_success_resolves_earlier_occurrences_only(
        self, tmp_path, monkeypatch
    ):
        """Later same-agent errors after a success remain unresolved."""
        import json
        from datetime import datetime

        from solstone.convey.shell_data import _resolve_attention

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        today = datetime.now().strftime("%Y%m%d")
        now_ms = _now_ms()
        agents_dir = tmp_path / "talents"
        agents_dir.mkdir()
        day_index = agents_dir / f"{today}.jsonl"
        day_index.write_text(
            json.dumps(
                {
                    "use_id": "1",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms,
                    "status": "error",
                }
            )
            + "\n"
            + json.dumps(
                {
                    "use_id": "2",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms + 1,
                    "status": "completed",
                }
            )
            + "\n"
            + json.dumps(
                {
                    "use_id": "3",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms + 2,
                    "status": "error",
                }
            )
            + "\n"
        )

        result = _resolve_attention({})
        assert result is not None
        assert result.placeholder_text == "1 agent error today — ask what happened"

    def test_p0_home_attention_count_matches_health_seed_count(
        self, tmp_path, monkeypatch
    ):
        """Home attention and health seed use the same occurrence count."""
        import json
        import time
        from datetime import datetime

        from solstone.apps.health.routes import _build_agent_error_seed
        from solstone.convey.shell_data import _resolve_attention
        from solstone.think.talent_runs import read_unresolved_agent_failures

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        today = datetime.now().strftime("%Y%m%d")
        now_ms = int(time.time() * 1000)
        agents_dir = tmp_path / "talents"
        agents_dir.mkdir()
        day_index = agents_dir / f"{today}.jsonl"
        day_index.write_text(
            json.dumps(
                {
                    "use_id": "1",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms,
                    "status": "error",
                }
            )
            + "\n"
            + json.dumps(
                {
                    "use_id": "2",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms + 1,
                    "status": "error",
                }
            )
            + "\n"
            + json.dumps(
                {
                    "use_id": "3",
                    "name": "meetings",
                    "day": today,
                    "ts": now_ms + 2,
                    "status": "error",
                }
            )
            + "\n"
        )

        scan = read_unresolved_agent_failures()
        attention = _resolve_attention({})

        assert attention is not None
        assert attention.placeholder_text == "3 agent errors today — ask what happened"
        home_count = int(attention.placeholder_text.split(" ", 1)[0])
        assert (
            home_count == len(_build_agent_error_seed(scan)) == len(scan.failures) == 3
        )

    def test_p0_readiness_branch_uses_latest_error_per_name(
        self, tmp_path, monkeypatch
    ):
        """An older blocker does not mask a later unresolved non-blocking error."""
        import json
        from datetime import datetime

        from solstone.convey.shell_data import _resolve_attention

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        today = datetime.now().strftime("%Y%m%d")
        now_ms = _now_ms()
        agents_dir = tmp_path / "talents"
        agents_dir.mkdir()
        day_index = agents_dir / f"{today}.jsonl"
        day_index.write_text(
            json.dumps(
                {
                    "use_id": "1",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms,
                    "status": "error",
                    "reason_code": "provider_key_missing",
                    "provider": "anthropic",
                }
            )
            + "\n"
            + json.dumps(
                {
                    "use_id": "2",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms + 1,
                    "status": "error",
                    "reason_code": "no_output",
                }
            )
            + "\n"
        )

        result = _resolve_attention({})
        assert result is not None
        assert result.placeholder_text == "2 agent errors today — ask what happened"

    def test_priority_p0_over_p1_imports(self, tmp_path, monkeypatch):
        """P0 (cortex errors) takes priority over P1 (recent import)."""
        import json
        from datetime import datetime

        from solstone.convey.shell_data import _resolve_attention

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        today = datetime.now().strftime("%Y%m%d")
        now_ms = _now_ms()
        agents_dir = tmp_path / "talents"
        agents_dir.mkdir()
        day_index = agents_dir / f"{today}.jsonl"
        day_index.write_text(
            json.dumps(
                {
                    "use_id": "1",
                    "name": "flow",
                    "day": today,
                    "ts": now_ms,
                    "status": "error",
                }
            )
            + "\n"
        )

        current = {
            "imports": {
                "has_imported": True,
                "last_completed": datetime.now().isoformat(),
                "last_result_summary": "10 items",
            }
        }
        result = _resolve_attention(current)
        assert result is not None
        assert "error" in result.placeholder_text.lower()

    def test_placeholder_no_attention_preserves_behavior(self):
        """When no attention items, existing placeholder logic unchanged."""
        from solstone.convey.shell_data import _resolve_placeholder

        current = {"journal": {"first_daily_ready": True}}
        result = _resolve_placeholder(current, 10)
        assert "ask me about your day" in result

    def test_all_placeholder_texts_under_90_chars(self, tmp_path, monkeypatch):
        """All attention placeholder texts must be <=90 characters."""
        import json
        from datetime import datetime

        from solstone.convey.shell_data import _resolve_attention

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        today = datetime.now().strftime("%Y%m%d")
        agents_dir = tmp_path / "talents"
        agents_dir.mkdir()
        day_index = agents_dir / f"{today}.jsonl"
        day_index.write_text(
            json.dumps(
                {"use_id": "1", "name": "flow", "ts": _now_ms(), "status": "error"}
            )
            + "\n"
        )
        result = _resolve_attention({})
        assert result is not None
        assert len(result.placeholder_text) <= 90

        day_index.unlink()
        agents_dir.rmdir()
        result = _resolve_attention(
            {
                "imports": {
                    "last_completed": datetime.now().isoformat(),
                    "last_result_summary": "142 Calendar events",
                }
            }
        )
        assert result is not None
        assert len(result.placeholder_text) <= 90

    def test_p3_daily_analysis(self, tmp_path, monkeypatch):
        """P3: daily analysis outputs available."""
        from datetime import datetime

        from solstone.convey.shell_data import _resolve_attention

        monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))

        today = datetime.now().strftime("%Y%m%d")
        agents_dir = tmp_path / today / "talents"
        agents_dir.mkdir(parents=True)
        (agents_dir / "flow.md").write_text("# Flow")
        (agents_dir / "meetings.md").write_text("# Meetings")

        current = {"journal": {"first_daily_ready": True}}
        result = _resolve_attention(current)
        assert result is not None
        assert "2" in result.placeholder_text
        assert "report" in result.placeholder_text.lower()
        assert len(result.placeholder_text) <= 90
