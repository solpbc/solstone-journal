# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Static-source assertions for the detach surface; no browser or DOM is exercised."""

from __future__ import annotations

from pathlib import Path

import pytest


@pytest.fixture(scope="module")
def html() -> str:
    return Path("solstone/apps/entities/workspace.html").read_text(encoding="utf-8")


def _function_body(text, name):
    start = text.index(f"function {name}(")
    nxt = text.index("\nfunction ", start + 1)
    return text[start:nxt]


def test_detach_button_neutral_reattach_blue(html):
    fn = _function_body(html, "renderDetailView")
    assert "detachBtn.className = 'btn btn-secondary'" in fn
    assert "showDetachConfirmModal(entity)" in fn
    assert "btn-danger" not in fn
    assert "detachBtn.className = 'btn btn-primary'" in fn
    assert "reattachEntity(entity)" in fn


def test_confirm_gates_delete(html):
    detach = _function_body(html, "detachEntity")
    assert "method: 'DELETE'" in detach
    confirm = _function_body(html, "confirmDetach")
    assert "detachEntity(entity)" in confirm
    close = _function_body(html, "closeDetachConfirmModal")
    assert "fetch(" not in close
    show = _function_body(html, "showDetachConfirmModal")
    assert "ENT_COPY.ENT_DETACH_CONFIRM" in show


def test_detach_uses_id_addressed_url(html):
    detach = _function_body(html, "detachEntity")
    assert (
        "api/${encodeURIComponent(currentFacet)}/entity/${encodeURIComponent(entity.id)}"
        in detach
    )
    assert "body: JSON.stringify" not in detach
    assert "name: entity.name" not in detach


def test_save_description_uses_id_addressed_url(html):
    save = _function_body(html, "saveDescription")
    assert (
        "api/${encodeURIComponent(currentFacet)}/entity/${encodeURIComponent(entity.id)}/description"
        in save
    )
    assert "name: entity.name" not in save
    assert "ok: response.ok" in save
    assert "if (ok)" in save


def test_attach_branches_gate_on_response_ok(html):
    for function_name in [
        "reattachEntity",
        "attachEntityToFacet",
        "attachDetectedEntity",
    ]:
        fn = _function_body(html, function_name)
        assert "ok: response.ok" in fn
        assert "if (ok)" in fn
        assert "if (data.success)" not in fn


def test_success_only_toast(html):
    detach = _function_body(html, "detachEntity")
    success, _, failure = detach.partition("\n  .catch(error")
    assert "notifications.show" in success
    assert "ENT_COPY.ENT_DETACH_DONE" in success
    assert "ENT_COPY.ENT_DETACH_REATTACH_ACTION" in success
    assert "ENT_COPY.ENT_DETACH_FIND_ACTION" in success
    assert "showInlineError('detach-action-error'" in failure
    assert "notifications.show" not in failure


def test_reattach_and_find_navigation(html):
    detach = _function_body(html, "detachEntity")
    assert "navigateToEntity(entityId)" in detach
    assert "window.selectFacet(null)" in detach


def test_journal_row_navigable_when_detached(html):
    fn = _function_body(html, "renderJournalDetailView")
    assert "if (isBlocked) {" in fn
    assert "if (isBlocked || isDetachedFacet)" not in fn
    assert "facet-rel-detached-badge" in fn
    assert "row.classList.add('detached')" in fn
    assert "window.selectFacet(facet.name)" in fn
    assert "row.setAttribute('role', 'button')" in fn


def test_observation_source_day_link(html):
    fn = _function_body(html, "renderObservationItem")
    assert "/app/thinking/#runs/${row.source_day}" in fn
    assert r"/^\d{8}$/.test(row.source_day)" in fn
    assert "ENT_COPY.ENT_OBS_SOURCE_LINK_TITLE" in fn
    assert "dayLink.textContent = formatDateShort(row.source_day)" in fn
    assert "metaParts" not in fn
