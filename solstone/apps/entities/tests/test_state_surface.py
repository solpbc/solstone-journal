# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from pathlib import Path

import pytest


@pytest.fixture(scope="module")
def html() -> str:
    return Path("solstone/apps/entities/workspace.html").read_text(encoding="utf-8")


def _function_body(text, name):
    start = text.index(f"function {name}(")
    nxt = text.index("\nfunction ", start + 1)
    return text[start:nxt]


def test_detected_empty_and_no_match_states(html):
    fn = _function_body(html, "renderDetectedTable")

    assert (
        'id="no-detected-entities" class="no-entities" style="display: none;"' in html
    )
    assert (
        'id="no-detected-matches" class="no-card-matches" style="display: none;">no entities match your search.</div>'
        in html
    )
    assert "ENT_COPY.ENT_DETECTED_EMPTY" in fn
    assert "noDetectedMatches.style.display = total === 0 ? 'block' : 'none';" in fn
    assert "if (searchTerm) {" in fn


def test_facet_cards_empty_copy_uses_ent_constant(html):
    fn = _function_body(html, "renderEntityCards")

    assert (
        '<div id="no-facet-entities" class="no-entities" style="display: none;"></div>'
        in html
    )
    assert "noEntities.textContent = ENT_COPY.ENT_CARDS_EMPTY;" in fn
    assert "no entities added to this facet yet. star entities below" not in html


def test_observation_empty_and_failure_states(html):
    detail = _function_body(html, "renderDetailView")
    show = _function_body(html, "showFacetDetailView")
    catch_body = show.split(".catch(error => {", 1)[1]

    assert "ENT_COPY.ENT_OBS_EMPTY.replace('{name}', entity.name)" in detail
    assert "'no observations yet.'" not in html

    assert (
        "const obsContainer = document.getElementById('detail-observations');"
        in catch_body
    )
    assert "obsContainer.innerHTML = window.SurfaceState.error({" in catch_body
    assert "heading: ENT_COPY.ENT_OBS_LOAD_FAILED" in catch_body
    assert "retry: true" in catch_body
    assert "reportable: false" in catch_body
    assert "headingLevel: 'h3'" in catch_body
    assert "retryBtn.onclick = () => showFacetDetailView(entityId);" in catch_body
    assert "loading..." not in catch_body


def test_entity_type_grouping_is_normalized_and_shared(html):
    helper = _function_body(html, "groupEntitiesByType")
    journal = _function_body(html, "renderJournalEntities")
    cards = _function_body(html, "renderEntityCards")

    assert html.count("function groupEntitiesByType(entities)") == 1
    assert ".trim().toLowerCase() || 'other'" in helper
    assert "new Map()" in helper
    assert "getTypeOrder().map(norm)" in helper

    assert "groupEntitiesByType(entities).forEach(({label, items}) => {" in journal
    assert "groupEntitiesByType(attached).forEach(({label, items}) => {" in cards
    assert "header.textContent = label;" in journal
    assert "header.textContent = label;" in cards

    assert "const type = entity.type || 'Other';" not in journal
    assert "const type = entity.type || 'Other';" not in cards
    assert "orderedTypes" not in journal
    assert "orderedTypes" not in cards
