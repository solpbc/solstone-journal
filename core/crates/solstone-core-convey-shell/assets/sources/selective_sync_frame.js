// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Shared selective-sync frame primitives for `/app/sources/` sync rows and
// `/app/sources/<source>` detail views (used by Email and Photos Library).
//
// Registering a detail view:
//   window.SolSelectiveSync.registerDetail('photos', { mount(ctx, root, frame) })
//
// All DOM construction avoids innerHTML and reads owner copy from JournalI18n.

(function (global) {
  'use strict';

  const details = new Map();

  function t(key, args) {
    return global.JournalI18n.render(key, args || []);
  }

  function formatCount(value) {
    const n = Number(value);
    if (!Number.isFinite(n)) return '0';
    try {
      return new Intl.NumberFormat('en-US').format(Math.trunc(n));
    } catch (_) {
      return String(Math.trunc(n));
    }
  }

  function formatShortDate(iso, timeZone) {
    if (typeof iso !== 'string' || !iso.trim()) return '';
    const ms = Date.parse(iso);
    if (!Number.isFinite(ms)) return '';
    try {
      const opts = { month: 'short', day: 'numeric', year: 'numeric' };
      if (timeZone) opts.timeZone = timeZone;
      return new Intl.DateTimeFormat('en-US', opts).format(new Date(ms)).toLowerCase();
    } catch (_) {
      return new Date(ms).toISOString().slice(0, 10);
    }
  }

  function parseTime(iso) {
    if (typeof iso !== 'string' || iso.trim() === '') return null;
    const parsed = Date.parse(iso);
    return Number.isFinite(parsed) ? parsed : null;
  }

  function ymd(ms, timeZone) {
    const parts = new Intl.DateTimeFormat('en-US', {
      timeZone: timeZone,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
    }).formatToParts(new Date(ms));
    const read = (type) => Number(parts.find((part) => part.type === type).value);
    return { y: read('year'), m: read('month'), d: read('day') };
  }

  function previousDay(y, m, d) {
    const date = new Date(Date.UTC(y, m - 1, d));
    date.setUTCDate(date.getUTCDate() - 1);
    return { y: date.getUTCFullYear(), m: date.getUTCMonth() + 1, d: date.getUTCDate() };
  }

  function relativeTime(value, nowMs, timeZone) {
    const then = typeof value === 'number' ? value : parseTime(value);
    if (then === null) return '';
    const nowRef = typeof nowMs === 'number' ? nowMs : Date.now();
    const tz = timeZone || Intl.DateTimeFormat().resolvedOptions().timeZone;
    const delta = nowRef - then;
    if (delta < 60000) return 'just now';
    const minutes = Math.floor(delta / 60000);
    if (minutes < 60) return minutes === 1 ? '1 min ago' : minutes + ' min ago';
    const hours = Math.floor(delta / 3600000);
    if (hours < 24) return hours === 1 ? '1 hour ago' : hours + ' hours ago';
    const now = ymd(nowRef, tz);
    const day = ymd(then, tz);
    const prev = previousDay(now.y, now.m, now.d);
    if (day.y === prev.y && day.m === prev.m && day.d === prev.d) return 'yesterday';
    const months = [
      t('journal.window.sources.months.label.january'),
      t('journal.window.sources.months.label.february'),
      t('journal.window.sources.months.label.march'),
      t('journal.window.sources.months.label.april'),
      t('journal.window.sources.months.label.may'),
      t('journal.window.sources.months.label.june'),
      t('journal.window.sources.months.label.july'),
      t('journal.window.sources.months.label.august'),
      t('journal.window.sources.months.label.september'),
      t('journal.window.sources.months.label.october'),
      t('journal.window.sources.months.label.november'),
      t('journal.window.sources.months.label.december'),
    ];
    const month = months[day.m - 1];
    if (day.y === now.y) return month + ' ' + day.d;
    return month + ' ' + day.d + ', ' + day.y;
  }

  function el(doc, tag, className, attrs) {
    const node = doc.createElement(tag);
    if (className) node.className = className;
    if (attrs) {
      Object.keys(attrs).forEach((k) => {
        if (attrs[k] !== undefined && attrs[k] !== null) node.setAttribute(k, String(attrs[k]));
      });
    }
    return node;
  }

  function text(doc, tag, className, value, attrs) {
    const node = el(doc, tag, className, attrs);
    node.textContent = value == null ? '' : String(value);
    return node;
  }

  function link(doc, className, label, href, attrs) {
    const node = text(doc, 'a', className, label, attrs);
    node.setAttribute('href', href);
    return node;
  }

  function stateSpec(state) {
    switch (state) {
      case 'on':
        return { cls: 'ss-pill st-on', label: t('journal.window.sources.selective.state.on') };
      case 'setting_up':
        return { cls: 'ss-pill st-setup', label: t('journal.window.sources.selective.state.setting.up') };
      case 'needs_attention':
        return { cls: 'ss-pill st-attn', label: t('journal.window.sources.selective.state.needs.attention') };
      case 'off':
        return { cls: 'ss-pill st-off', label: t('journal.window.sources.selective.state.off') };
      case 'paused':
        return { cls: 'ss-pill st-off', label: t('journal.window.sources.selective.state.paused') };
      case 'unavailable':
        return { cls: 'ss-pill st-off', label: t('journal.window.sources.selective.state.unavailable') };
      case 'ready_to_set_up':
      default:
        return { cls: 'ss-pill', label: t('journal.window.sources.selective.state.ready') };
    }
  }

  function renderStatePill(doc, state) {
    const spec = stateSpec(state);
    const pill = text(doc, 'span', spec.cls, spec.label);
    pill.setAttribute('data-state', state || 'ready_to_set_up');
    return pill;
  }

  function renderLeftOutDisclosure(doc, opts) {
    const fold = el(doc, 'details', 'ss-fold');
    fold.appendChild(text(doc, 'summary', null, opts.summary));
    const body = el(doc, 'div', 'ss-fold-body');
    if (Array.isArray(opts.items) && opts.items.length > 0) {
      const list = el(doc, 'ul', 'ss-reason-list');
      opts.items.forEach((item) => {
        const li = el(doc, 'li', null);
        li.appendChild(text(doc, 'span', null, item.label));
        li.appendChild(text(doc, 'b', null, formatCount(item.count)));
        list.appendChild(li);
      });
      body.appendChild(list);
    }
    if (opts.note) {
      body.appendChild(text(doc, 'p', 'ss-fold-note', opts.note));
    }
    fold.appendChild(body);
    return fold;
  }

  // Shared `/app/sources/` row inside the `syncs` band for selective-sync sources.
  function renderSyncRow(doc, opts) {
    const rowCls = opts.unavailable
      ? 'sources-sync-row sources-sync-unavailable'
      : 'sources-sync-row sources-sync-selective';
    const row = el(doc, 'li', rowCls);
    row.setAttribute('data-sync', opts.id);

    const top = el(doc, 'div', 'ss-sync-top');
    const idBox = el(doc, 'div', 'ss-sync-id');
    idBox.appendChild(text(doc, 'strong', 'sources-sync-name', opts.name));
    if (opts.subtitle) {
      idBox.appendChild(text(doc, 'span', 'ss-sync-acct', opts.subtitle));
    }
    top.appendChild(idBox);
    top.appendChild(renderStatePill(doc, opts.state));
    row.appendChild(top);

    if (opts.statusText || opts.statusLink) {
      const meta = el(doc, 'div', 'ss-sync-meta');
      if (opts.statusText) {
        meta.appendChild(text(doc, 'span', 'sources-sync-status', opts.statusText));
      }
      if (opts.statusLink) {
        if (opts.statusText) meta.appendChild(text(doc, 'span', 'sources-sync-sep', ' · '));
        meta.appendChild(link(doc, 'ss-inline-link', opts.statusLink.label, opts.statusLink.href));
      }
      row.appendChild(meta);
    }

    if (opts.leftOut && opts.leftOut.total > 0) {
      row.appendChild(renderLeftOutDisclosure(doc, opts.leftOut));
    }

    if (opts.actionLabel && opts.actionHref) {
      const actions = el(doc, 'div', 'ss-sync-actions');
      const linkCls = ['ss-action-link', 'sources-sync-action-' + opts.id];
      if (opts.actionTone === 'accent') linkCls.push('accent');
      if (opts.actionTone === 'attn') linkCls.push('attn');
      if (opts.unavailable) linkCls.push('sources-sync-unavailable-' + opts.id);
      actions.appendChild(link(doc, linkCls.join(' '), opts.actionLabel, opts.actionHref));
      row.appendChild(actions);
    }

    return row;
  }

  // Shared `/app/sources/<source>` detail shell card:
  // `< sources` back link, title + optional subtitle + canon state pill, optional lede.
  function renderDetailShell(doc, opts) {
    const card = el(doc, 'section', 'sources-detail-card');
    const backHref = opts.backHref || '/app/sources/';
    const backLabel = opts.backLabel || t('journal.window.sources.selective.back.sources');
    card.appendChild(link(doc, 'ss-back-link', '\u2039 ' + backLabel, backHref));

    const head = el(doc, 'div', 'ss-detail-head');
    const left = el(doc, 'div', 'ss-detail-head-l');
    left.appendChild(text(doc, 'h1', null, opts.title));
    if (opts.subtitle) {
      left.appendChild(text(doc, 'span', 'ss-detail-sub', opts.subtitle));
    }
    head.appendChild(left);
    head.appendChild(renderStatePill(doc, opts.state));
    card.appendChild(head);

    if (opts.lede) {
      card.appendChild(text(doc, 'p', 'ss-detail-lede', opts.lede));
    }

    const body = el(doc, 'div', 'ss-detail-body');
    card.appendChild(body);
    return { card, body };
  }

  function renderNotice(doc, opts) {
    const cls = opts.tone ? 'ss-notice ' + opts.tone : 'ss-notice';
    const role = opts.tone === 'err' ? 'alert' : 'status';
    const box = el(doc, 'div', cls, { role: role, 'aria-live': opts.tone === 'err' ? 'assertive' : 'polite' });
    if (opts.text) {
      box.appendChild(text(doc, 'span', null, opts.text));
    }
    if (Array.isArray(opts.children)) {
      opts.children.forEach((child) => { if (child) box.appendChild(child); });
    }
    return box;
  }

  function renderProgress(doc, opts) {
    const wrap = el(doc, 'div', 'ss-progress');
    const track = el(doc, 'div', opts.paused ? 'ss-prog-track paused' : 'ss-prog-track', {
      role: 'progressbar',
      'aria-valuemin': '0',
      'aria-valuemax': String(Math.max(0, Number(opts.max) || 0)),
      'aria-valuenow': String(Math.max(0, Number(opts.value) || 0)),
    });
    const pct = opts.max > 0 ? Math.min(100, Math.max(0, Math.round((opts.value / opts.max) * 100))) : 0;
    const fill = el(doc, 'div', 'ss-prog-fill');
    if (fill.style) fill.style.width = pct + '%';
    fill.setAttribute('data-pct', String(pct));
    track.appendChild(fill);
    wrap.appendChild(track);
    if (opts.label) {
      wrap.appendChild(text(doc, 'p', 'ss-prog-label', opts.label));
    }
    return wrap;
  }

  // Canonical 5-range picker (`from before today`).
  function renderRangeChooser(doc, opts) {
    const sec = el(doc, 'section', 'ss-choice-sec');
    sec.appendChild(text(doc, 'h2', null, opts.heading || t('journal.window.sources.selective.range.heading')));
    if (opts.intro) {
      sec.appendChild(text(doc, 'p', 'ss-detail-lede', opts.intro));
    }
    const list = el(doc, 'div', 'ss-choice-list', { role: 'radiogroup', 'aria-label': opts.heading || t('journal.window.sources.selective.range.heading') });

    (opts.options || []).forEach((opt) => {
      const selected = opts.selected === opt.value;
      const cls = ['ss-choice-row'];
      if (selected) cls.push('sel');
      if (opt.covered) cls.push('covered');

      const label = el(doc, 'label', cls.join(' '));
      label.setAttribute('data-range', opt.value);

      const left = el(doc, 'span', 'ss-choice-row-l');
      const input = el(doc, 'input', null, {
        type: 'radio',
        name: opts.name || 'ss-range',
        value: opt.value,
      });
      input.checked = selected;
      if (selected) input.setAttribute('checked', 'checked');
      if (opt.disabled) {
        input.disabled = true;
        input.setAttribute('disabled', 'disabled');
      }
      input.addEventListener('change', () => {
        if (!opt.disabled && typeof opts.onChange === 'function') opts.onChange(opt.value);
      });
      left.appendChild(input);

      const titleWrap = el(doc, 'span', null);
      titleWrap.appendChild(text(doc, 'span', 'ss-choice-row-t', opt.label));
      if (opt.subLabel) {
        titleWrap.appendChild(text(doc, 'span', 'ss-choice-row-d', opt.subLabel));
      }
      left.appendChild(titleWrap);
      label.appendChild(left);

      const right = el(doc, 'span', 'ss-choice-row-r');
      if (opt.rightPrimary) {
        right.appendChild(text(doc, 'span', opt.rightDelta ? 'ss-choice-delta-tag' : null, opt.rightPrimary));
      }
      if (opt.rightSub) {
        right.appendChild(text(doc, 'span', 'ss-choice-sub-tag', opt.rightSub));
      }
      label.appendChild(right);
      list.appendChild(label);
    });

    sec.appendChild(list);
    if (opts.helperNote) {
      sec.appendChild(text(doc, 'p', 'ss-helper-note', opts.helperNote));
    }
    return sec;
  }

  // Shared 3-unit processing estimate band (solstone's own units only, never money).
  function renderEstimateBand(doc, opts) {
    const band = el(doc, 'section', 'ss-est-band');
    band.appendChild(text(doc, 'h2', null, opts.heading || t('journal.window.sources.selective.estimate.heading')));
    if (opts.isZero) {
      band.appendChild(text(doc, 'p', 'ss-est-zero', opts.zeroLine));
    } else {
      const units = el(doc, 'div', 'ss-est-units');
      (opts.units || []).forEach((u) => {
        const box = el(doc, 'div', 'ss-est-unit');
        box.appendChild(text(doc, 'span', 'ss-est-num', u));
        units.appendChild(box);
      });
      band.appendChild(units);
    }
    if (Array.isArray(opts.glossNodes) && opts.glossNodes.length > 0) {
      const gloss = el(doc, 'div', 'ss-est-gloss');
      opts.glossNodes.forEach((node) => { if (node) gloss.appendChild(node); });
      band.appendChild(gloss);
    }
    return band;
  }

  function renderFactsList(doc, rows) {
    const dl = el(doc, 'dl', 'ss-facts-dl');
    (rows || []).forEach((row) => {
      if (!row) return;
      const item = el(doc, 'div', null);
      if (row.key) item.setAttribute('data-fact', row.key);
      item.appendChild(text(doc, 'dt', null, row.label));
      const dd = el(doc, 'dd', null);
      dd.appendChild(text(doc, 'span', null, row.value));
      if (row.sub) {
        dd.appendChild(text(doc, 'span', 'ss-fact-sub', row.sub));
      }
      item.appendChild(dd);
      dl.appendChild(item);
    });
    return dl;
  }

  // Accessible confirmation modal mounted onto root (automatically cleaned up on view dispose).
  function openConfirmModal(doc, ctx, mountTarget, opts) {
    const prevFocus = doc.activeElement;
    const wrap = el(doc, 'div', 'ss-modal-wrap', { 'data-modal': opts.id || 'confirm' });
    const dialog = el(doc, 'div', 'ss-modal', {
      role: 'dialog',
      'aria-modal': 'true',
      'aria-labelledby': 'ss-modal-title',
    });
    dialog.appendChild(text(doc, 'h2', null, opts.title, { id: 'ss-modal-title' }));
    dialog.appendChild(text(doc, 'p', null, opts.body));

    const actions = el(doc, 'div', 'ss-act-row');
    const cancelBtn = text(doc, 'button', 'ss-btn', opts.cancelLabel || t('journal.window.sources.selective.action.cancel'), { type: 'button', 'data-modal-cancel': 'true' });
    const confirmCls = opts.danger ? 'ss-btn danger' : 'ss-btn pri';
    const confirmBtn = text(doc, 'button', confirmCls, opts.confirmLabel, { type: 'button', 'data-modal-confirm': 'true' });
    actions.appendChild(cancelBtn);
    actions.appendChild(confirmBtn);
    dialog.appendChild(actions);
    wrap.appendChild(dialog);

    let closed = false;
    let offKey = null;
    function close(restoreFocus) {
      if (closed) return;
      closed = true;
      if (offKey) offKey();
      if (wrap.parentNode) wrap.parentNode.removeChild(wrap);
      if (restoreFocus && prevFocus && typeof prevFocus.focus === 'function') {
        prevFocus.focus();
      }
    }

    cancelBtn.addEventListener('click', () => {
      close(true);
      if (typeof opts.onCancel === 'function') opts.onCancel();
    });
    confirmBtn.addEventListener('click', () => {
      confirmBtn.setAttribute('disabled', 'disabled');
      confirmBtn.disabled = true;
      if (typeof opts.onConfirm === 'function') {
        Promise.resolve(opts.onConfirm(close)).catch(() => {
          confirmBtn.removeAttribute('disabled');
          confirmBtn.disabled = false;
        });
      } else {
        close(false);
      }
    });
    wrap.addEventListener('click', (event) => {
      if (event.target === wrap) {
        close(true);
        if (typeof opts.onCancel === 'function') opts.onCancel();
      }
    });
    if (ctx && typeof ctx.on === 'function') {
      offKey = ctx.on(doc, 'keydown', (event) => {
        if (event.key === 'Escape') {
          close(true);
          if (typeof opts.onCancel === 'function') opts.onCancel();
        }
      });
    }
    mountTarget.appendChild(wrap);
    if (typeof cancelBtn.focus === 'function') cancelBtn.focus();
    return { close, wrap, confirmBtn, cancelBtn };
  }

  const frameApi = {
    registerDetail(id, handler) {
      if (typeof id === 'string' && id.trim() && handler && typeof handler.mount === 'function') {
        details.set(id.trim(), handler);
      }
    },
    getDetail(id) {
      return details.get(id) || null;
    },
    formatCount,
    formatShortDate,
    relativeTime,
    el,
    text,
    link,
    stateSpec,
    renderStatePill,
    renderLeftOutDisclosure,
    renderSyncRow,
    renderDetailShell,
    renderNotice,
    renderProgress,
    renderRangeChooser,
    renderEstimateBand,
    renderFactsList,
    openConfirmModal,
  };

  if (typeof module !== 'undefined' && module.exports) module.exports = frameApi;
  global.SolSelectiveSync = frameApi;
})(typeof window !== 'undefined' ? window : globalThis);
