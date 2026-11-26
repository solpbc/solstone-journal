// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// A cell is the time a device last added something for that source: either an
// upload that named it, or the device's own capture upload (one that names no
// source, or the phone's segment upload) whose latest delivery carried it. A
// blank cell is not a claim that the source added nothing.

(function (global) {
  'use strict';

  const frame = global.SolSelectiveSync
    || (typeof require === 'function' ? require('./selective_sync_frame.js') : null);

  function t(key, args) {
    return global.JournalI18n.render(key, args || []);
  }

  const COPY = {
    title: t('journal.window.sources.copy.title.sources'),
    lede: t('journal.window.sources.switcher.lede'),
    switcherLabel: t('journal.window.sources.switcher.aria.label'),
    switcherImports: t('journal.window.sources.switcher.tab.imports'),
    empty: t('journal.window.sources.copy.empty.no.devices.are.paired.yet'),
    pair: t('journal.window.sources.copy.pair.pair.a.device'),
    manage: t('journal.window.sources.copy.manage.manage.devices'),
    importFiles: t('journal.window.sources.copy.import.import.files.from.other.apps'),
    nothing: t('journal.window.sources.copy.nothing.nothing.added.yet'),
    error: t('journal.window.sources.copy.error.this.didn.t.load'),
    retry: t('journal.window.sources.copy.retry.try.again'),
    syncsHeading: t('journal.window.sources.syncs.heading'),
    syncsManage: t('journal.window.sources.syncs.manage'),
    syncsEmpty: t('journal.window.sources.syncs.empty.hint'),
    syncsEmptyWithGarmin: t('journal.window.sources.syncs.empty.hint.with.garmin'),
    plaud: t('journal.window.sources.syncs.provider.plaud'),
    granola: t('journal.window.sources.syncs.provider.granola'),
    garmin: t('journal.window.sources.syncs.provider.garmin'),
    email: t('journal.window.sources.email.title'),
    photos: t('journal.window.sources.photos.title'),
    syncSyncing: t('journal.window.sources.syncs.state.syncing'),
    syncPaused: t('journal.window.sources.syncs.state.paused'),
    syncUnavailable: t('journal.window.sources.syncs.unavailable'),
    garminExpiring: t('journal.window.sources.syncs.garmin.expiring'),
  };

  const SOURCES_HREF = '/app/sources/';
  const EMAIL_HREF = '/app/sources/email';
  const PHOTOS_HREF = '/app/sources/photos';
  const HEALTH_HREF = '/app/health/';
  const NETWORK = '/app/network/';
  const IMPORT = '/app/import/';
  const SYNC_SETTINGS = '/app/settings/#sync';
  const CLIENTS = '/app/network/api/clients';
  const PLAUD = '/app/settings/api/plaud';
  const GRANOLA = '/app/settings/api/granola';
  const GARMIN = '/app/settings/api/garmin';
  const EMAIL = '/app/sources/api/email';
  const PHOTOS = '/app/sources/photos/api/state';

  const DEFAULT_SYNC = { status: 200, body: { connected: false, syncing: false } };
  const DEFAULT_GARMIN_SYNC = { status: 200, body: { available: false, connected: false, syncing: false } };
  const DEFAULT_PHOTOS_SYNC = { status: 404, body: { code: 'photos_library_disabled' } };
  const DEFAULT_EMAIL_SYNC = {
    status: 200,
    body: {
      source: 'email',
      state: 'ready_to_set_up',
      phase: 'not_connected',
      connected: false,
      syncing: false,
    },
  };

  const GRANOLA_ATTENTION_KEYS = {
    invalid_key: 'journal.window.settings.granola.response.invalidkey',
    key: 'journal.window.settings.granola.response.key',
    access: 'journal.window.settings.granola.response.access',
    limited: 'journal.window.settings.granola.response.limited',
    unavailable: 'journal.window.settings.granola.response.unavailable',
    response: 'journal.window.settings.granola.response.response',
    transcript: 'journal.window.settings.granola.response.transcript',
    storage: 'journal.window.settings.granola.response.storage',
    busy: 'journal.window.settings.granola.response.busy',
    stopped: 'journal.window.settings.granola.response.stopped',
  };

  const NAMED = [
    { label: t('journal.window.sources.source.or.date.unit.label.audio'), keys: ['audio'], carried: 'audio' },
    { label: t('journal.window.sources.source.or.date.unit.label.screen'), keys: ['screen'], carried: 'screen' },
    { label: t('journal.window.sources.source.or.date.unit.label.terminal'), keys: ['terminal', 'tmux'] },
    { label: t('journal.window.sources.source.or.date.unit.label.browsers'), keys: ['browser', 'browsers', 'safari'] },
    { label: t('journal.window.sources.source.or.date.unit.label.obsidian'), keys: ['obsidian'] },
    { label: t('journal.window.sources.source.or.date.unit.label.location'), keys: ['location'], carried: 'location' },
    { label: t('journal.window.sources.source.or.date.unit.label.calendar'), keys: ['calendar'] },
    { label: t('journal.window.sources.source.or.date.unit.label.camera'), keys: ['camera'] },
    // S1 / J4 in the design system's screenshots copy file.
    { label: t('journal.window.sources.source.or.date.unit.label.screenshots'), keys: ['screenshots'] },
    { label: t('journal.window.sources.source.or.date.unit.label.photos'), keys: ['photos'] },
    { label: t('journal.window.sources.source.or.date.unit.label.watch'), keys: ['watch', 'watch-audio'] },
  ];

  const NOT_ROWS = { '': true, mobile: true, 'mobile-segment': true, share: true };

  // The device's own capture. Its delivery says what its latest upload carried.
  const OWN_CAPTURE = ['', 'mobile', 'mobile-segment'];

  const MONTHS = [
    t('journal.window.sources.months.label.january'), t('journal.window.sources.months.label.february'), t('journal.window.sources.months.label.march'), t('journal.window.sources.months.label.april'), t('journal.window.sources.months.label.may'), t('journal.window.sources.months.label.june'),
    t('journal.window.sources.months.label.july'), t('journal.window.sources.months.label.august'), t('journal.window.sources.months.label.september'), t('journal.window.sources.months.label.october'), t('journal.window.sources.months.label.november'), t('journal.window.sources.months.label.december'),
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

  function carries(row, kind) {
    return Boolean(row && Array.isArray(row.carried) && row.carried.indexOf(kind) !== -1);
  }

  function sourceTime(client, spec) {
    let best = null;
    const rows = delivery(client);
    const consider = (row) => {
      if (!row) return;
      const parsed = parseTime(row.last_accepted_ingest_at);
      if (parsed !== null && (best === null || parsed > best)) best = parsed;
    };
    spec.keys.forEach((key) => consider(rows[key]));
    if (spec.carried) {
      OWN_CAPTURE.forEach((key) => {
        if (carries(rows[key], spec.carried)) consider(rows[key]);
      });
    }
    return best;
  }

  function rowsFor(clients) {
    const list = Array.isArray(clients) ? clients : [];
    const named = NAMED.filter((spec) => list.some((client) => sourceTime(client, spec) !== null));
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
      .map((row) => sourceTime(client, row))
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

  function garminHidden(entry) {
    return Boolean(entry && entry.status === 200 && entry.body && typeof entry.body === 'object' && entry.body.available === false);
  }

  function syncFailed(entry) {
    if (!entry || entry.status !== 200) return true;
    if (!entry.body || typeof entry.body !== 'object') return true;
    return typeof entry.body.connected !== 'boolean';
  }

  function emailFailed(entry) {
    if (!entry || entry.status !== 200) return true;
    if (!entry.body || typeof entry.body !== 'object') return true;
    return typeof entry.body.phase !== 'string';
  }

  function photosHidden(entry) {
    return Boolean(entry && entry.status === 404);
  }

  function photosFailed(entry) {
    if (photosHidden(entry)) return false;
    if (!entry || entry.status !== 200) return true;
    if (!entry.body || typeof entry.body !== 'object') return true;
    return typeof entry.body.state !== 'string';
  }

  function syncAttention(id, data) {
    if (!data) return '';
    if (id === 'granola') {
      const code = typeof data.attention_code === 'string' ? data.attention_code.trim() : '';
      if (code && Object.prototype.hasOwnProperty.call(GRANOLA_ATTENTION_KEYS, code)) {
        return t(GRANOLA_ATTENTION_KEYS[code]);
      }
    }
    const attention = typeof data.attention === 'string' ? data.attention.trim() : '';
    if (attention !== '') return attention;
    if (id === 'garmin' && data.key_expiring_soon === true) return COPY.garminExpiring;
    return '';
  }

  function unpackPayload(payload) {
    if (payload && payload.clients && typeof payload.clients === 'object') {
      return {
        clients: payload.clients,
        plaud: payload.plaud || DEFAULT_SYNC,
        granola: payload.granola || DEFAULT_SYNC,
        garmin: payload.garmin || DEFAULT_GARMIN_SYNC,
        email: payload.email || DEFAULT_EMAIL_SYNC,
        photos: payload.photos || DEFAULT_PHOTOS_SYNC,
      };
    }
    return {
      clients: payload || { status: 0, body: null },
      plaud: (payload && payload.plaud) || DEFAULT_SYNC,
      granola: (payload && payload.granola) || DEFAULT_SYNC,
      garmin: (payload && payload.garmin) || DEFAULT_GARMIN_SYNC,
      email: (payload && payload.email) || DEFAULT_EMAIL_SYNC,
      photos: (payload && payload.photos) || DEFAULT_PHOTOS_SYNC,
    };
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

  function link(doc, className, label, href) {
    const node = text(doc, 'a', className, label);
    node.setAttribute('href', href);
    return node;
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

  function paintSwitcher(doc, showManageDevices) {
    const header = el(doc, 'header', 'intake-switcher');
    header.appendChild(text(doc, 'h1', 'sr-only', COPY.title));
    const top = el(doc, 'div', 'intake-switcher-top');
    const nav = el(doc, 'nav', 'intake-switcher-nav');
    nav.setAttribute('aria-label', COPY.switcherLabel);
    const sourcesTab = link(doc, 'intake-switcher-tab sources-current', COPY.title, SOURCES_HREF);
    sourcesTab.setAttribute('aria-current', 'page');
    const importsTab = link(doc, 'intake-switcher-tab sources-import', COPY.switcherImports, IMPORT);
    nav.appendChild(sourcesTab);
    nav.appendChild(importsTab);
    top.appendChild(nav);
    if (showManageDevices) {
      top.appendChild(link(doc, 'sources-manage', COPY.manage, NETWORK));
    }
    header.appendChild(top);
    header.appendChild(text(doc, 'p', 'intake-switcher-lede', COPY.lede));
    return header;
  }

  function paintDevices(doc, root, clientsEntry, opts) {
    const band = el(doc, 'section', 'sources-devices-band');
    const status = clientsEntry ? clientsEntry.status : 0;
    const body = clientsEntry && clientsEntry.body;
    if (failed(status, body)) {
      band.appendChild(text(doc, 'p', 'sources-error', COPY.error));
      const button = text(doc, 'button', 'sources-retry', COPY.retry);
      button.setAttribute('type', 'button');
      button.setAttribute('data-retry', 'true');
      button.addEventListener('click', () => load(doc, root, opts, true));
      band.appendChild(button);
      return band;
    }
    const clients = body.clients;
    if (clients.length === 0) {
      band.appendChild(text(doc, 'p', 'sources-empty', COPY.empty));
      band.appendChild(link(doc, 'sources-pair', COPY.pair, NETWORK));
      return band;
    }
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
          const parsed = sourceTime(client, spec);
          const cell = text(doc, 'td', 'sources-cell', parsed === null ? '' : relative(parsed, opts.now, opts.timeZone));
          cell.setAttribute('data-device', heading(client));
          row.appendChild(cell);
        });
        tbody.appendChild(row);
      });
      table.appendChild(tbody);
      band.appendChild(table);
      return band;
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
          const parsed = sourceTime(client, spec);
          item.appendChild(text(doc, 'span', 'sources-cell', parsed === null ? '' : relative(parsed, opts.now, opts.timeZone)));
          items.appendChild(item);
        });
        box.appendChild(items);
      }
      list.appendChild(box);
    });
    band.appendChild(list);
    return band;
  }

  function paintEmailRow(doc, emailEntry, opts) {
    const f = frame || global.SolSelectiveSync;
    const isFailed = emailFailed(emailEntry);
    if (isFailed) {
      return f.renderSyncRow(doc, {
        id: 'email',
        name: COPY.email,
        state: 'unavailable',
        unavailable: true,
        statusText: COPY.syncUnavailable,
        actionLabel: t('journal.window.sources.email.row.open'),
        actionHref: EMAIL_HREF,
      });
    }

    const data = emailEntry.body;
    const phase = data.phase || 'not_connected';
    const state = data.state || 'ready_to_set_up';
    const account = data.account || '';
    const scan = data.scan || {};
    const sync = data.sync || {};
    const acc = data.accounting || {};
    const lo = acc.left_out || { total: 0 };

    if (phase === 'not_connected' && state === 'ready_to_set_up') {
      return f.renderSyncRow(doc, {
        id: 'email',
        name: COPY.email,
        state: 'ready_to_set_up',
        statusText: t('journal.window.sources.email.row.ready'),
        actionLabel: t('journal.window.sources.email.row.connect'),
        actionHref: EMAIL_HREF,
      });
    }

    if (state === 'needs_attention' || phase === 'needs_attention') {
      const reason = data.attention_reason || 'password_rejected';
      let statusText;
      if (reason === 'all_mail_hidden' || reason === 'imap_disabled') {
        statusText = t('journal.window.sources.email.row.attention.imap');
      } else if (reason === 'unreachable') {
        statusText = t('journal.window.sources.email.row.attention.unreachable');
      } else {
        statusText = t('journal.window.sources.email.row.attention.password');
      }
      return f.renderSyncRow(doc, {
        id: 'email',
        name: COPY.email,
        subtitle: account,
        state: 'needs_attention',
        statusText,
        actionLabel: t('journal.window.sources.email.row.fix'),
        actionHref: EMAIL_HREF,
        actionTone: 'attn',
      });
    }

    if (phase === 'scanning' || phase === 'scan_paused') {
      const checked = Number(scan.checked || 0);
      const total = Number(scan.total || 0);
      const statusText = total > 0
        ? t('journal.window.sources.email.row.scanning', [
          ['checked', f.formatCount(checked)],
          ['total', f.formatCount(total)],
        ])
        : t('journal.window.sources.email.row.scanning.empty');
      return f.renderSyncRow(doc, {
        id: 'email',
        name: COPY.email,
        subtitle: account,
        state: 'setting_up',
        statusText,
        actionLabel: t('journal.window.sources.email.row.open'),
        actionHref: EMAIL_HREF,
      });
    }

    if (phase === 'awaiting_choice') {
      const eligible = Number(scan.eligible_sent || 0);
      return f.renderSyncRow(doc, {
        id: 'email',
        name: COPY.email,
        subtitle: account,
        state: 'setting_up',
        statusText: t('journal.window.sources.email.row.scan.done', [
          ['quantity', eligible],
          ['count', f.formatCount(eligible)],
        ]),
        actionLabel: t('journal.window.sources.email.row.choose'),
        actionHref: EMAIL_HREF,
        actionTone: 'accent',
      });
    }

    if (phase === 'fetching') {
      const done = Number(sync.done || 0);
      const total = Number(sync.total || 0);
      return f.renderSyncRow(doc, {
        id: 'email',
        name: COPY.email,
        subtitle: account,
        state: 'on',
        statusText: t('journal.window.sources.email.row.bringing.in', [
          ['done', f.formatCount(done)],
          ['total', f.formatCount(total)],
        ]),
        statusLink: {
          label: t('journal.window.sources.email.row.follow.status'),
          href: HEALTH_HREF,
        },
        actionLabel: t('journal.window.sources.email.row.open'),
        actionHref: EMAIL_HREF,
      });
    }

    if (phase === 'disconnected' || state === 'off') {
      const inJournal = Number(acc.in_journal || 0);
      const statusText = inJournal > 0
        ? t('journal.window.sources.email.row.disconnected', [
          ['quantity', inJournal],
          ['count', f.formatCount(inJournal)],
        ])
        : t('journal.window.sources.email.row.disconnected.none');
      return f.renderSyncRow(doc, {
        id: 'email',
        name: COPY.email,
        subtitle: account,
        state: 'off',
        statusText,
        actionLabel: t('journal.window.sources.email.row.reconnect'),
        actionHref: EMAIL_HREF,
      });
    }

    // Steady `on` state
    const parts = [
      t('journal.window.sources.email.row.in.journal', [['count', f.formatCount(acc.in_journal || 0)]]),
    ];
    const addedMs = parseTime(sync.last_added_at);
    if (addedMs !== null) {
      parts.push(t('journal.window.sources.syncs.last.added', [['when', relative(addedMs, opts.now, opts.timeZone)]]));
    }
    const checkedMs = parseTime(sync.last_checked_at);
    if (checkedMs !== null) {
      parts.push(t('journal.window.sources.syncs.last.checked', [['when', relative(checkedMs, opts.now, opts.timeZone)]]));
    }

    const leftOut = lo.total > 0
      ? {
        total: lo.total,
        summary: t('journal.window.sources.email.row.left.out.summary', [['count', f.formatCount(lo.total)]]),
        items: [
          { label: t('journal.window.sources.email.reason.calendar'), count: lo.calendar_responses || 0 },
          { label: t('journal.window.sources.email.reason.auto'), count: lo.automatic_replies || 0 },
          { label: t('journal.window.sources.email.reason.drafts'), count: lo.drafts || 0 },
          { label: t('journal.window.sources.email.reason.bounces'), count: lo.delivery_reports || 0 },
        ],
        note: t('journal.window.sources.email.row.left.out.note'),
      }
      : null;

    return f.renderSyncRow(doc, {
      id: 'email',
      name: COPY.email,
      subtitle: account,
      state: 'on',
      statusText: parts.join(' \u00b7 '),
      leftOut,
      actionLabel: t('journal.window.sources.email.row.manage'),
      actionHref: EMAIL_HREF,
    });
  }

  const PHOTOS_REASON_SPECS = [
    ['near_duplicates', 'journal.window.sources.photos.reason.near.duplicates'],
    ['videos', 'journal.window.sources.photos.reason.videos'],
    ['saved_and_shared', 'journal.window.sources.photos.reason.saved.shared'],
    ['busy_moments', 'journal.window.sources.photos.reason.busy.moments'],
    ['screenshots', 'journal.window.sources.photos.reason.screenshots'],
    ['noise', 'journal.window.sources.photos.reason.noise'],
  ];

  function photosDeviceSummary(devices) {
    if (!Array.isArray(devices) || devices.length === 0) return '';
    const seen = [];
    devices.forEach((d) => {
      const n = d && typeof d.name === 'string' ? d.name.trim() : '';
      if (n && seen.indexOf(n) === -1) seen.push(n);
    });
    return seen.join(', ');
  }

  function attentionPhotosDevice(devices) {
    if (!Array.isArray(devices) || devices.length === 0) return '';
    const bad = devices.filter((d) => {
      const acc = d && d.access_state ? String(d.access_state) : '';
      return acc === 'limited' || acc === 'denied' || acc === 'restricted';
    });
    return photosDeviceSummary(bad.length > 0 ? bad : devices);
  }

  function latestPhotosDeviceTime(devices, field) {
    if (!Array.isArray(devices)) return null;
    let best = null;
    devices.forEach((d) => {
      const ms = parseTime(d && d[field]);
      if (ms !== null && (best === null || ms > best)) best = ms;
    });
    return best;
  }

  function readyPhotosDeviceKind(photosData, clientsEntry) {
    const capable = photosData && Array.isArray(photosData.capable_device_kinds)
      ? photosData.capable_device_kinds
      : null;
    if (capable && capable.length > 0) {
      if (capable.indexOf('iphone') !== -1) return 'iphone';
      if (capable.indexOf('ipad') !== -1) return 'ipad';
    }
    const clients = clientsEntry && clientsEntry.body && Array.isArray(clientsEntry.body.clients)
      ? clientsEntry.body.clients
      : [];
    let foundIpad = false;
    for (let i = 0; i < clients.length; i += 1) {
      const kind = kindWord(clients[i]);
      if (kind === 'iphone') return 'iphone';
      if (kind === 'ipad') foundIpad = true;
    }
    return foundIpad ? 'ipad' : null;
  }

  function paintPhotosRow(doc, photosEntry, clientsEntry, opts) {
    if (photosHidden(photosEntry)) return null;
    const f = frame || global.SolSelectiveSync;
    if (photosFailed(photosEntry)) {
      return f.renderSyncRow(doc, {
        id: 'photos',
        name: COPY.photos,
        state: 'unavailable',
        unavailable: true,
        statusText: COPY.syncUnavailable,
        actionLabel: t('journal.window.sources.photos.row.open'),
        actionHref: PHOTOS_HREF,
      });
    }

    const data = photosEntry.body;
    const topState = data.state || 'ready_to_set_up';
    const substate = data.substate || '';
    const dev = photosDeviceSummary(data.devices);
    const scan = data.scan || {};
    const prog = data.progress || {};
    const ranges = data.ranges || {};
    const lo = data.left_out || { total: 0, reasons: {} };

    if (topState === 'ready_to_set_up') {
      const kind = readyPhotosDeviceKind(data, clientsEntry);
      const statusText = kind
        ? t('journal.window.sources.photos.row.ready.device', [['device_kind', kind]])
        : t('journal.window.sources.photos.row.ready');
      return f.renderSyncRow(doc, {
        id: 'photos',
        name: COPY.photos,
        state: 'ready_to_set_up',
        statusText: statusText,
        actionLabel: t('journal.window.sources.photos.row.open'),
        actionHref: PHOTOS_HREF,
      });
    }

    if (topState === 'needs_attention') {
      const isAlbum = substate === 'album_contract_error'
        || data.attention_reason === 'smart_album_contract_failed';
      const attnDev = attentionPhotosDevice(data.devices);
      let statusText;
      if (isAlbum) {
        statusText = t('journal.window.sources.photos.row.attention.album');
      } else if (attnDev) {
        statusText = t('journal.window.sources.photos.row.attention.access', [['device', attnDev]]);
      } else {
        statusText = t('journal.window.sources.photos.row.attention.access.nodevice');
      }
      return f.renderSyncRow(doc, {
        id: 'photos',
        name: COPY.photos,
        subtitle: dev,
        state: 'needs_attention',
        statusText: statusText,
        actionLabel: t('journal.window.sources.photos.row.open'),
        actionHref: PHOTOS_HREF,
        actionTone: 'attn',
      });
    }

    if (topState === 'setting_up' && (substate === 'scanning' || substate === 'scan_paused')) {
      const checked = Number(scan.items_checked || 0);
      const total = Number(scan.items_total || 0);
      const hasActiveChoice = Boolean(data.choice && data.choice.range)
        || Number(scan.photos_brought_in_so_far || 0) > 0
        || Number((data.progress && data.progress.in_journal) || 0) > 0;
      let statusText;
      if (substate === 'scan_paused' || scan.paused === true) {
        if (total > 0) {
          statusText = dev
            ? t('journal.window.sources.photos.row.scan.paused', [
              ['checked', f.formatCount(checked)],
              ['total', f.formatCount(total)],
              ['device', dev],
            ])
            : t('journal.window.sources.photos.row.scan.paused.nodevice', [
              ['checked', f.formatCount(checked)],
              ['total', f.formatCount(total)],
            ]);
        } else {
          statusText = dev
            ? t('journal.window.sources.photos.scan.paused.empty', [['device', dev]])
            : t('journal.window.sources.photos.scan.paused.empty.nodevice');
        }
      } else if (total > 0) {
        statusText = t(
          hasActiveChoice
            ? 'journal.window.sources.photos.row.scanning.rescan'
            : 'journal.window.sources.photos.row.scanning',
          [
            ['checked', f.formatCount(checked)],
            ['total', f.formatCount(total)],
          ]
        );
      } else {
        statusText = t(
          hasActiveChoice
            ? 'journal.window.sources.photos.row.scanning.rescan.empty'
            : 'journal.window.sources.photos.row.scanning.empty'
        );
      }
      return f.renderSyncRow(doc, {
        id: 'photos',
        name: COPY.photos,
        subtitle: dev,
        state: 'setting_up',
        statusText: statusText,
        actionLabel: t('journal.window.sources.photos.row.open'),
        actionHref: PHOTOS_HREF,
      });
    }

    if (topState === 'setting_up' && substate === 'choose') {
      const hasExisting = Number(prog.in_journal || 0) > 0
        || Number(prog.moved_to_trash_by_owner || 0) > 0;
      const yearEst = ranges['1y'] || {};
      const readyInYear = hasExisting && yearEst.delta_photos !== undefined
        ? Number(yearEst.delta_photos || 0)
        : Number(yearEst.photos || 0);
      const statusText = readyInYear > 0
        ? t('journal.window.sources.photos.row.scan.done', [
          ['quantity', readyInYear],
          ['count', f.formatCount(readyInYear)],
        ])
        : t('journal.window.sources.photos.row.scan.done.generic');
      return f.renderSyncRow(doc, {
        id: 'photos',
        name: COPY.photos,
        subtitle: dev,
        state: 'setting_up',
        statusText: statusText,
        actionLabel: t('journal.window.sources.photos.row.choose'),
        actionHref: PHOTOS_HREF,
        actionTone: 'accent',
      });
    }

    if (topState === 'paused') {
      const done = Number(prog.arrived || 0);
      const total = Number(prog.chosen_total || 0);
      const waiting = Number(prog.waiting_on_phone || 0);
      const statusText = waiting > 0 && total > 0
        ? t('journal.window.sources.photos.row.paused.backfill', [
          ['quantity', total],
          ['done', f.formatCount(done)],
          ['total', f.formatCount(total)],
        ])
        : t('journal.window.sources.photos.row.paused.steady');
      return f.renderSyncRow(doc, {
        id: 'photos',
        name: COPY.photos,
        subtitle: dev,
        state: 'paused',
        statusText: statusText,
        actionLabel: t('journal.window.sources.photos.row.open'),
        actionHref: PHOTOS_HREF,
      });
    }

    if (topState === 'off') {
      const inJournal = Number(prog.in_journal || 0);
      const statusText = inJournal > 0
        ? t('journal.window.sources.photos.row.off.stay', [
          ['quantity', inJournal],
          ['count', f.formatCount(inJournal)],
        ])
        : t('journal.window.sources.photos.row.off.none');
      return f.renderSyncRow(doc, {
        id: 'photos',
        name: COPY.photos,
        subtitle: dev,
        state: 'off',
        statusText: statusText,
        actionLabel: t('journal.window.sources.photos.row.open'),
        actionHref: PHOTOS_HREF,
      });
    }

    if (substate === 'bringing_in') {
      const done = Number(prog.arrived || 0);
      const total = Number(prog.chosen_total || 0);
      return f.renderSyncRow(doc, {
        id: 'photos',
        name: COPY.photos,
        subtitle: dev,
        state: 'on',
        statusText: t('journal.window.sources.photos.row.bringing.in', [
          ['quantity', total],
          ['done', f.formatCount(done)],
          ['total', f.formatCount(total)],
        ]),
        statusLink: {
          label: t('journal.window.sources.photos.row.follow.status'),
          href: HEALTH_HREF,
        },
        actionLabel: t('journal.window.sources.photos.row.open'),
        actionHref: PHOTOS_HREF,
      });
    }

    // Steady `on` state
    const inJournal = Number(prog.in_journal || 0);
    const parts = [
      t('journal.window.sources.photos.row.in.journal', [
        ['quantity', inJournal],
        ['count', f.formatCount(inJournal)],
      ]),
    ];
    const addedMs = latestPhotosDeviceTime(data.devices, 'last_upload_at');
    if (addedMs !== null) {
      parts.push(t('journal.window.sources.syncs.last.added', [['when', relative(addedMs, opts.now, opts.timeZone)]]));
    }
    const scanMs = latestPhotosDeviceTime(data.devices, 'last_scan_at');
    if (scanMs !== null) {
      parts.push(t('journal.window.sources.photos.row.last.scan', [['when', relative(scanMs, opts.now, opts.timeZone)]]));
    }

    const leftOutItems = [];
    const reasons = lo.reasons || {};
    PHOTOS_REASON_SPECS.forEach((spec) => {
      const count = Number(reasons[spec[0]] || 0);
      if (count > 0) leftOutItems.push({ label: t(spec[1]), count: count });
    });
    const leftOut = Number(lo.total || 0) > 0
      ? {
        total: Number(lo.total || 0),
        summary: t('journal.window.sources.photos.row.left.out.summary', [['count', f.formatCount(lo.total)]]),
        items: leftOutItems,
        note: t('journal.window.sources.photos.row.left.out.note'),
      }
      : null;

    return f.renderSyncRow(doc, {
      id: 'photos',
      name: COPY.photos,
      subtitle: dev,
      state: 'on',
      statusText: parts.join(' \u00b7 '),
      leftOut: leftOut,
      actionLabel: t('journal.window.sources.photos.row.manage'),
      actionHref: PHOTOS_HREF,
    });
  }

  function paintSyncs(doc, plaudEntry, granolaEntry, garminEntry, emailEntry, photosEntry, clientsEntry, opts) {
    const section = el(doc, 'section', 'sources-syncs');
    const head = el(doc, 'div', 'sources-syncs-head');
    head.appendChild(text(doc, 'h2', 'sources-syncs-title', COPY.syncsHeading));
    head.appendChild(link(doc, 'sources-syncs-manage', COPY.syncsManage, SYNC_SETTINGS));
    section.appendChild(head);

    const plaudFailed = syncFailed(plaudEntry);
    const granolaFailed = syncFailed(granolaEntry);
    const garminAvailable = !garminHidden(garminEntry);
    const garminFailed = garminAvailable && syncFailed(garminEntry);
    const plaudConnected = !plaudFailed && plaudEntry.body.connected === true;
    const granolaConnected = !granolaFailed && granolaEntry.body.connected === true;
    const garminConnected = garminAvailable && !garminFailed && garminEntry.body.connected === true;

    const list = el(doc, 'ul', 'sources-sync-list');

    // Selective-sync sources on the shared frame (Photos when enabled; Email).
    const photosRow = paintPhotosRow(doc, photosEntry, clientsEntry, opts);
    if (photosRow) list.appendChild(photosRow);
    list.appendChild(paintEmailRow(doc, emailEntry, opts));

    const providers = [
      ['plaud', COPY.plaud, plaudEntry, plaudFailed, plaudConnected],
      ['granola', COPY.granola, granolaEntry, granolaFailed, granolaConnected],
    ];
    if (garminAvailable) {
      providers.push(['garmin', COPY.garmin, garminEntry, garminFailed, garminConnected]);
    }
    providers.forEach((spec) => {
      const id = spec[0];
      const providerName = spec[1];
      const entry = spec[2];
      const isFailed = spec[3];
      const isConnected = spec[4];
      if (!isFailed && !isConnected) return;

      const row = el(doc, 'li', isFailed ? 'sources-sync-row sources-sync-unavailable' : 'sources-sync-row');
      row.setAttribute('data-sync', id);

      if (isFailed) {
        const unavailable = el(doc, 'a', 'sources-sync-link sources-sync-unavailable-' + id);
        unavailable.setAttribute('href', SYNC_SETTINGS);
        unavailable.appendChild(text(doc, 'strong', 'sources-sync-name', providerName));
        unavailable.appendChild(text(doc, 'span', 'sources-sync-sep', ' \u00b7 '));
        unavailable.appendChild(text(doc, 'span', 'sources-sync-status', COPY.syncUnavailable));
        row.appendChild(unavailable);
        list.appendChild(row);
        return;
      }

      const data = entry.body;
      const main = el(doc, 'div', 'sources-sync-main');
      const primary = el(doc, 'span', 'sources-sync-primary');
      primary.appendChild(text(doc, 'strong', 'sources-sync-name', providerName));
      primary.appendChild(text(doc, 'span', 'sources-sync-sep', ' \u00b7 '));
      const addedMs = parseTime(data.last_added);
      const primaryWords = addedMs !== null
        ? t('journal.window.sources.syncs.last.added', [['when', relative(addedMs, opts.now, opts.timeZone)]])
        : COPY.nothing;
      primary.appendChild(text(doc, 'span', 'sources-sync-status', primaryWords));
      main.appendChild(primary);

      let secondaryWords = '';
      if (data.syncing === true) {
        secondaryWords = COPY.syncSyncing;
      } else if (id === 'plaud' && data.enabled === false) {
        secondaryWords = COPY.syncPaused;
      } else {
        const checkedMs = parseTime(data.last_sync);
        if (checkedMs !== null) {
          secondaryWords = t('journal.window.sources.syncs.last.checked', [['when', relative(checkedMs, opts.now, opts.timeZone)]]);
        }
      }
      if (secondaryWords !== '') {
        main.appendChild(text(doc, 'span', 'sources-sync-secondary', secondaryWords));
      }
      row.appendChild(main);

      const attentionWords = syncAttention(id, data);
      if (attentionWords !== '') {
        row.appendChild(link(doc, 'sources-sync-attention sources-sync-attention-' + id, attentionWords, SYNC_SETTINGS));
      }
      list.appendChild(row);
    });
    section.appendChild(list);

    if (!plaudFailed && !granolaFailed && !garminFailed && !plaudConnected && !granolaConnected && !garminConnected) {
      section.appendChild(text(doc, 'p', 'sources-syncs-empty', garminAvailable ? COPY.syncsEmptyWithGarmin : COPY.syncsEmpty));
    }
    return section;
  }

  // The controls a draw makes. A redraw takes out whichever one held focus,
  // so focus goes to the same control drawn again, or else to the view's
  // root, which ships with the page and is never redrawn.
  const CONTROLS = [
    'sources-retry',
    'sources-manage',
    'sources-pair',
    'sources-current',
    'sources-import',
    'sources-syncs-manage',
    'sources-sync-action-photos',
    'sources-sync-unavailable-photos',
    'sources-sync-action-email',
    'sources-sync-unavailable-email',
    'sources-sync-unavailable-plaud',
    'sources-sync-unavailable-granola',
    'sources-sync-unavailable-garmin',
    'sources-sync-attention-plaud',
    'sources-sync-attention-granola',
    'sources-sync-attention-garmin',
  ];

  function focusedControl(doc, root) {
    const active = doc.activeElement;
    if (!active || active === root || !root.contains(active)) return active === root ? 'root' : null;
    const name = CONTROLS.find((value) => (active.className || '').split(/\s+/).indexOf(value) !== -1);
    return name || 'root';
  }

  function refocus(doc, root, held) {
    if (!held) return;
    const again = held === 'root' ? null : root.querySelector('.' + held);
    const target = again || root;
    if (doc.activeElement !== target && typeof target.focus === 'function') target.focus();
  }

  function render(doc, root, payload, opts) {
    const held = focusedControl(doc, root);
    paint(doc, root, payload, opts);
    refocus(doc, root, held);
  }

  function paint(doc, root, payload, opts) {
    root.textContent = '';
    const unpacked = unpackPayload(payload);
    const clientsEntry = unpacked.clients;
    const clientsOk = !failed(clientsEntry ? clientsEntry.status : 0, clientsEntry && clientsEntry.body);
    const hasPairedDevices = clientsOk && clientsEntry.body.clients.length > 0;
    root.appendChild(paintSwitcher(doc, hasPairedDevices));
    root.appendChild(paintDevices(doc, root, clientsEntry, opts));
    root.appendChild(paintSyncs(doc, unpacked.plaud, unpacked.granola, unpacked.garmin, unpacked.email, unpacked.photos, clientsEntry, opts));
  }

  // The state a drawn payload reports as the view's readiness.
  function readiness(clientsEntry) {
    if (failed(clientsEntry.status, clientsEntry.body)) return 'error';
    return clientsEntry.body.clients.length === 0 ? 'empty' : 'content';
  }

  async function fetchEndpoint(fetchFn, url, ended) {
    try {
      const response = await fetchFn(url, { method: 'GET' });
      if (ended()) return null;
      if (!response || typeof response.json !== 'function') {
        return { status: response ? response.status : 0, body: null };
      }
      const status = response.status;
      const body = await response.json();
      if (ended()) return null;
      return { status: status, body: body };
    } catch (error) {
      return ended() ? null : { status: 0, body: null };
    }
  }

  // A view that has gone draws nothing: `opts.ended` says so, after the read
  // and after its body, since either can answer once the view is gone.
  async function load(doc, root, opts, isRetry) {
    const ended = () => typeof opts.ended === 'function' && opts.ended();
    const prev = isRetry ? opts.last : null;
    const needPlaud = !prev || !prev.plaud || syncFailed(prev.plaud);
    const needGranola = !prev || !prev.granola || syncFailed(prev.granola);
    const needGarmin = !prev || !prev.garmin || (!garminHidden(prev.garmin) && syncFailed(prev.garmin));
    const needEmail = !prev || !prev.email || emailFailed(prev.email);
    const needPhotos = !prev || !prev.photos || (!photosHidden(prev.photos) && photosFailed(prev.photos));
    const results = await Promise.all([
      fetchEndpoint(opts.fetch, CLIENTS, ended),
      needPlaud ? fetchEndpoint(opts.fetch, PLAUD, ended) : Promise.resolve(prev.plaud),
      needGranola ? fetchEndpoint(opts.fetch, GRANOLA, ended) : Promise.resolve(prev.granola),
      needGarmin ? fetchEndpoint(opts.fetch, GARMIN, ended) : Promise.resolve(prev.garmin),
      needEmail ? fetchEndpoint(opts.fetch, EMAIL, ended) : Promise.resolve(prev.email),
      needPhotos ? fetchEndpoint(opts.fetch, PHOTOS, ended) : Promise.resolve(prev.photos),
    ]);
    if (ended() || !results[0] || !results[1] || !results[2] || !results[3] || !results[4] || !results[5]) return null;
    const clients = results[0];
    const plaud = results[1];
    const granola = results[2];
    const garmin = results[3];
    const email = results[4];
    const photos = results[5];
    const payload = {
      status: clients.status,
      body: clients.body,
      clients: clients,
      plaud: plaud,
      granola: granola,
      garmin: garmin,
      email: email,
      photos: photos,
    };
    opts.last = payload;
    render(doc, root, payload, opts);
    // The view's first draw is its readiness; a retry draws again, quietly.
    if (typeof opts.ready === 'function' && !opts.reported) {
      opts.reported = true;
      opts.ready(readiness(clients));
    }
    return payload;
  }

  function stateFor(media, fetchFn) {
    return {
      now: Date.now(),
      timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      layout: media.matches ? 'cards' : 'wide',
      fetch: fetchFn,
      last: null,
    };
  }

  function onLayout(doc, root, media, state) {
    return () => {
      state.layout = media.matches ? 'cards' : 'wide';
      state.now = Date.now();
      if (state.last) render(doc, root, state.last, state);
    };
  }

  function detailSlugFor(pathname) {
    const match = /^\/app\/sources\/([^/]+)\/?$/.exec(String(pathname || ''));
    if (!match) return null;
    try {
      return decodeURIComponent(match[1]);
    } catch (_) {
      return match[1];
    }
  }

  // Sources on the view contract. Delegates `/app/sources/<source>` subpaths
  // to registered selective-sync detail views (`SolSelectiveSync`), and draws
  // the `/app/sources/` overview otherwise.
  function mount(ctx) {
    const doc = global.document;
    const root = doc.getElementById('sources-root');
    if (!root) throw new Error('sources has no root');
    const pathname = (ctx.route && ctx.route.pathname) || (global.location && global.location.pathname) || SOURCES_HREF;
    const slug = detailSlugFor(pathname);
    const sharedFrame = frame || global.SolSelectiveSync;
    if (slug && sharedFrame && typeof sharedFrame.getDetail === 'function') {
      const detail = sharedFrame.getDetail(slug);
      if (detail) {
        return detail.mount(ctx, root, sharedFrame);
      }
    }
    const media = global.matchMedia('(max-width: 700px)');
    const state = stateFor(media, ctx.fetch);
    state.ended = () => ctx.disposed;
    state.ready = ctx.ready;
    ctx.on(media, 'change', onLayout(doc, root, media, state));
    load(doc, root, state, false).catch((error) => {
      if (!ctx.disposed) global.logError?.(error, { context: 'sources' });
    });
  }

  const api = {
    COPY: COPY,
    relative: relative,
    rowsFor: rowsFor,
    headingTime: headingTime,
    kindWord: kindWord,
    render: render,
    load: load,
    mount: mount,
  };

  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (global.SolViews && typeof global.SolViews.register === 'function') {
    global.SolViews.register('sources', { mount: mount });
  }
})(typeof window !== 'undefined' ? window : globalThis);
