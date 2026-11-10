// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// A cell is the time a device named for that source. An upload that did not
// name a source stays on the device. A blank cell is not a claim that the
// source added nothing.

(function (global) {
  'use strict';

  const COPY = {
    title: 'sources',
    lede: 'when your devices last added something to your journal.',
    empty: 'no devices are paired yet.',
    pair: 'pair a device',
    manage: 'manage devices',
    nothing: 'nothing added yet',
    error: "this didn't load.",
    retry: 'try again',
  };

  const NETWORK = '/app/network/';
  const CLIENTS = '/app/network/api/clients';

  const NAMED = [
    { label: 'audio', keys: ['audio'] },
    { label: 'screen', keys: ['screen'] },
    { label: 'terminal', keys: ['terminal', 'tmux'] },
    { label: 'browsers', keys: ['browser', 'browsers'] },
    { label: 'Obsidian', keys: ['obsidian'] },
    { label: 'location', keys: ['location'] },
    { label: 'calendar', keys: ['calendar'] },
    { label: 'camera', keys: ['camera'] },
    // S1 / J4 in the design system's screenshots copy file.
    { label: 'screenshots', keys: ['screenshots'] },
    { label: 'watch', keys: ['watch', 'watch-audio'] },
  ];

  const NOT_ROWS = { '': true, mobile: true, 'mobile-segment': true, share: true };

  const MONTHS = [
    'january', 'february', 'march', 'april', 'may', 'june',
    'july', 'august', 'september', 'october', 'november', 'december',
  ];

  function parseTime(iso) {
    if (typeof iso !== 'string' || iso.trim() === '') return null;
    const parsed = Date.parse(iso);
    return Number.isFinite(parsed) ? parsed : null;
  }

  function delivery(client) {
    const value = client && client.source_delivery;
    return value && typeof value === 'object' ? value : {};
  }

  function sourceTime(client, keys) {
    let best = null;
    const rows = delivery(client);
    for (let i = 0; i < keys.length; i += 1) {
      const row = rows[keys[i]];
      if (!row) continue;
      const parsed = parseTime(row.last_accepted_ingest_at);
      if (parsed !== null && (best === null || parsed > best)) best = parsed;
    }
    return best;
  }

  function rowsFor(clients) {
    const list = Array.isArray(clients) ? clients : [];
    const named = NAMED.filter((spec) => list.some((client) => sourceTime(client, spec.keys) !== null));
    const namedKeys = {};
    NAMED.forEach((spec) => spec.keys.forEach((key) => { namedKeys[key] = true; }));
    const extra = [];
    list.forEach((client) => {
      const rows = delivery(client);
      Object.keys(rows).forEach((key) => {
        if (NOT_ROWS[key] || namedKeys[key]) return;
        if (parseTime(rows[key] && rows[key].last_accepted_ingest_at) === null) return;
        if (extra.indexOf(key) === -1) extra.push(key);
      });
    });
    extra.sort();
    return named.concat(extra.map((key) => ({
      label: key.replace(/[-_]/g, ' ').toLowerCase(),
      keys: [key],
    })));
  }

  function headingTime(client, rows) {
    const device = parseTime(client && client.last_accepted_ingest_at);
    const sourceTimes = rows
      .map((row) => sourceTime(client, row.keys))
      .filter((time) => time !== null);
    if (device === null) return sourceTimes.length === 0 ? { kind: 'nothing' } : { kind: 'omit' };
    if (sourceTimes.length === 0 || device > Math.max.apply(null, sourceTimes)) {
      return { kind: 'time', ms: device };
    }
    return { kind: 'omit' };
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

  function relative(value, nowMs, timeZone) {
    const then = typeof value === 'number' ? value : parseTime(value);
    if (then === null) return '';
    const delta = nowMs - then;
    if (delta < 60000) return 'just now';
    const minutes = Math.floor(delta / 60000);
    if (minutes < 60) return minutes === 1 ? '1 min ago' : minutes + ' min ago';
    const hours = Math.floor(delta / 3600000);
    if (hours < 24) return hours === 1 ? '1 hour ago' : hours + ' hours ago';
    const now = ymd(nowMs, timeZone);
    const day = ymd(then, timeZone);
    const prev = previousDay(now.y, now.m, now.d);
    if (day.y === prev.y && day.m === prev.m && day.d === prev.d) return 'yesterday';
    const month = MONTHS[day.m - 1];
    if (day.y === now.y) return month + ' ' + day.d;
    return month + ' ' + day.d + ', ' + day.y;
  }

  function heading(client) {
    const fields = ['display_label', 'owner_label', 'device_label'];
    for (let i = 0; i < fields.length; i += 1) {
      const value = client && client[fields[i]];
      if (typeof value === 'string' && value.trim() !== '') return value.trim();
    }
    return '';
  }

  function kindWord(client) {
    const reported = (client && client.reported) || {};
    const platform = String(reported.platform || '').trim().toLowerCase();
    const type = reported.device_type == null ? null : String(reported.device_type).trim().toLowerCase();
    if (platform === 'macos') return 'mac';
    if (platform === 'ios' && type === 'phone') return 'iphone';
    if (platform === 'ios' && type === 'tablet') return 'ipad';
    if (platform === 'ipados' && type === 'tablet') return 'ipad';
    if (platform === 'android' && type === 'phone') return 'phone';
    if (platform === 'android' && type === 'tablet') return 'tablet';
    if (platform === 'android') return null;
    if (platform === 'windows' || platform === 'linux') {
      if (type === 'phone') return 'phone';
      if (type === 'tablet') return 'tablet';
      return 'computer';
    }
    if (platform === 'watchos') return 'watch';
    return null;
  }

  function failed(status, body) {
    if (status !== 200) return true;
    if (!body || !Array.isArray(body.clients)) return true;
    return body.clients.some((client) => {
      const reason = client && client.unassessed_reason;
      return reason === 'activity_unreadable' || reason === 'activity_malformed';
    });
  }

  function el(doc, tag, className) {
    const node = doc.createElement(tag);
    if (className) node.className = className;
    return node;
  }

  function text(doc, tag, className, value) {
    const node = el(doc, tag, className);
    node.textContent = value;
    return node;
  }

  function networkLink(doc, className, label) {
    const link = text(doc, 'a', className, label);
    link.setAttribute('href', NETWORK);
    return link;
  }

  function fillDevice(doc, box, client, rows, opts) {
    const name = heading(client);
    const nameNode = text(doc, box.tagName === 'ARTICLE' ? 'h2' : 'span', 'sources-name', name);
    box.appendChild(nameNode);
    const kind = kindWord(client);
    if (kind) box.appendChild(text(doc, 'span', 'sources-kind', kind));
    const mark = headingTime(client, rows);
    if (mark.kind === 'nothing') box.appendChild(text(doc, 'span', 'sources-none', COPY.nothing));
    if (mark.kind === 'time') {
      box.appendChild(text(doc, 'span', 'sources-device-time', relative(mark.ms, opts.now, opts.timeZone)));
    }
  }

  function render(doc, root, payload, opts) {
    root.textContent = '';
    root.appendChild(text(doc, 'h1', null, COPY.title));
    root.appendChild(text(doc, 'p', 'sources-lede', COPY.lede));
    const status = payload ? payload.status : 0;
    const body = payload && payload.body;
    if (failed(status, body)) {
      root.appendChild(text(doc, 'p', 'sources-error', COPY.error));
      const button = text(doc, 'button', 'sources-retry', COPY.retry);
      button.setAttribute('type', 'button');
      button.setAttribute('data-retry', 'true');
      button.addEventListener('click', () => load(doc, root, opts));
      root.appendChild(button);
      return;
    }
    const clients = body.clients;
    if (clients.length === 0) {
      root.appendChild(text(doc, 'p', 'sources-empty', COPY.empty));
      root.appendChild(networkLink(doc, 'sources-pair', COPY.pair));
      return;
    }
    root.appendChild(networkLink(doc, 'sources-manage', COPY.manage));
    const rows = rowsFor(clients);
    const cards = opts.layout === 'cards';
    if (!cards && rows.length > 0) {
      const table = el(doc, 'table', 'sources-matrix');
      const head = el(doc, 'tr', null);
      head.appendChild(el(doc, 'th', null));
      clients.forEach((client) => {
        const cell = el(doc, 'th', 'sources-device');
        cell.setAttribute('data-device', heading(client));
        fillDevice(doc, cell, client, rows, opts);
        head.appendChild(cell);
      });
      const thead = el(doc, 'thead', null);
      thead.appendChild(head);
      table.appendChild(thead);
      const tbody = el(doc, 'tbody', null);
      rows.forEach((spec) => {
        const row = el(doc, 'tr', null);
        row.setAttribute('data-source', spec.label);
        const label = text(doc, 'th', null, spec.label);
        label.setAttribute('scope', 'row');
        row.appendChild(label);
        clients.forEach((client) => {
          const parsed = sourceTime(client, spec.keys);
          const cell = text(doc, 'td', 'sources-cell', parsed === null ? '' : relative(parsed, opts.now, opts.timeZone));
          cell.setAttribute('data-device', heading(client));
          row.appendChild(cell);
        });
        tbody.appendChild(row);
      });
      table.appendChild(tbody);
      root.appendChild(table);
      return;
    }
    const list = el(doc, cards ? 'div' : 'ul', cards ? 'sources-cards' : 'sources-devices');
    clients.forEach((client) => {
      const box = el(doc, cards ? 'article' : 'li', cards ? 'sources-card sources-device' : 'sources-device');
      box.setAttribute('data-device', heading(client));
      fillDevice(doc, box, client, rows, opts);
      if (rows.length > 0) {
        const items = el(doc, 'ul', 'sources-rows');
        rows.forEach((spec) => {
          const item = el(doc, 'li', null);
          item.setAttribute('data-source', spec.label);
          item.appendChild(text(doc, 'span', 'sources-row-label', spec.label));
          const parsed = sourceTime(client, spec.keys);
          item.appendChild(text(doc, 'span', 'sources-cell', parsed === null ? '' : relative(parsed, opts.now, opts.timeZone)));
          items.appendChild(item);
        });
        box.appendChild(items);
      }
      list.appendChild(box);
    });
    root.appendChild(list);
  }

  async function load(doc, root, opts) {
    let status = 0;
    let body = null;
    try {
      const response = await opts.fetch(CLIENTS, { method: 'GET' });
      status = response.status;
      body = await response.json();
    } catch (error) {
      status = 0;
      body = null;
    }
    const payload = { status: status, body: body };
    opts.last = payload;
    render(doc, root, payload, opts);
    return payload;
  }

  function boot(view) {
    const doc = view && view.document;
    const root = doc && doc.getElementById && doc.getElementById('sources-root');
    if (!root || !view.matchMedia) return Promise.resolve();
    const media = view.matchMedia('(max-width: 700px)');
    const state = {
      now: Date.now(),
      timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      layout: media.matches ? 'cards' : 'wide',
      fetch: function (url, init) { return view.fetch(url, init); },
      last: null,
    };
    if (media.addEventListener) {
      media.addEventListener('change', () => {
        state.layout = media.matches ? 'cards' : 'wide';
        state.now = Date.now();
        if (state.last) render(doc, root, state.last, state);
      });
    }
    return load(doc, root, state);
  }

  const api = {
    COPY: COPY,
    relative: relative,
    rowsFor: rowsFor,
    headingTime: headingTime,
    kindWord: kindWord,
    render: render,
    load: load,
    boot: boot,
  };

  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (global.document && global.document.getElementById && global.document.getElementById('sources-root')) {
    api.boot(global);
  }
})(typeof window !== 'undefined' ? window : globalThis);
