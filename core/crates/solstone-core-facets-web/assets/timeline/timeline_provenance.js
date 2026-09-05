// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function (global) {
  'use strict';

  // --- owner-facing strings ---
  const PROVENANCE_TITLE_PREFIX = 'rolled up at';
  const PROVENANCE_TITLE_ON = 'on';
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

  function absoluteRollupTitle(generatedAtMs) {
    const date = new Date(generatedAtMs);
    const hh = pad2(date.getHours());
    const mm = pad2(date.getMinutes());
    const y = date.getFullYear();
    const mo = pad2(date.getMonth() + 1);
    const da = pad2(date.getDate());
    return `${PROVENANCE_TITLE_PREFIX} ${hh}:${mm} ${PROVENANCE_TITLE_ON} ${y}-${mo}-${da}`;
  }

  function renderArtifactTruth(status, generatedAtMs, provenance, artifactOutcome) {
    const normalized = ["current", "stale", "missing"].includes(status) ? status : "stale";
    const label = normalized === "current" ? "current" : normalized === "stale" ? "refresh needed" : "missing";
    const model = provenance?.model ? ` · ${provenance.model}` : "";
    const generated = generatedAtMs ? ` · ${absoluteRollupTitle(generatedAtMs)}` : "";
    // Only show the outcome in parens when it says something the status
    // badge doesn't already say -- status "missing" with outcome "missing"
    // has no more specific reason to add, and rendered as "MISSING (missing)".
    const reason = artifactOutcome && artifactOutcome !== "current" && artifactOutcome !== normalized
      ? ` (${artifactOutcome.replaceAll("_", " ")})`
      : "";
    const recovery = normalized === "current"
      ? ""
      : '<a class="timeline-truth-action" href="/app/health">refresh timeline in system health →</a>';
    return `<div class="timeline-truth timeline-truth-${normalized}" role="status">
      <span class="timeline-truth-badge">${escapeHtml(label)}</span>
      <span class="timeline-truth-detail">${escapeHtml(`${generated}${model}${reason}`.replace(/^ · /, ""))}</span>
      ${recovery}
    </div>`;
  }

  const TimelineProvenance = {
    absoluteRollupTitle,
    renderArtifactTruth,
  };
  global.TimelineProvenance = TimelineProvenance;
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = TimelineProvenance;
  }
})(typeof window !== 'undefined' ? window : globalThis);
