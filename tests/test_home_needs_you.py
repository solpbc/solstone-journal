# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import logging
from dataclasses import fields
from datetime import datetime
from pathlib import Path
from typing import Any

from solstone.apps.home.needs_you import (
    DISABLED_EMPTY_PROMPT_REASON,
    DISABLED_INVALID_ROUTE_REASON,
    NeedsYouItem,
    _chat_item,
    _normalize_route_payload,
    classify_needs_you,
    format_degraded_capture_line,
    needs_dedup_key,
)


def _june_22_ms() -> float:
    return datetime(2026, 6, 22, 12, 0, 0).timestamp() * 1000


def _use_tmp_journal(tmp_path: Path, monkeypatch) -> Path:
    monkeypatch.setenv("SOLSTONE_JOURNAL", str(tmp_path))
    import solstone.think.utils as think_utils

    think_utils._journal_path_cache = None
    config_path = tmp_path / "config" / "journal.json"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    config_path.write_text(
        json.dumps({"setup": {"completed_at": 1700000000000}}) + "\n",
        encoding="utf-8",
    )
    return tmp_path


def _seed_owner_candidate(
    journal: Path,
    *,
    recommendation: str,
    streams_represented: int = 2,
) -> None:
    from solstone.think.awareness import update_state

    candidate_path = journal / "awareness" / "owner_candidate.npz"
    candidate_path.parent.mkdir(parents=True, exist_ok=True)
    candidate_path.write_bytes(b"candidate")
    update_state(
        "voiceprint",
        {
            "status": "candidate",
            "cluster_size": 40,
            "streams_represented": streams_represented,
            "recommendation": recommendation,
            "samples": [],
        },
    )


def _seed_principal_centroid(journal: Path) -> None:
    entity_dir = journal / "entities" / "self_person"
    entity_dir.mkdir(parents=True, exist_ok=True)
    (entity_dir / "entity.json").write_text(
        json.dumps(
            {
                "id": "self_person",
                "name": "Self Person",
                "type": "Person",
                "is_principal": True,
            }
        )
        + "\n",
        encoding="utf-8",
    )
    (entity_dir / "owner_centroid.npz").write_bytes(b"centroid")


def _discovery_record(
    day: str,
    stream: str,
    segment_key: str,
    *,
    source: str = "mic_audio",
    sentence_id: int = 1,
) -> dict[str, Any]:
    return {
        "day": day,
        "stream": stream,
        "segment_key": segment_key,
        "source": source,
        "sentence_id": sentence_id,
    }


def _seed_discovery_segments(
    journal: Path,
    records: list[dict[str, Any]],
    *,
    settings: dict[tuple[str, str, str], str] | None = None,
) -> None:
    for record in records:
        day = record.get("day")
        stream = record.get("stream")
        segment_key = record.get("segment_key")
        if not all(isinstance(value, str) for value in (day, stream, segment_key)):
            continue
        segment_dir = journal / "chronicle" / day / stream / segment_key
        segment_dir.mkdir(parents=True, exist_ok=True)
        setting = (settings or {}).get((day, stream, segment_key))
        if setting is not None:
            (segment_dir / "imported_audio.jsonl").write_text(
                json.dumps({"setting": setting}) + "\n",
                encoding="utf-8",
            )


def _write_discovery_cache_records(
    journal: Path,
    clusters: dict[str, list[dict[str, Any]]],
    *,
    settings: dict[tuple[str, str, str], str] | None = None,
) -> None:
    cache_path = journal / "awareness" / "discovery_clusters.json"
    cache_path.parent.mkdir(parents=True, exist_ok=True)
    for records in clusters.values():
        _seed_discovery_segments(journal, records, settings=settings)
    cache_path.write_text(
        json.dumps(
            {
                "version": "2026-07-20T00:00:00",
                "clusters": clusters,
            }
        )
        + "\n",
        encoding="utf-8",
    )


def _write_discovery_cache(journal: Path, triples: list[tuple[str, str, str]]) -> None:
    _write_discovery_cache_records(
        journal,
        {
            "1": [
                _discovery_record(day, stream, segment_key)
                for day, stream, segment_key in triples
            ]
        },
    )


