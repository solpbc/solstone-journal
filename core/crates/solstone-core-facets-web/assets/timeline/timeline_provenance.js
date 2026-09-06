// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function (global) {
  'use strict';

  // --- owner-facing strings ---
  const STATE_LABEL = {
    current: 'current',
    stale: 'out of date',
    missing: 'not made yet',
    failed: "couldn't be made",
  };
  const STATE_ACTION = {
    stale: 'update this day →',
    missing: 'make this day →',
    failed: 'try again in system health →',
  };
  const ROLLED_UP_PREFIX = 'rolled up';
  const NEWER_MATERIAL_PREFIX = 'newer material arrived after';
  const NEWER_MATERIAL_UNDATED = 'newer material arrived after it was made';
  const PROVENANCE_SUMMARY = 'how this was made';
  const FACT_MODEL = 'model';
  const FACT_ROLLED_UP = 'rolled up';
  const FACT_REASON = 'reason';
  // --- end owner-facing strings ---

  const ACTION_HREF = '/app/health';
  // Outcomes where the artifact itself could not be read or trusted, as
  // opposed to one that is simply behind the material it was made from.
  const UNMAKEABLE_OUTCOMES = ['unreadable', 'malformed', 'invalid', 'state_unavailable'];

  // The day re-renders whole on a live refresh, so the disclosure's open state
  // has to live outside the markup or every refresh would snap it shut.
  let provenanceOpen = false;
  if (typeof document !== 'undefined') {
    // 'toggle' does not bubble; listen in the capture phase.
    document.addEventListener('toggle', (event) => {
      const target = event.target;
      if (target && target.classList && target.classList.contains('timeline-truth-provenance')) {
        provenanceOpen = Boolean(target.open);
      }
    }, true);
  }

  function escapeHtml(value) {
    return String(value ?? '').replace(/[&<>"']/g, (char) => ({
      '&': '&amp;',
      '<': '&lt;',
      '>': '&gt;',
      '"': '&quot;',
      "'": '&#39;'
    })[char]);
  }

  function rollupTimeLabel(generatedAtMs) {
    if (!generatedAtMs) return '';
    return global.JournalFormat.timestamp(generatedAtMs);
  }

  function truthState(status, artifactOutcome) {
    if (status === 'current') return 'current';
    if (status === 'missing') return 'missing';
    if (UNMAKEABLE_OUTCOMES.includes(artifactOutcome)) return 'failed';
    return 'stale';
  }

  // The one line the owner reads first: where this day stands, in plain words.
  function truthDetail(state, artifactOutcome, timeLabel) {
    if (state === 'current') {
      return timeLabel ? `${ROLLED_UP_PREFIX} ${timeLabel}` : '';
    }
    if (state === 'stale' && artifactOutcome === 'digest_mismatch') {
      return timeLabel ? `${NEWER_MATERIAL_PREFIX} ${timeLabel}` : NEWER_MATERIAL_UNDATED;
    }
    return '';
  }

  function renderFact(term, value) {
    return `<div><dt>${escapeHtml(term)}</dt><dd>${escapeHtml(value)}</dd></div>`;
  }

  // Model, exact time and the machine reason are required by the agent-output
  // canon but are not the owner's first line — they live behind a disclosure.
  function renderProvenanceDetails(state, generatedAtMs, provenance, artifactOutcome, timeLabel) {
    const facts = [];
    if (provenance?.model) facts.push(renderFact(FACT_MODEL, provenance.model));
    if (timeLabel) facts.push(renderFact(FACT_ROLLED_UP, timeLabel));
    if (artifactOutcome && artifactOutcome !== 'current' && artifactOutcome !== state) {
      facts.push(renderFact(FACT_REASON, artifactOutcome.replaceAll('_', ' ')));
    }
    if (!facts.length) return '';
    return `<details class="timeline-truth-provenance"${provenanceOpen ? ' open' : ''}>
      <summary>${escapeHtml(PROVENANCE_SUMMARY)}</summary>
      <dl class="timeline-truth-facts">${facts.join('')}</dl>
    </details>`;
  }

  function renderArtifactTruth(status, generatedAtMs, provenance, artifactOutcome) {
    const state = truthState(status, artifactOutcome);
    const timeLabel = rollupTimeLabel(generatedAtMs);
    const detail = truthDetail(state, artifactOutcome, timeLabel);
    const action = STATE_ACTION[state]
      ? `<a class="timeline-truth-action" href="${ACTION_HREF}">${escapeHtml(STATE_ACTION[state])}</a>`
      : '';
    return `<div class="timeline-truth timeline-truth-${state}">
      <p class="timeline-truth-line" role="status">
        <span class="timeline-truth-badge">${escapeHtml(STATE_LABEL[state])}</span>
        ${detail ? `<span class="timeline-truth-detail">${escapeHtml(detail)}</span>` : ''}
        ${action}
      </p>
      ${renderProvenanceDetails(state, generatedAtMs, provenance, artifactOutcome, timeLabel)}
    </div>`;
  }

  const TimelineProvenance = {
    truthState,
    renderArtifactTruth,
  };
  global.TimelineProvenance = TimelineProvenance;
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = TimelineProvenance;
  }
})(typeof window !== 'undefined' ? window : globalThis);
