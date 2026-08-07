# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any

import pytest

from solstone.convey import create_app
from solstone.think.indexer.journal import scan_journal


@pytest.fixture
def apostrophe_search_client(tmp_path, monkeypatch):
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
    import solstone.think.utils as think_utils
    from solstone.think import core_handshake
    from solstone.think.indexer import journal as journal_index, native

    think_utils._journal_path_cache = None
    helper = Path(__file__).resolve().parents[4] / "core" / "target" / "debug" / "solstone-core"
    native_kwargs = {
        "handshake_checker": lambda: core_handshake.CoreHandshakeResult("ok"),
        "helper_locator": lambda: helper,
        "platform_reader": lambda: ("linux", "x86_64"),
        "platform_tag_reader": lambda: {"manylinux2014_x86_64"},
    }
    monkeypatch.setattr(
        journal_index,
        "run_native_indexer_search",
        lambda query, journal_path, **options: native.run_native_indexer_search(
            query, journal_path, **options, **native_kwargs
        ),
    )
    monkeypatch.setattr(
        journal_index,
        "run_native_indexer_agents",
        lambda journal_path: native.run_native_indexer_agents(journal_path, **native_kwargs),
    )
    monkeypatch.setattr(
        journal_index,
        "run_native_indexer_coverage",
        lambda journal_path: native.run_native_indexer_coverage(journal_path, **native_kwargs),
    )

    seeded_day = "20240101"
    talents_dir = journal / "chronicle" / seeded_day / "talents"
    talents_dir.mkdir(parents=True)
    (talents_dir / "flow.md").write_text(
        "# Apostrophe Search\n\n"
        "it's indexed exactly here. "
        "Bob O'Brien said don't panic. "
        "O'Brien brought dogs to the review.\n"
    )

    yesterday = (datetime.now() - timedelta(days=1)).strftime("%Y%m%d")
    yesterday_dir = journal / "chronicle" / yesterday / "talents"
    yesterday_dir.mkdir(parents=True, exist_ok=True)
    (yesterday_dir / "flow.md").write_text(
        "# Yesterday\n\nReviewed yesterday's meeting notes.\n"
    )

    # The control day carries text IDENTICAL to yesterday's, so the only thing that
    # can keep it out of a "yesterday's meeting" result is the temporal narrowing the
    # query itself carries. With different text, FTS excludes it on its own and a test
    # asserting "control is absent" proves nothing about dates — it stays green even
    # when explicit day bounds kill the extracted-temporal branch outright.
    control_day = (datetime.now() - timedelta(days=2)).strftime("%Y%m%d")
    control_dir = journal / "chronicle" / control_day / "talents"
    control_dir.mkdir(parents=True, exist_ok=True)
    (control_dir / "flow.md").write_text(
        "# Control\n\nReviewed yesterday's meeting notes.\n"
    )

    scan_journal(str(journal), full=True)

    app = create_app(journal=str(journal))
    return app.test_client(), seeded_day, yesterday, control_day


def _get_json(response) -> dict[str, Any]:
    payload = response.get_json()
    assert isinstance(payload, dict)
    return payload


def test_search_api_finds_single_apostrophe_term(apostrophe_search_client):
    client, *_ = apostrophe_search_client

    response = client.get("/app/search/api/search", query_string={"q": "it's"})

    assert response.status_code == 200
    payload = _get_json(response)
    assert payload["total"] >= 1
    assert payload["days"]


def test_search_api_finds_apostrophe_operator_query(apostrophe_search_client):
    client, *_ = apostrophe_search_client

    response = client.get(
        "/app/search/api/search", query_string={"q": "O'Brien AND dogs"}
    )

    assert response.status_code == 200
    payload = _get_json(response)
    assert payload["total"] >= 1
    assert payload["days"]


def test_search_api_finds_temporal_apostrophe_query(apostrophe_search_client):
    client, *_ = apostrophe_search_client

    response = client.get(
        "/app/search/api/search", query_string={"q": "yesterday's meeting"}
    )

    assert response.status_code == 200
    payload = _get_json(response)
    assert payload["total"] >= 1
    assert payload["days"]


def test_search_api_sentinel_bounds_preserve_temporal_query(apostrophe_search_client):
    client, _seeded_day, yesterday, control_day = apostrophe_search_client
    query = {"q": "yesterday's meeting"}

    plain_response = client.get("/app/search/api/search", query_string=query)
    sentinel_response = client.get(
        "/app/search/api/search",
        query_string={**query, "day_from": "00000000", "day_to": "99999999"},
    )

    assert plain_response.status_code == 200
    assert sentinel_response.status_code == 200
    plain_payload = _get_json(plain_response)
    sentinel_payload = _get_json(sentinel_response)
    assert sentinel_payload == plain_payload

    returned_days = [group["day"] for group in sentinel_payload["days"]]
    assert returned_days == [yesterday]
    assert control_day not in returned_days


def test_search_api_apostrophe_only_is_json_not_500(apostrophe_search_client):
    client, *_ = apostrophe_search_client

    response = client.get("/app/search/api/search", query_string={"q": "'"})

    assert response.status_code == 200
    assert response.status_code != 500
    payload = _get_json(response)
    assert "total" in payload


def test_day_results_api_handles_apostrophe_query(apostrophe_search_client):
    client, seeded_day, *_ = apostrophe_search_client

    response = client.get(
        "/app/search/api/day_results",
        query_string={"q": "it's", "day": seeded_day},
    )

    assert response.status_code == 200
    payload = _get_json(response)
    assert payload["total"] >= 1
    assert payload["results"]
