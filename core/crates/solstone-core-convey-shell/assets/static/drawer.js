// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  'use strict';

  // --- owner-facing strings ---
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

  function formatLine(line) {
    const value = String(line ?? '').trim();
    return value
      ? escapeHtml(value).replace(/&#?\w+;|\d[\d,.:]*(?:\s?(?:am|pm|s))?/g, (m) => (m.startsWith('&') ? m : `<b>${m}</b>`))
      : '';
  }

  function render(options) {
    const config = options || {};
    const id = String(config.id ?? '').trim();
    const idAttr = id ? ` data-drawer-id="${escapeHtml(id)}"` : '';
    const openAttr = config.open ? ' open' : '';
    const line = formatLine(config.line);
    const lineHtml = line ? `<span class="drawer-line">${line}</span>` : '';
    const chipText = String(config.chipText ?? '').trim();
    const chipTone = config.chipTone === 'warn' || config.chipTone === 'danger'
      ? ` drawer-chip--${config.chipTone}`
      : '';
    const chipHtml = chipText
      ? `<span class="drawer-chip${chipTone}">${escapeHtml(chipText)}</span>`
      : '';

    return `<details class="drawer"${idAttr}${openAttr}>` +
      '<summary>' +
      '<span class="drawer-chev"></span>' +
      `<span class="drawer-summary-text"><span class="drawer-label">${escapeHtml(config.label ?? '')}</span>${lineHtml}</span>` +
      chipHtml +
      '</summary>' +
      `<div class="drawer-body">${config.bodyHtml || ''}</div>` +
      '</details>';
  }

  function preserveOpen(container, rerenderFn) {
    const openIds = new Set();
    if (container) {
      container.querySelectorAll('details.drawer[data-drawer-id][open]').forEach((detail) => {
        openIds.add(detail.getAttribute('data-drawer-id'));
      });
    }

    const result = rerenderFn();

    if (container && openIds.size) {
      container.querySelectorAll('details.drawer[data-drawer-id]').forEach((detail) => {
        if (openIds.has(detail.getAttribute('data-drawer-id'))) {
          detail.open = true;
        }
      });
    }

    return result;
  }

  window.Drawer = Object.freeze({ render, preserveOpen, formatLine });
})();
