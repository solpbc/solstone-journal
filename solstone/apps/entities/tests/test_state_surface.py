# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import re
from pathlib import Path

import pytest


@pytest.fixture(scope="module")
def html() -> str:
    return Path("solstone/apps/entities/workspace.html").read_text(encoding="utf-8")


def _function_body(text, name):
    try:
        start = text.index(f"function {name}(")
    except ValueError:
        start = text.index(f"async function {name}(")
    boundary = re.search(r"\n(?:async\s+)?function\s+", text[start + 1 :])
    nxt = start + 1 + boundary.start() if boundary else len(text)
    return text[start:nxt]


def _css_rule(text: str, selector: str) -> str:
    start = f"\n{selector} {{"
    assert text.count(start) == 1, f"expected exactly one CSS rule for {selector}"
    start_idx = text.index(start) + len(start)
    assert "}" in text[start_idx:], f"missing end of CSS rule for {selector}"
    end_idx = text.index("}", start_idx)
    return text[start_idx:end_idx]


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


def test_observation_empty_and_failure_states(html):
    detail = _function_body(html, "renderDetailView")
    observation_list = _function_body(html, "renderObservationList")
    show = _function_body(html, "showFacetDetailView")
    catch_body = show.split(".catch(error => {", 1)[1]

    assert (
        "renderObservationList(entity, detailObservationState.observations);" in detail
    )
    assert "ENT_COPY.ENT_OBS_EMPTY.replace('{name}', entity.name)" in observation_list

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


def test_observation_day_grid_surface_and_state_hooks(html):
    hide = _function_body(html, "hideObservationDayGrid")
    show = _function_body(html, "showFacetDetailView")
    render_grid = _function_body(html, "renderObservationDayGridForState")
    apply_grid = _function_body(html, "applyObservationDayGrid")
    load_grid = _function_body(html, "loadObservationDayGrid")
    range_setter = _function_body(html, "setObservationGridRange")
    range_clear = _function_body(html, "clearObservationGridRange")
    observations = _function_body(html, "observationsForDetailState")
    state_render = _function_body(html, "renderDetailObservationsForState")
    detail = _function_body(html, "renderDetailView")

    card = html.index('id="detail-observation-grid-card"')
    observations_mount = html.index('id="detail-observations"')

    assert card < observations_mount
    assert 'id="detail-observations-heading"' in html
    assert (
        "document.getElementById('detail-observations-heading').textContent = ENT_COPY.ENT_OBS_HEADING || '';"
        in show
    )

    assert "loadObservationDayGrid(currentFacet, entityId, openToken);" in show
    assert (
        "renderObservationList(entity, detailObservationState.observations);" in detail
    )
    assert "if (detailObservationState.gridData)" in detail
    assert "renderObservationDayGridForState();" in detail

    assert "data: null" in hide
    assert "mode: 'select'" in hide
    assert "onRange: setObservationGridRange" in hide
    assert (
        "window.DayGrid.gate(data, { minSpanDays: 70, minActiveDays: 6 })" in apply_grid
    )
    assert "window.DayGrid.legend(legend, { unit, data });" in render_grid
    assert "onRange: setObservationGridRange" in render_grid
    assert "onSelect:" not in html
    assert "onDay:" not in html

    assert "try {" in load_grid
    assert "fetch(" in load_grid
    assert "catch (_error)" in load_grid
    assert "resetObservationGridContext();" in load_grid
    assert "detailObservationTokenCurrent(token)" in load_grid

    assert "detailObservationState.mode = 'range';" in range_setter
    assert "if (!range)" in range_setter
    assert "clearObservationGridRange();" in range_setter
    assert "detailObservationState.mode = 'default';" in range_clear
    assert "day && day >= range.from && day <= range.to" in observations
    assert "rangeClear.onclick = clearObservationGridRange;" in html
    assert "showAllButton.onclick = showAllDetailObservations;" in state_render


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


def test_connection_mounts_and_renderers_are_shared(html):
    journal = _function_body(html, "renderJournalDetailView")
    detail = _function_body(html, "renderDetailView")
    state = _function_body(html, "loadEntitiesState")

    journal_facets = html.index('id="journal-detail-facets"')
    journal_connections = html.index('id="journal-detail-connections"')
    journal_edit = html.index('id="journal-detail-edit-error"')
    detail_description = html.index('id="description-save-error"')
    detail_connections = html.index('id="detail-connections"')
    detail_observations = html.index('id="detail-observations"')

    assert journal_facets < journal_connections < journal_edit
    assert detail_description < detail_connections < detail_observations
    assert "<h4>connections</h4>" in html

    assert "const isPrincipal = entity.is_principal === true;" in journal
    assert "const isPrincipal = entity.is_principal === true;" in detail
    assert (
        "renderConnectionsSection(entity.id, 'journal-detail-connections', "
        "isPrincipal);"
    ) in journal
    assert (
        "renderConnectionsSection(entity.id, 'detail-connections', isPrincipal);"
        in detail
    )
    assert "ATTENDANCE_KINDS = new Set(state.attendance_kinds || []);" in state


