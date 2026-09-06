// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  'use strict';

  // --- owner-facing strings ---
  // "consistency" deliberately overlaps with SPK_OVERVIEW_OWNER_COHESION_LABEL:
  // both name intra_cosine_p25, but this static module cannot share that payload
  // key without tripping the speakers html payload-key test.
  const copy = Object.freeze({
    drawer_label: 'why not yet?',
    na: 'n/a',
    source_label: 'source:',
    manual_tags_label: 'Manual tags:',
    segments_label: 'Segments with audio:',
    embeddings_label: 'Embeddings:',
    reasons: Object.freeze({
      too_few_stmts: Object.freeze({
        row_label: 'statements',
        next: 'tag more clear longer statements, then build from manual tags.'
      }),
      median_duration_too_short: Object.freeze({
        row_label: 'median length',
        next: 'tag longer statements, then build from manual tags.'
      }),
      cluster_too_diffuse: Object.freeze({
        row_label: 'consistency',
        next: 'tag a steadier set of owner statements, then build from manual tags.'
      })
    }),
    next_ready: 'manual tags are ready; build from manual tags to save the voice profile.',
  });
  // --- end owner-facing strings ---

  const DRAWER_ID = 'speakers-owner-gate-diagnostics';

  function escapeHtml(value) {
    return String(value ?? '').replace(/[&<>"']/g, (char) => ({
      '&': '&amp;',
      '<': '&lt;',
      '>': '&gt;',
      '"': '&quot;',
      "'": '&#39;'
    })[char]);
  }

  function observedMetric(value) {
    return typeof value === 'number' && Number.isFinite(value) ? value : null;
  }

  function thresholdMetric(value) {
    const number = observedMetric(value);
    return number !== null && number > 0 ? number : null;
  }

  function formatMetric(value) {
    if (value === null) return copy.na;
    if (Number.isInteger(value)) return String(value);
    return value.toFixed(2);
  }

  function formatWithUnit(value, unit) {
    const text = formatMetric(value);
    return text === copy.na ? text : `${text}${unit}`;
  }

  function countText(value) {
    return String(value || 0);
  }

  function reasonConfig(reason) {
    return copy.reasons[reason] || null;
  }

  function lineFor(reason, observed, threshold) {
    if (reason === 'too_few_stmts') {
      const base = `transcribed in ${formatMetric(observed)} longer statements`;
      return threshold === null ? base : `${base}, needs ${formatMetric(threshold)}`;
    }
    if (reason === 'median_duration_too_short') {
      const base = `median statement length ${formatWithUnit(observed, 's')}`;
      return threshold === null
        ? base
        : `${base}, needs ${formatWithUnit(threshold, 's')}`;
    }
    if (reason === 'cluster_too_diffuse') {
      return 'voice pattern is still too spread out';
    }
  }

  function metricValueFor(reason, observed, threshold) {
    if (reason === 'median_duration_too_short') {
      return {
        observed: formatWithUnit(observed, 's'),
        threshold: threshold === null ? '' : formatWithUnit(threshold, 's')
      };
    }
    return {
      observed: formatMetric(observed),
      threshold: threshold === null ? '' : formatMetric(threshold)
    };
  }

  function gateRowHtml(reason, observed, threshold) {
    const config = reasonConfig(reason);
    if (!config) return '';
    const values = metricValueFor(reason, observed, threshold);
    const needHtml = values.threshold
      ? ` <span class="gate-need">needs ${escapeHtml(values.threshold)}</span>`
      : '';
    return '<div class="gate-row">' +
      `<span>${escapeHtml(config.row_label)}</span>` +
      `<span>${escapeHtml(values.observed)}${needHtml}</span>` +
      '</div>';
  }

  function barHtml(observed, threshold) {
    if (observed === null || threshold === null) return '';
    const percent = Math.max(0, Math.min(100, (observed / threshold) * 100));
    return `<div class="gate-bar"><span style="width:${percent.toFixed(2)}%"></span></div>`;
  }

  function nextLineHtml(reason, canBuild) {
    if (canBuild === true) {
      return `<p class="gate-next">${escapeHtml(copy.next_ready)}</p>`;
    }
    const config = reasonConfig(reason);
    return `<p class="gate-next">${escapeHtml(config.next)}</p>`;
  }

  function bodyHtml(data, reason, observed, threshold, actionHtml) {
    return gateRowHtml(reason, observed, threshold) +
      barHtml(observed, threshold) +
      `<div class="spk-owner-diagnostics-line">${escapeHtml(copy.source_label)} ${escapeHtml(data.source || 'auto')}</div>` +
      `<div class="spk-owner-diagnostics-line">${escapeHtml(copy.manual_tags_label)} ${escapeHtml(countText(data.manual_tags_count))}</div>` +
      `<div class="spk-owner-diagnostics-line">${escapeHtml(copy.segments_label)} ${escapeHtml(countText(data.segments_available))}</div>` +
      `<div class="spk-owner-diagnostics-line">${escapeHtml(copy.embeddings_label)} ${escapeHtml(countText(data.embeddings_available))}</div>` +
      nextLineHtml(reason, data.can_build_from_tags) +
      (actionHtml || '');
  }

  function render(data, options) {
    const payload = data || {};
    const reason = String(payload.low_quality_reason || '');
    const config = reasonConfig(reason);
    if (!config) return '';
    const observed = observedMetric(payload.observed_value);
    const threshold = thresholdMetric(payload.threshold_value);
    const line = lineFor(reason, observed, threshold);
    const actionHtml = String((options || {}).actionHtml || '');

    return window.Drawer.render({
      id: DRAWER_ID,
      label: copy.drawer_label,
      line,
      bodyHtml: bodyHtml(payload, reason, observed, threshold, actionHtml)
    });
  }

  window.GateDrawer = Object.freeze({ render });
})();
