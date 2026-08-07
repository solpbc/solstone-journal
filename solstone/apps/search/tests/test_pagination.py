# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from typing import Any

import pytest

from solstone.apps.search import routes
from solstone.convey import create_app


def _counts_payload(counts: dict[str, Any]) -> dict[str, Any]:
    return {
        "facets": {},
        "agents": {},
        "days": {},
        "streams": {},
        "total": 0,
        "relaxed": False,
        **counts,
    }


@pytest.fixture
def search_client(tmp_path, monkeypatch):
    journal = tmp_path / "journal"
    config_dir = journal / "config"
    config_dir.mkdir(parents=True)
    (config_dir / "journal.json").write_text(
        json.dumps(
            {
                "setup": {"completed_at": 1700000000000},
            },
            indent=2,
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(journal))

    app = create_app(journal=str(journal))
    return app.test_client()


def _stub_search(
    monkeypatch,
    counts: dict[str, Any] | list[dict[str, Any]],
    *,
    coverage: dict[str, str] | None = None,
) -> dict[str, Any]:
    recorded: dict[str, Any] = {"search_counts": [], "search_journal": []}
    count_payloads = counts if isinstance(counts, list) else [counts]

    def fake_search_journal(*_args, **kwargs):
        recorded["limit"] = kwargs.get("limit")
        recorded["offset"] = kwargs.get("offset")
        recorded["search_journal"].append(kwargs)
        return 0, []

    def fake_counts(*_args, **kwargs):
        recorded["search_counts"].append(kwargs)
        index = min(len(recorded["search_counts"]) - 1, len(count_payloads) - 1)
        return _counts_payload(count_payloads[index])

    monkeypatch.setattr(routes, "search_journal", fake_search_journal)
    monkeypatch.setattr(routes, "search_counts", fake_counts)
    monkeypatch.setattr(routes, "get_corpus_day_coverage", lambda: coverage)
    return recorded


def test_day_results_non_numeric_limit_is_200_and_defaults(search_client, monkeypatch):
    recorded = _stub_search(monkeypatch, {"total": 0})

    response = search_client.get(
        "/app/search/api/day_results?q=x&day=20260304&limit=abc"
    )

    assert response.status_code == 200
    assert recorded["limit"] == 20


def test_day_results_high_limit_clamped(search_client, monkeypatch):
    recorded = _stub_search(monkeypatch, {"total": 0})

    response = search_client.get(
        "/app/search/api/day_results?q=x&day=20260304&limit=100000"
    )

    assert response.status_code == 200
    assert recorded["limit"] == 100


def test_day_results_lower_bound(search_client, monkeypatch):
    recorded = _stub_search(monkeypatch, {"total": 0})

    response = search_client.get("/app/search/api/day_results?q=x&day=20260304&limit=0")

    assert response.status_code == 200
    assert recorded["limit"] == 1

    response = search_client.get(
        "/app/search/api/day_results?q=x&day=20260304&offset=-5"
    )

    assert response.status_code == 200
    assert recorded["offset"] == 0


def test_result_fetches_omit_total(search_client, monkeypatch):
    recorded = _stub_search(monkeypatch, {"days": {"20260304": 1}, "total": 1})

    response = search_client.get("/app/search/api/search?q=test")

    assert response.status_code == 200
    assert recorded["search_journal"][-1]["include_total"] is False

    response = search_client.get("/app/search/api/day_results?q=test&day=20260304")

    assert response.status_code == 200
    assert recorded["search_journal"][-1]["include_total"] is False


def test_search_non_numeric_limit_is_200(search_client, monkeypatch):
    recorded = _stub_search(
        monkeypatch,
        {"facets": [], "agents": [], "days": [("20260304", 3)], "total": 3},
    )

    response = search_client.get("/app/search/api/search?q=test&limit=abc")

    assert response.status_code == 200
    assert recorded["limit"] == 5


@pytest.mark.parametrize("relaxed", [True, False])
def test_search_api_returns_relaxed_flag(search_client, monkeypatch, relaxed):
    _stub_search(
        monkeypatch,
        {
            "facets": {},
            "agents": {},
            "days": {"20260304": 1},
            "total": 1,
            "relaxed": relaxed,
        },
    )

    response = search_client.get("/app/search/api/search?q=test")

    assert response.status_code == 200
    assert response.get_json()["relaxed"] is relaxed


def test_search_range_filters_day_groups_inclusively(search_client, monkeypatch):
    _stub_search(
        monkeypatch,
        {
            "days": {
                "20260303": 1,
                "20260304": 2,
                "20260305": 3,
                "20260306": 4,
            },
            "total": 10,
        },
    )

    response = search_client.get(
        "/app/search/api/search",
        query_string={
            "q": "test",
            "day_from": "20260304",
            "day_to": "20260305",
        },
    )

    assert response.status_code == 200
    payload = response.get_json()
    assert [day["day"] for day in payload["days"]] == ["20260305", "20260304"]
    assert payload["total"] == 5
    assert payload["total_days"] == 2


@pytest.mark.parametrize(
    ("params", "detail"),
    [
        ({"day_from": "2026030"}, "day_from must be YYYYMMDD"),
        ({"day_to": "20261332"}, "day_to must be a real day"),
        (
            {"day_from": "20260305", "day_to": "20260304"},
            "day_from must be <= day_to",
        ),
    ],
)
def test_search_range_invalid_days_return_invalid_day(
    search_client, monkeypatch, params, detail
):
    _stub_search(monkeypatch, {"total": 0})

    response = search_client.get(
        "/app/search/api/search",
        query_string={"q": "test", **params},
    )

    assert response.status_code == 400
    payload = response.get_json()
    assert payload["reason_code"] == "invalid_day"
    assert payload["detail"] == detail


def test_search_range_leaves_day_grid_unfiltered(search_client, monkeypatch):
    _stub_search(
        monkeypatch,
        {
            "days": {
                "20260301": 1,
                "20260302": 2,
                "20260303": 3,
            },
            "total": 6,
        },
        coverage={"start": "20260101", "end": "20261231"},
    )

    response = search_client.get(
        "/app/search/api/search",
        query_string={
            "q": "test",
            "day_from": "20260302",
            "day_to": "20260302",
        },
    )

    assert response.status_code == 200
    payload = response.get_json()
    assert [day["day"] for day in payload["days"]] == ["20260302"]
    assert payload["day_grid"] == {
        "coverage": {"start": "20260101", "end": "20261231"},
        "days": {
            "20260301": 1,
            "20260302": 2,
            "20260303": 3,
        },
        "pending": {},
    }


def test_search_day_grid_uses_corpus_span_not_match_span(search_client, monkeypatch):
    _stub_search(
        monkeypatch,
        {"days": {"20260304": 2}, "total": 2},
        coverage={"start": "20250101", "end": "20261231"},
    )

    response = search_client.get("/app/search/api/search?q=test")

    assert response.status_code == 200
    assert response.get_json()["day_grid"]["coverage"] == {
        "start": "20250101",
        "end": "20261231",
    }


def test_search_day_grid_has_no_pending_days(search_client, monkeypatch):
    _stub_search(
        monkeypatch,
        {"days": {"20260304": 2, "20260306": 1}, "total": 3},
    )

    response = search_client.get("/app/search/api/search?q=test")

    assert response.status_code == 200
    payload = response.get_json()["day_grid"]
    assert payload["days"] == {"20260304": 2, "20260306": 1}
    assert payload["pending"] == {}


def test_search_range_scopes_totals_not_discovery_counts(search_client, monkeypatch):
    monkeypatch.setattr(
        routes,
        "get_facets",
        lambda: {
            "work": {"title": "Work", "color": "", "emoji": ""},
            "personal": {"title": "Personal", "color": "", "emoji": ""},
        },
    )
    _stub_search(
        monkeypatch,
        [
            {
                "facets": {"work": 12, "personal": 5},
                "agents": {"flow": 9},
                "days": {
                    "20260301": 1,
                    "20260302": 2,
                    "20260303": 3,
                },
                "total": 6,
            },
            {
                "facets": {"work": 99},
                "agents": {"screen": 99},
                "days": {
                    "20260301": 1,
                    "20260302": 2,
                    "20260303": 3,
                },
                "total": 6,
            },
        ],
    )

    response = search_client.get(
        "/app/search/api/search",
        query_string={
            "q": "test",
            "day_from": "20260302",
            "day_to": "20260302",
        },
    )

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["total"] == 2
    assert payload["total_days"] == 1
    assert {facet["name"]: facet["count"] for facet in payload["facets"]} == {
        "work": 12,
        "personal": 5,
    }
    assert [
        {key: talent[key] for key in ("name", "label", "count")}
        for talent in payload["talents"]
    ] == [{"name": "flow", "label": "Flow", "count": 9}]


def test_search_zero_matches_returns_empty_day_grid_map(search_client, monkeypatch):
    _stub_search(
        monkeypatch,
        {"days": {}, "total": 0},
        coverage={"start": "20260101", "end": "20261231"},
    )

    response = search_client.get("/app/search/api/search?q=test")

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["days"] == []
    assert payload["day_grid"] == {
        "coverage": {"start": "20260101", "end": "20261231"},
        "days": {},
        "pending": {},
    }


def test_search_no_range_preserves_dayless_total(search_client, monkeypatch):
    _stub_search(
        monkeypatch,
        {"days": {"20260304": 2, "20260305": 1}, "total": 10},
    )

    response = search_client.get("/app/search/api/search?q=test")

    assert response.status_code == 200
    payload = response.get_json()
    assert payload["total"] == 10
    assert payload["total_days"] == 2


def test_search_range_never_reaches_indexer_kwargs(search_client, monkeypatch):
    recorded = _stub_search(
        monkeypatch,
        {"days": {"20260304": 2, "20260305": 1}, "total": 3},
    )

    response = search_client.get(
        "/app/search/api/search",
        query_string={
            "q": "test",
            "day_from": "20260304",
            "day_to": "20260305",
        },
    )

    assert response.status_code == 200
    for kwargs in recorded["search_counts"] + recorded["search_journal"]:
        assert "day_from" not in kwargs
        assert "day_to" not in kwargs