def _degraded_capture(
    *,
    name: str = "fedora",
    active_count=79,
    first_ts=None,
    include_rejection: bool = True,
) -> dict:
    observer = {
        "name": name,
        "status": "degraded",
    }
    if include_rejection:
        observer["ingest_rejection"] = {
            "reason_code": "ingest_contract_invalid",
            "active_count": active_count,
            "first_ts": _june_22_ms() if first_ts is None else first_ts,
            "latest_ts": _june_22_ms(),
            "summary": "/tmp/private/screen.jsonl:2: value is invalid",
            "stream": "fedora",
            "version": "0.3.1",
            "segment": "20260622/120000_300",
        }
    return {"status": "degraded", "observers": [observer]}


def test_classify_needs_you_locked_shape_and_order():
    attention = {"placeholder_text": "Pipeline needs review"}
    pulse_needs = ["Review the launch checklist"]
    items = classify_needs_you(attention, pulse_needs)

    assert [item.text for item in items] == [
        "Pipeline needs review",
        "Review the launch checklist",
    ]
    assert [field.name for field in fields(NeedsYouItem)] == [
        "text",
        "kind",
        "payload",
        "disabled",
        "reason",
    ]
    for item in items:
        data = item.to_dict()
        assert list(data) == ["text", "kind", "payload", "disabled", "reason"]
        assert data["kind"] in ["chat", "confirm", "route"]
        assert data["disabled"] is False
        assert data["reason"] == ""


def test_needs_dedup_key_same_source_identity_matches_across_text():
    source = "sol://20260313/archon/091500_300"

    assert needs_dedup_key(
        {"text": "Q3 report needs your review", "source_id": source}
    ) == needs_dedup_key({"text": "Look at the Q3 numbers", "source_id": source})


def test_needs_dedup_key_identity_beats_identical_text():
    source_a = "sol://20260313/archon/091500_300"
    source_b = "sol://facets/work/news/20260326"

    assert needs_dedup_key(
        {"text": "Review the report", "source_id": source_a}
    ) != needs_dedup_key({"text": "Review the report", "source_id": source_b})


def test_needs_dedup_key_source_id_matches_inline_sol_ref():
    source = "sol://20260313/archon/091500_300"

    assert needs_dedup_key({"text": "whatever", "source_id": source}) == source
    assert needs_dedup_key(f"Look at the numbers {source}") == source


def test_needs_dedup_key_legacy_strings_normalize_text():
    assert needs_dedup_key("Follow up  with Acme") == needs_dedup_key(
        "follow up with acme"
    )


def test_format_degraded_capture_line_returns_flat_sentence():
    line = format_degraded_capture_line(_degraded_capture())

    assert line == "one of your devices isn't reaching your journal."
    assert "segment" not in line
    assert "screen.jsonl" not in line
    assert "/tmp/private" not in line
    assert "fedora" not in line


def test_format_degraded_capture_line_multiple_stays_flat():
    capture = _degraded_capture()
    capture["observers"].append(
        {
            "name": "phone",
            "status": "degraded",
            "ingest_rejection": {
                "reason_code": "ingest_contract_invalid",
                "active_count": 2,
                "first_ts": _june_22_ms(),
                "latest_ts": _june_22_ms(),
                "summary": "screen.jsonl:2: value is invalid",
                "stream": "phone",
                "version": None,
            },
        }
    )

    assert format_degraded_capture_line(capture) == (
        "one of your devices isn't reaching your journal."
    )


def test_format_degraded_capture_line_ignores_missing_count_or_date():
    missing_ts = _degraded_capture()
    del missing_ts["observers"][0]["ingest_rejection"]["first_ts"]
    assert (
        format_degraded_capture_line(missing_ts)
        == "one of your devices isn't reaching your journal."
    )
    assert (
        format_degraded_capture_line(_degraded_capture(active_count=None))
        == "one of your devices isn't reaching your journal."
    )


def test_format_degraded_capture_line_fallbacks_and_non_degraded():
    assert (
        format_degraded_capture_line(_degraded_capture(include_rejection=False))
        == "one of your devices isn't reaching your journal."
    )
    assert (
        format_degraded_capture_line({"status": "degraded", "observers": []})
        == "one of your devices isn't reaching your journal."
    )
    assert format_degraded_capture_line({"status": "active", "observers": []}) is None


