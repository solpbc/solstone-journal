// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  'use strict';

  // --- owner-facing strings ---
  // "consistency" deliberately overlaps with SPK_OVERVIEW_OWNER_COHESION_LABEL:
  // both name intra_cosine_p25, but this static module cannot share that payload
  // key without tripping the speakers html payload-key test.
  const copy = Object.freeze({
    drawer_label: window.JournalI18n.render('journal.window.shared.gate.drawer.open.label'),
    na: 'n/a',
    reasons: Object.freeze({
      too_few_stmts: Object.freeze({
        row_label: 'statements',
        next: window.JournalI18n.render('journal.window.shared.gate.drawer.window.explanation.teach.a.few.more.of.your')
      }),
      median_duration_too_short: Object.freeze({
        row_label: window.JournalI18n.render('journal.window.shared.gate.drawer.median.length'),
        next: window.JournalI18n.render('journal.window.shared.gate.drawer.window.explanation.pick.longer.statements.when.you.teach')
      }),
      cluster_too_diffuse: Object.freeze({
        row_label: 'consistency',
        next: window.JournalI18n.render('journal.window.shared.gate.drawer.window.explanation.pick.statements.where.only.you.are')
      })
    }),
    next_ready: window.JournalI18n.render('journal.window.shared.gate.drawer.window.explanation.you.ve.taught.enough.to.save'),
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

  function countArgs(value) {
    const count = value || 0;
    return [['quantity', window.JournalI18n.cardinalQuantity(count)], ['count', String(count)]];
  }

  function reasonConfig(reason) {
    return copy.reasons[reason] || null;
  }

  function lineFor(reason, observed, threshold) {
    if (reason === 'too_few_stmts') {
      const facts = [['quantity', window.JournalI18n.cardinalQuantity(observed)], ['observed', formatMetric(observed)]];
      if (threshold === null) return window.JournalI18n.render('journal.window.shared.gate.drawer.statements.observed.count', facts);
      return window.JournalI18n.render('journal.window.shared.gate.drawer.statements.observed.and.required.count', [
        ...facts, ['minimum_quantity', window.JournalI18n.cardinalQuantity(threshold)], ['minimum', formatMetric(threshold)]
      ]);
    }
    if (reason === 'median_duration_too_short') {
      const id = observed === null
        ? (threshold === null ? 'journal.window.shared.gate.drawer.statement.median.unavailable' : 'journal.window.shared.gate.drawer.statement.median.unavailable.required.seconds')
        : (threshold === null ? 'journal.window.shared.gate.drawer.statement.median.seconds' : 'journal.window.shared.gate.drawer.statement.median.required.seconds');
      const args = [];
      if (observed !== null) args.push(['observed', formatMetric(observed)]);
      if (threshold !== null) args.push(['minimum', formatMetric(threshold)]);
      return window.JournalI18n.render(id, args);
    }
    if (reason === 'cluster_too_diffuse') {
      return window.JournalI18n.render('journal.window.shared.gate.drawer.window.explanation.voice.pattern.is.still.too.spread');
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
      ? ` <span class="gate-need">${window.JournalI18n.renderHtml('journal.window.shared.gate.drawer.threshold', [['threshold', values.threshold]])}</span>`
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
      `<div class="spk-owner-diagnostics-line">${window.JournalI18n.renderHtml('journal.window.shared.gate.drawer.manual.tags.count', countArgs(data.manual_tags_count))}</div>` +
      `<div class="spk-owner-diagnostics-line">${window.JournalI18n.renderHtml('journal.window.shared.gate.drawer.audio.recordings.count', countArgs(data.segments_available))}</div>` +
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