def test_connection_network_states_are_statically_distinguishable(html):
    section = _function_body(html, "renderConnectionsSection")
    unavailable = _function_body(html, "renderConnectionsIndexUnavailable")
    failed = _function_body(html, "renderConnectionsLoadFailed")

    assert (
        "container.innerHTML = '<span class=\"observations-empty\">loading...</span>';"
        in section
    )
    assert "const network = await loadEntityNetwork(entityId);" in section
    assert "if (network.resolved === null)" in section
    assert "renderConnectionsAmbiguous(container);" in section
    assert "const referenceDay = network.reference_day || '';" in section
    assert (
        "renderConnectionsNetwork(container, entityId, neighbors, referenceDay, isPrincipal)"
        in section
    )
    assert "if (!isPrincipal && withYouSlot)" in section
    assert "const history = await loadWithYouHistory(entityId);" in section
    assert "renderWithYouFailure(withYouSlot);" in section
    assert "neighbors.length === 0 && !hasWithYou" in section
    assert "renderConnectionsEmpty(container);" in section
    assert "error?.body?.reason_code === 'edge_index_unavailable'" in section
    assert "renderConnectionsIndexUnavailable(container);" in section
    assert (
        "renderConnectionsLoadFailed(container, entityId, containerId, isPrincipal);"
        in section
    )

    assert "ENT_COPY.ENT_CONN_INDEX_UNAVAILABLE" in unavailable
    assert "link.href = '/app/health';" in unavailable
    assert "ENT_COPY.ENT_CONN_INDEX_ACTION" in unavailable
    assert "SurfaceState.error" not in unavailable

    assert "container.innerHTML = window.SurfaceState.error({" in failed
    assert "heading: ENT_COPY.ENT_CONN_LOAD_FAILED" in failed
    assert "retry: true" in failed
    assert "reportable: false" in failed
    assert "headingLevel: 'h3'" in failed
    assert "container.querySelector('.surface-state-retry')" in failed
    assert (
        "retryBtn.onclick = () => renderConnectionsSection(entityId, "
        "containerId, isPrincipal);"
    ) in failed


def test_connection_fetch_urls_and_reference_day_threading(html):
    network = _function_body(html, "loadEntityNetwork")
    with_you = _function_body(html, "loadWithYouHistory")
    history = _function_body(html, "loadConnectionHistory")
    with_you_block = _function_body(html, "renderWithYouBlock")
    initial = _function_body(html, "loadInitialConnectionEvidence")
    pane = _function_body(html, "renderEvidencePane")
    actions = _function_body(html, "renderEvidenceActions")

    assert "entity: entityId" in network
    assert "limit: '15'" in network
    assert "evidence_limit: '1'" in network
    assert "`/app/entities/api/network?${query.toString()}`" in network

    assert "entity: entityId" in with_you
    assert "limit: String(ENTITY_CONN_PAGE_SIZE)" in with_you
    assert "`/app/entities/api/history?${query.toString()}`" in with_you

    assert "entity: entityId" in history
    assert "peer: peerId" in history
    assert "offset: String(offset || 0)" in history

    assert "const summaryTemplate = total === 1" in with_you_block
    assert "? ENT_COPY.ENT_CONN_WITH_YOU_SUMMARY_ONE" in with_you_block
    assert ": ENT_COPY.ENT_CONN_WITH_YOU_SUMMARY" in with_you_block
    assert ".replace('{kind}', entityConnKindWord(latest.kind))" in with_you_block
    assert (
        ".replace('{day}', formatEntityConnDay(latest.day, referenceDay))"
        in with_you_block
    )

    assert (
        "renderEvidencePane(pane, entityId, peerId, history, referenceDay, true);"
        in initial
    )
    assert "appendEvidenceRows(list, history?.evidence || [], referenceDay);" in pane
    assert (
        "renderEvidenceActions(pane, entityId, peerId, referenceDay, showViewLink);"
        in pane
    )
    assert (
        "appendEvidenceRows(pane.querySelector('.entity-conn-evidence-list'), rows, referenceDay);"
        in actions
    )
    assert "pane.dataset.offset = String(offset + rows.length);" in actions
    assert "navigateToEntity(peerId);" in actions