def test_classify_needs_you_no_longer_emits_capture_route():
    items = classify_needs_you({"placeholder_text": "x"}, ["y"])

    assert all(item.payload != {"href": "/app/health"} for item in items)


def test_classify_needs_you_warns_and_omits_malformed(caplog):
    caplog.set_level("WARNING", logger="solstone.apps.home.needs_you")

    items = classify_needs_you(
        None,
        [None, ""],
    )

    assert items == []
    assert any(
        "omitting malformed needs-you" in record.message for record in caplog.records
    )


def test_classify_needs_you_route_same_origin_only(caplog):
    caplog.set_level("WARNING", logger="solstone.apps.home.needs_you")

    route_items = classify_needs_you(
        None,
        [
            {
                "text": "Open the settings page",
                "kind": "route",
                "payload": {"href": "/app/settings"},
            }
        ],
    )

    assert route_items == [
        NeedsYouItem(
            text="Open the settings page",
            kind="route",
            payload={"href": "/app/settings"},
        )
    ]
    assert _normalize_route_payload({"href": "/app/foo"}) == {"href": "/app/foo"}
    assert _normalize_route_payload({"href": "//evil.com/foo"}) is None
    assert _normalize_route_payload({"href": "https://evil.com"}) is None
    assert any("off-origin href" in record.message for record in caplog.records)




def test_owner_voice_needs_are_retired() -> None:
    from solstone.apps.home.owner_voice import build_owner_voice_needs

    assert build_owner_voice_needs("20260720") == []


def test_classify_needs_you_invalid_route_returns_disabled_item():
    items = classify_needs_you(
        None,
        [
            {
                "text": "Open the offsite link",
                "kind": "route",
                "payload": {"href": "https://evil.com"},
            }
        ],
    )

    assert items == [
        NeedsYouItem(
            text="Open the offsite link",
            kind="route",
            payload={},
            disabled=True,
            reason=DISABLED_INVALID_ROUTE_REASON,
        )
    ]


def test_chat_item_with_empty_prompt_returns_disabled_item():
    assert _chat_item("Review this", " ") == NeedsYouItem(
        text="Review this",
        kind="chat",
        payload={},
        disabled=True,
        reason=DISABLED_EMPTY_PROMPT_REASON,
    )


def test_classify_needs_you_folds_confirm_to_chat():
    items = classify_needs_you(
        None,
        [{"text": "Confirm the next step", "kind": "confirm", "payload": {}}],
    )

    assert items == [
        NeedsYouItem(
            text="Confirm the next step",
            kind="chat",
            payload={"prompt": "let's dig into Confirm the next step"},
        )
    ]


def _home_render_js() -> str:
    return (
        Path(__file__).resolve().parents[1]
        / "solstone"
        / "apps"
        / "home"
        / "static"
        / "home.js"
    ).read_text(encoding="utf-8")


def test_unknown_kind_renders_inert():
    render_js = _home_render_js()

    dispatch_start = render_js.index("function dispatchNeedsYouItem(item)")
    # The dispatch body runs until the next top-level function in the module.
    next_fn = render_js.index("\n  function ", dispatch_start + 1)
    dispatch_body = render_js[dispatch_start:next_fn]

    assert "if (item.kind === 'chat')" in dispatch_body
    assert "if (item.kind === 'route')" in dispatch_body
    assert "if (item.kind === 'confirm')" in dispatch_body
    assert "unsupported confirm needs-you item" in dispatch_body
    # No catch-all else — an unknown kind falls through inert.
    assert "else" not in dispatch_body


def test_disabled_items_render_noninteractive():
    render_js = _home_render_js()

    # The disabled needs-you item renders client-side with the reason and no
    # interactive affordances; the interactive path carries them.
    assert "pulse-needs-item-disabled" in render_js
    assert "pulse-needs-reason" in render_js
    disabled_start = render_js.index("if (item && item.disabled)")
    disabled_end = render_js.index(
        "return", render_js.index("return", disabled_start) + 1
    )
    disabled_branch = render_js[disabled_start:disabled_end]
    assert 'role="button"' not in disabled_branch
    assert "tabindex" not in disabled_branch
    assert "data-needs-you-item" not in disabled_branch
    # Dispatch is a no-op for a disabled item.
    assert "if (item.disabled) return;" in render_js
