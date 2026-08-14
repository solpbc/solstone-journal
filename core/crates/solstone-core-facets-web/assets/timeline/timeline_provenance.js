// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function (global) {
  'use strict';

  // --- owner-facing strings ---
  const PROVENANCE_PREFIX = 'rolled up';
  const PROVENANCE_AGO = 'ago';
  const PROVENANCE_TITLE_PREFIX = 'rolled up at';
  const PROVENANCE_TITLE_ON = 'on';
  const PROVENANCE_SEPARATOR = ' · ';
  // --- end owner-facing strings ---

  function escapeHtml(value) {
    return String(value ?? '').replace(/[&<>"']/g, (char) => ({
      '&': '&amp;',
      '<': '&lt;',
      '>': '&gt;',
      '"': '&quot;',
      "'": '&#39;'
    })[char]);
  }

  function pad2(value) {
    return String(value).padStart(2, '0');
  }

  function absoluteRollupTitle(generatedAt) {
    const date = new Date(generatedAt * 1000);
    const hh = pad2(date.getHours());
    const mm = pad2(date.getMinutes());
    const y = date.getFullYear();
    const mo = pad2(date.getMonth() + 1);
    const da = pad2(date.getDate());
    return `${PROVENANCE_TITLE_PREFIX} ${hh}:${mm} ${PROVENANCE_TITLE_ON} ${y}-${mo}-${da}`;
  }

  function renderDayProvenance(generatedAt, model, nowMs = Date.now()) {
    if (!generatedAt || !model) return "";
    const generatedMs = generatedAt * 1000;
    const relative = global.relativeTime(nowMs - generatedMs);
    const text = `${PROVENANCE_PREFIX} ${relative} ${PROVENANCE_AGO}${PROVENANCE_SEPARATOR}${model}`;
    const title = absoluteRollupTitle(generatedAt);
    return `<p class="timeline-day-provenance" title="${escapeHtml(title)}">${escapeHtml(text)}</p>`;
  }

  const TimelineProvenance = {
    absoluteRollupTitle,
    renderDayProvenance,
  };
  global.TimelineProvenance = TimelineProvenance;
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = TimelineProvenance;
  }
})(typeof window !== 'undefined' ? window : globalThis);