def test_connection_async_writes_are_current_entity_guarded(html):
    guard = _function_body(html, "isCurrentConnectionEntity")
    section = _function_body(html, "renderConnectionsSection")
    initial = _function_body(html, "loadInitialConnectionEvidence")
    actions = _function_body(html, "renderEvidenceActions")
    facet_show = _function_body(html, "showFacetDetailView")
    journal_show = _function_body(html, "showJournalDetailView")

    assert "return currentDetailEntity?.id === entityId;" in guard
    assert "currentDetailEntity = null;" in facet_show
    assert "currentDetailEntity = null;" in journal_show
    assert section.count("if (!isCurrentConnectionEntity(entityId)) return;") >= 3
    assert initial.count("if (!isCurrentConnectionEntity(entityId)) return;") == 2
    assert actions.count("if (!isCurrentConnectionEntity(entityId)) return;") == 2


def test_connection_evidence_trailing_controls_are_cleared(html):
    clear = _function_body(html, "clearEvidenceTrailingControls")
    actions = _function_body(html, "renderEvidenceActions")
    failed = _function_body(html, "renderEvidenceFetchFailed")

    assert "pane.querySelector('.entity-conn-evidence-actions')?.remove();" in clear
    assert "pane.querySelector('.entity-conn-evidence-failed')?.remove();" in clear
    assert "clearEvidenceTrailingControls(pane);" in actions
    assert "clearEvidenceTrailingControls(pane);" in failed
    assert (
        "pane.querySelector('.entity-conn-evidence-actions')?.remove();" not in actions
    )
    assert "pane.querySelector('.entity-conn-evidence-failed')?.remove();" not in failed


def test_connection_singular_row_meta_keeps_day_label(html):
    meta = _function_body(html, "entityConnRowMeta")

    assert "const day = formatEntityConnDay(neighbor.last_seen, referenceDay);" in meta
    assert "if (count === 1)" in meta
    assert "ENT_COPY.ENT_CONN_ROW_META_ONE_ATTENDANCE" in meta
    assert "ENT_COPY.ENT_CONN_ROW_META_ONE" in meta
    assert ".replace('{day}', day);" in meta


def test_connection_formatter_and_label_fallback(html):
    formatter = _function_body(html, "formatEntityConnDay")
    label = _function_body(html, "entityConnEvidenceLabel")
    rows = _function_body(html, "appendEvidenceRows")

    assert "parseYYYYMMDD(day)" in formatter
    assert "parseYYYYMMDD(referenceDay)" in formatter
    assert "{ month: 'short', day: 'numeric' }" in formatter
    assert "date.getFullYear() !== reference.getFullYear()" in formatter
    assert 'label += " \'" + String(date.getFullYear()).slice(-2);' in formatter
    assert "ENT_COPY.ENT_CONN_UPCOMING" in formatter
    assert "formatDateShort" not in formatter

    assert "String(rawLabel).trim()" in label
    assert "entityConnCopyMap('ENT_CONN_LABEL_FALLBACKS')" in label
    assert "fallbacks[row?.kind] || entityConnKindWord(row?.kind)" in label
    assert "path" not in label
    assert "anchor" not in label

    assert "date.href = `/app/thinking/#runs/${row.day}`;" in rows
    assert "date.textContent = formatEntityConnDay(row.day, referenceDay);" in rows
    assert "item.classList.add('entity-conn-evidence-row-upcoming');" in rows
    assert "label.textContent = entityConnEvidenceLabel(row);" in rows
    assert "source.textContent = entityConnSourceWord(row.source);" in rows
    assert "innerHTML" not in rows


def test_connection_attendance_chip_and_copy_maps(html):
    chips = _function_body(html, "renderConnectionKindChips")
    kind_word = _function_body(html, "entityConnKindWord")
    chip_word = _function_body(html, "entityConnChipWord")
    source_word = _function_body(html, "entityConnSourceWord")

    assert "neighbor.evidence_class === 'attendance'" in chips
    assert "ENT_COPY.ENT_CONN_EVENTS_ONLY" in chips
    assert ".filter(kind => !ATTENDANCE_KINDS.has(kind))" in chips
    assert "return rightWeight - leftWeight || left.localeCompare(right);" in chips
    assert ".slice(0, 3)" in chips

    assert "entityConnCopyMap('ENT_CONN_KIND_WORDS')" in kind_word
    assert "entityConnCopyMap('ENT_CONN_KIND_CHIP_WORDS')" in chip_word
    assert "chips[kind] || entityConnKindWord(kind)" in chip_word
    assert "entityConnCopyMap('ENT_CONN_SOURCE_WORDS')" in source_word
    assert "ENT_COPY.ENT_CONN_SOURCE_WORD_FALLBACK" in source_word
