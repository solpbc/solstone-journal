// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// `/app/sources/photos` detail view on the shared selective-sync frame
// (`SolSelectiveSync`). Handles ready-to-set-up, metadata scan, before-anything-
// syncs range choice, bringing-in progress + pause + status relationship, steady
// state + widen range + chosen-set Trash, turn off, and split needs-attention.

(function (global) {
  'use strict';

  const STATE_API = '/app/sources/photos/api/state';
  const CHOOSE_API = '/app/sources/photos/api/choose-range';
  const WIDEN_API = '/app/sources/photos/api/widen-range';
  const PAUSE_API = '/app/sources/photos/api/pause';
  const RESUME_API = '/app/sources/photos/api/resume';
  const TURN_OFF_API = '/app/sources/photos/api/turn-off';
  const PHOTOS_API = '/app/sources/photos/api/photos';
  const TRASH_API = '/app/sources/photos/api/trash';
  const HEALTH_HREF = '/app/health/';
  const TRASH_HREF = '/app/settings/#trash';
  const POLL_MS = 2000;
  const PAGE_LIMIT = 48;

  const RANGE_ORDER = ['30d', '6m', '1y', 'all', 'none'];
  const WIDEN_ORDER = ['30d', '6m', '1y', 'all'];
  const REASON_SPECS = [
    ['near_duplicates', 'journal.window.sources.photos.reason.near.duplicates'],
    ['videos', 'journal.window.sources.photos.reason.videos'],
    ['saved_and_shared', 'journal.window.sources.photos.reason.saved.shared'],
    ['busy_moments', 'journal.window.sources.photos.reason.busy.moments'],
    ['screenshots', 'journal.window.sources.photos.reason.screenshots'],
    ['noise', 'journal.window.sources.photos.reason.noise'],
  ];

  function t(key, args) {
    return global.JournalI18n.render(key, args || []);
  }

  function monthNames() {
    return [
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
  }

  function rangeLabel(range) {
    switch (range) {
      case '30d': return t('journal.window.sources.selective.range.thirty.days');
      case '6m': return t('journal.window.sources.selective.range.six.months');
      case '1y': return t('journal.window.sources.selective.range.one.year');
      case 'all': return t('journal.window.sources.selective.range.all');
      case 'none': return t('journal.window.sources.selective.range.none');
      default: return String(range || '');
    }
  }

  function parseIsoMs(iso) {
    if (typeof iso !== 'string' || !iso.trim()) return null;
    const ms = Date.parse(iso);
    return Number.isFinite(ms) ? ms : null;
  }

  function formatMonthYear(value, timeZone) {
    if (typeof value !== 'string' || !value.trim()) return '';
    const trimmed = value.trim();
    const ym = /^(\d{4})-(\d{2})(?:-\d{2})?$/.exec(trimmed);
    const months = monthNames();
    if (ym) {
      const m = Number(ym[2]);
      if (m >= 1 && m <= 12) return months[m - 1] + ' ' + ym[1];
    }
    const ms = parseIsoMs(trimmed);
    if (ms === null) return '';
    try {
      const parts = new Intl.DateTimeFormat('en-US', {
        timeZone: timeZone,
        year: 'numeric',
        month: 'numeric',
      }).formatToParts(new Date(ms));
      const y = parts.find((p) => p.type === 'year');
      const m = parts.find((p) => p.type === 'month');
      if (y && m) {
        const idx = Number(m.value) - 1;
        if (idx >= 0 && idx < 12) return months[idx] + ' ' + y.value;
      }
    } catch (_) {
      // fall through
    }
    const d = new Date(ms);
    return months[d.getUTCMonth()] + ' ' + d.getUTCFullYear();
  }

  function formatFullDate(value, timeZone) {
    if (typeof value !== 'string' || !value.trim()) return '';
    const trimmed = value.trim();
    const ymd = /^(\d{4})-(\d{2})-(\d{2})$/.exec(trimmed);
    const months = monthNames();
    if (ymd) {
      const m = Number(ymd[2]);
      const day = Number(ymd[3]);
      if (m >= 1 && m <= 12) return months[m - 1] + ' ' + day + ', ' + ymd[1];
    }
    const ms = parseIsoMs(trimmed);
    if (ms === null) return '';
    try {
      const parts = new Intl.DateTimeFormat('en-US', {
        timeZone: timeZone,
        year: 'numeric',
        month: 'numeric',
        day: 'numeric',
      }).formatToParts(new Date(ms));
      const y = parts.find((p) => p.type === 'year');
      const m = parts.find((p) => p.type === 'month');
      const d = parts.find((p) => p.type === 'day');
      if (y && m && d) {
        const idx = Number(m.value) - 1;
        if (idx >= 0 && idx < 12) return months[idx] + ' ' + Number(d.value) + ', ' + y.value;
      }
    } catch (_) {
      // fall through
    }
    const dt = new Date(ms);
    return months[dt.getUTCMonth()] + ' ' + dt.getUTCDate() + ', ' + dt.getUTCFullYear();
  }

  function formatPhotoDate(item, timeZone) {
    const ms = parseIsoMs(item && item.captured_at);
    if (ms !== null) {
      try {
        const dOpts = { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false };
        if (timeZone) dOpts.timeZone = timeZone;
        const parts = new Intl.DateTimeFormat('en-US', dOpts).formatToParts(new Date(ms));
        const mon = (parts.find((p) => p.type === 'month') || {}).value || '';
        const day = (parts.find((p) => p.type === 'day') || {}).value || '';
        const hr = (parts.find((p) => p.type === 'hour') || {}).value || '';
        const min = (parts.find((p) => p.type === 'minute') || {}).value || '';
        if (mon && day && hr && min) {
          return (mon + ' ' + day).toLowerCase() + ' \u00b7 ' + hr + ':' + min;
        }
      } catch (_) {
        // fall through
      }
    }
    const rawDay = String((item && item.day) || '');
    if (/^\d{8}$/.test(rawDay)) {
      return formatFullDate(rawDay.slice(0, 4) + '-' + rawDay.slice(4, 6) + '-' + rawDay.slice(6, 8), timeZone);
    }
    return rawDay;
  }

  function deviceNames(devices) {
    if (!Array.isArray(devices)) return [];
    const seen = new Set();
    const out = [];
    devices.forEach((d) => {
      const name = d && typeof d.name === 'string' ? d.name.trim() : '';
      if (name && !seen.has(name)) {
        seen.add(name);
        out.push(name);
      }
    });
    return out;
  }

  function headerSubtitle(devices) {
    const names = deviceNames(devices);
    if (names.length === 0) return '';
    if (names.length === 1) return names[0];
    return t('journal.window.sources.photos.devices.multi', [['devices', names.join(', ')]]);
  }

  function primaryDevice(devices) {
    const names = deviceNames(devices);
    return names.length > 0 ? names.join(', ') : '';
  }

  function attentionDevice(devices) {
    if (!Array.isArray(devices) || devices.length === 0) return '';
    const bad = devices.filter((d) => {
      const acc = d && d.access_state ? String(d.access_state) : '';
      return acc === 'limited' || acc === 'denied' || acc === 'restricted';
    });
    const names = deviceNames(bad.length > 0 ? bad : devices);
    return names.length > 0 ? names.join(', ') : '';
  }

  function latestDeviceTime(devices, field) {
    if (!Array.isArray(devices)) return null;
    let best = null;
    devices.forEach((d) => {
      const ms = parseIsoMs(d && d[field]);
      if (ms !== null && (best === null || ms > best)) best = ms;
    });
    return best;
  }

  function icloudWaitingTotal(devices) {
    if (!Array.isArray(devices)) return 0;
    return devices.reduce((sum, d) => sum + Math.max(0, Number((d && d.icloud_waiting_count) || 0)), 0);
  }

  function devicesOffline(devices) {
    return Array.isArray(devices) && devices.length > 0 && devices.every((d) => d && d.connected === false);
  }

  function leftOutRows(breakdown) {
    const reasons = (breakdown && breakdown.reasons) || {};
    const out = [];
    REASON_SPECS.forEach((spec) => {
      const count = Number(reasons[spec[0]] || 0);
      if (count > 0) {
        out.push({ key: spec[0], label: t(spec[1]), count: count });
      }
    });
    return out;
  }

  function photoKey(item) {
    return String(item.day) + '/' + String(item.stream || 'photos') + '/' + String(item.segment);
  }

  function parseJsonSafe(response) {
    if (!response || typeof response.json !== 'function') return Promise.resolve(null);
    return response.json().catch(() => null);
  }

  function firstUncoveredRange(ranges) {
    for (let i = 0; i < WIDEN_ORDER.length; i += 1) {
      const key = WIDEN_ORDER[i];
      const est = ranges && ranges[key];
      if (est && !est.covered) return key;
    }
    return 'all';
  }

  function hasUncoveredWidenRange(ranges) {
    return WIDEN_ORDER.some((key) => ranges && ranges[key] && !ranges[key].covered);
  }

  function mount(ctx, root, frame) {
    const doc = global.document;
    const ended = () => !!ctx.disposed;
    const nowFn = typeof ctx.now === 'function' ? ctx.now : () => Date.now();
    const timeZone = ctx.timeZone || Intl.DateTimeFormat().resolvedOptions().timeZone;

    const state = {
      snap: null,
      disabled: false,
      loadFailed: false,
      reported: false,
      busy: false,
      errorMsg: '',
      // Range selection (default `1y` = the last year)
      selectedRange: '1y',
      widenOpen: false,
      widenRange: 'all',
      // Landed photos picker (strictly chosen-set; no select-all)
      photosLoaded: false,
      photosLoading: false,
      photosMonths: [],
      photosItems: [],
      photosNextCursor: null,
      photosMonth: 'all',
      picked: new Map(), // key -> { day, stream, segment }
      trashReceiptCount: 0,
      // Polling & modal handles
      pollTimer: 0,
      activeModal: null,
    };

    function reportReady(kind) {
      if (state.reported || typeof ctx.ready !== 'function') return;
      state.reported = true;
      ctx.ready(kind);
    }

    function clearPoll() {
      if (state.pollTimer) {
        ctx.clearTimeout(state.pollTimer);
        state.pollTimer = 0;
      }
    }

    function closeModal() {
      if (state.activeModal) {
        state.activeModal.close(false);
        state.activeModal = null;
      }
    }

    function schedulePollIfNeeded() {
      clearPoll();
      if (ended() || !state.snap) return;
      const sub = state.snap.substate;
      if (sub === 'scanning' || sub === 'bringing_in') {
        state.pollTimer = ctx.setTimeout(() => {
          state.pollTimer = 0;
          fetchState(true);
        }, POLL_MS);
      }
    }

    async function apiRequest(url, method, bodyObj) {
      const init = { method: method || 'GET', credentials: 'same-origin' };
      if (!method || method === 'GET') init.cache = 'no-store';
      if (bodyObj !== undefined) {
        init.headers = { 'Content-Type': 'application/json' };
        init.body = JSON.stringify(bodyObj);
      }
      try {
        const res = await ctx.fetch(url, init);
        if (ended()) return null;
        const body = await parseJsonSafe(res);
        if (ended()) return null;
        return { ok: !!(res && res.ok), status: res ? res.status : 0, body: body };
      } catch (_) {
        if (ended()) return null;
        return { ok: false, status: 0, body: null };
      }
    }

    function shouldShowPicker(snap) {
      if (!snap || state.widenOpen) return false;
      const inJournal = Number((snap.progress && snap.progress.in_journal) || 0);
      if (inJournal <= 0) return false;
      return snap.substate === 'steady'
        || snap.state === 'paused'
        || snap.state === 'needs_attention'
        || snap.state === 'off';
    }

    async function fetchPhotosPage(append) {
      if (state.photosLoading) return;
      state.photosLoading = true;
      const params = ['limit=' + String(PAGE_LIMIT)];
      if (state.photosMonth && state.photosMonth !== 'all') {
        params.push('month=' + encodeURIComponent(state.photosMonth));
      }
      if (append && state.photosNextCursor) {
        params.push('cursor=' + encodeURIComponent(state.photosNextCursor));
      }
      const res = await apiRequest(PHOTOS_API + '?' + params.join('&'), 'GET');
      if (ended() || !res) return;
      state.photosLoading = false;
      if (res.ok && res.body && typeof res.body === 'object') {
        state.photosLoaded = true;
        if (Array.isArray(res.body.months)) state.photosMonths = res.body.months;
        const incoming = Array.isArray(res.body.items) ? res.body.items : [];
        state.photosItems = append ? state.photosItems.concat(incoming) : incoming;
        state.photosNextCursor = typeof res.body.next_cursor === 'string' && res.body.next_cursor
          ? res.body.next_cursor
          : null;
      }
      paint();
    }

    function applyState(snap) {
      const prevSub = state.snap ? state.snap.substate : null;
      state.snap = snap;
      state.disabled = false;
      state.loadFailed = false;
      if (snap && snap.substate === 'choose' && prevSub !== 'choose') {
        state.selectedRange = '1y';
        state.widenOpen = false;
      }
      if (snap && (snap.state === 'off' || snap.substate === 'bringing_in' || snap.substate === 'scanning')) {
        state.widenOpen = false;
      }
      paint();
      schedulePollIfNeeded();
      if (shouldShowPicker(snap) && !state.photosLoaded && !state.photosLoading) {
        fetchPhotosPage(false);
      }
    }

    async function fetchState(fromPoll) {
      const res = await apiRequest(STATE_API, 'GET');
      if (ended() || !res) return;
      if (res.status === 404) {
        state.disabled = true;
        state.snap = null;
        state.loadFailed = false;
        paint();
        reportReady('empty');
        return;
      }
      if (!res.ok || !res.body || typeof res.body !== 'object' || typeof res.body.state !== 'string') {
        if (!fromPoll) {
          state.loadFailed = true;
          paint();
          reportReady('error');
        } else {
          schedulePollIfNeeded();
        }
        return;
      }
      applyState(res.body);
      reportReady('content');
    }

    // ---- Actions ----

    async function runPostAction(url, bodyObj, afterOk) {
      state.busy = true;
      state.errorMsg = '';
      paint();
      const res = await apiRequest(url, 'POST', bodyObj);
      if (ended() || !res) return;
      state.busy = false;
      if (res.ok && res.body && typeof res.body === 'object') {
        if (typeof afterOk === 'function') afterOk(res.body);
        else applyState(res.body);
      } else {
        state.errorMsg = t('journal.window.sources.photos.error.action');
        paint();
      }
    }

    function submitChooseRange() {
      runPostAction(CHOOSE_API, { range: state.selectedRange }, (body) => {
        state.widenOpen = false;
        state.trashReceiptCount = 0;
        applyState(body);
      });
    }

    function submitWidenRange() {
      runPostAction(WIDEN_API, { range: state.widenRange }, (body) => {
        state.widenOpen = false;
        applyState(body);
      });
    }

    function pausePhotos() {
      runPostAction(PAUSE_API, {});
    }

    function resumePhotos() {
      runPostAction(RESUME_API, {});
    }

    function askTurnOff() {
      closeModal();
      const dev = primaryDevice(state.snap && state.snap.devices);
      const bodyText = dev
        ? t('journal.window.sources.photos.turnoff.modal.body', [['device', dev]])
        : t('journal.window.sources.photos.turnoff.modal.body.nodevice');
      state.activeModal = frame.openConfirmModal(doc, ctx, root, {
        id: 'turn-off-photos',
        title: t('journal.window.sources.photos.turnoff.modal.title'),
        body: bodyText,
        cancelLabel: t('journal.window.sources.selective.action.cancel'),
        confirmLabel: t('journal.window.sources.photos.action.turn.off'),
        danger: true,
        onCancel: () => { state.activeModal = null; },
        onConfirm: async (close) => {
          const res = await apiRequest(TURN_OFF_API, 'POST', {});
          if (ended() || !res) return;
          close(false);
          state.activeModal = null;
          if (res.ok && res.body && typeof res.body === 'object') {
            state.widenOpen = false;
            state.errorMsg = '';
            applyState(res.body);
          } else {
            state.errorMsg = t('journal.window.sources.photos.error.action');
            paint();
          }
        },
      });
    }

    function askTrashSelected() {
      const segments = Array.from(state.picked.values());
      if (segments.length === 0) return;
      closeModal();
      const count = segments.length;
      state.activeModal = frame.openConfirmModal(doc, ctx, root, {
        id: 'trash-chosen-photos',
        title: t('journal.window.sources.photos.trash.modal.title', [
          ['quantity', count],
          ['count', frame.formatCount(count)],
        ]),
        body: t('journal.window.sources.photos.trash.modal.body', [['quantity', count]]),
        cancelLabel: t('journal.window.sources.selective.action.cancel'),
        confirmLabel: t('journal.window.sources.photos.trash.modal.confirm'),
        danger: true,
        onCancel: () => { state.activeModal = null; },
        onConfirm: async (close) => {
          const res = await apiRequest(TRASH_API, 'POST', { segments: segments });
          if (ended() || !res) return;
          close(false);
          state.activeModal = null;
          if (res.ok && res.body && typeof res.body === 'object') {
            const moved = Number(res.body.moved || 0);
            state.picked.clear();
            state.trashReceiptCount = moved;
            state.errorMsg = '';
            state.photosLoaded = false;
            state.photosItems = [];
            state.photosNextCursor = null;
            await fetchState(false);
          } else {
            state.errorMsg = t('journal.window.sources.photos.error.action');
            paint();
          }
        },
      });
    }

    // ---- Shared sub-renderers ----

    function renderEstimateSection(est, opts) {
      const isNone = opts.isNone === true;
      const isDelta = opts.isDelta === true;
      const heading = isDelta
        ? t('journal.window.sources.photos.widen.estimate.heading')
        : t('journal.window.sources.selective.estimate.heading');

      let units;
      if (isNone) {
        units = [
          t('journal.window.sources.photos.estimate.zero.photos'),
          t('journal.window.sources.photos.estimate.zero.days'),
          t('journal.window.sources.photos.estimate.zero.thoughts'),
        ];
      } else {
        const photosN = isDelta ? Number(est.delta_photos || 0) : Number(est.photos || 0);
        const daysN = isDelta ? Number(est.delta_days || 0) : Number(est.days || 0);
        const thoughtsN = isDelta ? Number(est.delta_thoughts || 0) : Number(est.thoughts || 0);
        units = [
          t(isDelta ? 'journal.window.sources.photos.estimate.more.photos' : 'journal.window.sources.photos.estimate.photos', [
            ['quantity', photosN],
            ['count', frame.formatCount(photosN)],
          ]),
          t(isDelta ? 'journal.window.sources.selective.estimate.more.days' : 'journal.window.sources.selective.estimate.days', [
            ['quantity', daysN],
            ['count', frame.formatCount(daysN)],
          ]),
          t('journal.window.sources.selective.estimate.thoughts', [
            ['quantity', thoughtsN],
            ['count', frame.formatCount(thoughtsN)],
          ]),
        ];
      }

      const glossNodes = [];
      if (isDelta) {
        glossNodes.push(frame.text(doc, 'span', null, t('journal.window.sources.photos.widen.estimate.gloss')));
      } else {
        glossNodes.push(frame.text(doc, 'span', null, t('journal.window.sources.photos.estimate.gloss1')));
        const line2 = frame.el(doc, 'span', null);
        line2.appendChild(frame.text(doc, 'span', null, t('journal.window.sources.photos.estimate.gloss2.before') + ' '));
        line2.appendChild(frame.link(doc, 'ss-inline-link', t('journal.window.sources.photos.estimate.gloss2.link'), HEALTH_HREF));
        line2.appendChild(frame.text(doc, 'span', null, t('journal.window.sources.photos.estimate.gloss2.after')));
        glossNodes.push(line2);
      }

      const band = frame.renderEstimateBand(doc, {
        heading: heading,
        isZero: false,
        units: units,
        glossNodes: glossNodes,
      });

      if (opts.thinkingReady === false) {
        band.appendChild(frame.renderNotice(doc, {
          text: t('journal.window.sources.photos.estimate.no.thinking'),
        }));
      }
      return band;
    }

    function renderWidenPanel(snap) {
      const ranges = snap.ranges || {};
      const sec = frame.el(doc, 'div', 'ss-widen-panel', { 'data-widen-panel': 'true' });

      const options = WIDEN_ORDER.map((key) => {
        const est = ranges[key] || {};
        const covered = est.covered === true;
        const earliest = key === 'all' && est.earliest_date
          ? formatMonthYear(est.earliest_date, timeZone)
          : '';
        let rightPrimary;
        let rightSub = '';
        if (covered) {
          rightPrimary = t('journal.window.sources.photos.choose.range.count', [
            ['quantity', Number(est.photos || 0)],
            ['count', frame.formatCount(est.photos || 0)],
          ]);
          rightSub = t('journal.window.sources.selective.range.already.brought.in');
        } else {
          const delta = Number(est.delta_photos || 0);
          rightPrimary = t('journal.window.sources.photos.widen.delta', [
            ['quantity', delta],
            ['count', frame.formatCount(delta)],
          ]);
          rightSub = t('journal.window.sources.photos.widen.total', [
            ['count', frame.formatCount(est.photos || 0)],
          ]);
        }
        return {
          value: key,
          label: rangeLabel(key),
          subLabel: earliest ? t('journal.window.sources.selective.range.all.since', [['month', earliest]]) : '',
          covered: covered,
          disabled: covered,
          rightPrimary: rightPrimary,
          rightDelta: !covered,
          rightSub: rightSub,
        };
      });

      const chooser = frame.renderRangeChooser(doc, {
        heading: t('journal.window.sources.photos.widen.heading'),
        intro: t('journal.window.sources.photos.widen.intro'),
        name: 'photos-widen-range',
        selected: state.widenRange,
        options: options,
        onChange: (val) => {
          state.widenRange = val;
          paint();
        },
      });
      sec.appendChild(chooser);

      const chosenEst = ranges[state.widenRange] || {};
      sec.appendChild(renderEstimateSection(chosenEst, { isNone: false, isDelta: true, thinkingReady: snap.thinking_ready }));

      const deltaN = Number(chosenEst.delta_photos || 0);
      const actRow = frame.el(doc, 'div', 'ss-act-row');
      const confirmLabel = deltaN > 0
        ? t('journal.window.sources.photos.widen.confirm', [['quantity', deltaN], ['count', frame.formatCount(deltaN)]])
        : t('journal.window.sources.photos.action.widen');
      const confirmBtn = frame.text(doc, 'button', 'ss-btn pri', confirmLabel, {
        type: 'button',
        'data-action': 'widen-confirm',
      });
      if (state.busy || chosenEst.covered) {
        confirmBtn.disabled = true;
        confirmBtn.setAttribute('disabled', 'disabled');
      }
      confirmBtn.addEventListener('click', submitWidenRange);
      actRow.appendChild(confirmBtn);

      const cancelBtn = frame.text(doc, 'button', 'ss-btn quiet', t('journal.window.sources.selective.action.cancel'), {
        type: 'button',
        'data-action': 'widen-cancel',
      });
      cancelBtn.addEventListener('click', () => {
        state.widenOpen = false;
        paint();
      });
      actRow.appendChild(cancelBtn);
      sec.appendChild(actRow);
      return sec;
    }

    function renderPickerCard() {
      const card = frame.el(doc, 'section', 'sources-detail-card ph-picker-card', { 'data-photos-picker': 'true' });

      const head = frame.el(doc, 'div', 'ph-picker-head');
      head.appendChild(frame.text(doc, 'h2', 'sources-syncs-title', t('journal.window.sources.photos.picker.heading')));

      const filterLbl = frame.el(doc, 'label', 'ph-month-filter');
      filterLbl.appendChild(frame.text(doc, 'span', null, t('journal.window.sources.photos.picker.month.label')));
      const select = frame.el(doc, 'select', 'ph-month-select', { id: 'ph-month-filter' });
      const allOpt = frame.text(doc, 'option', null, t('journal.window.sources.photos.picker.month.all'), { value: 'all' });
      if (state.photosMonth === 'all') allOpt.setAttribute('selected', 'selected');
      select.appendChild(allOpt);
      state.photosMonths.forEach((ym) => {
        const opt = frame.text(doc, 'option', null, formatMonthYear(ym, timeZone) || ym, { value: ym });
        if (state.photosMonth === ym) opt.setAttribute('selected', 'selected');
        select.appendChild(opt);
      });
      select.value = state.photosMonth;
      select.addEventListener('change', (ev) => {
        const nextMonth = (ev && ev.target && ev.target.value) || select.value || 'all';
        state.photosMonth = nextMonth;
        state.picked.clear();
        state.photosItems = [];
        state.photosNextCursor = null;
        fetchPhotosPage(false);
      });
      filterLbl.appendChild(select);
      head.appendChild(filterLbl);
      card.appendChild(head);

      card.appendChild(frame.text(doc, 'p', 'ss-detail-lede', t('journal.window.sources.photos.picker.intro')));

      if (state.trashReceiptCount > 0) {
        const receiptMsg = frame.text(doc, 'span', null, t('journal.window.sources.photos.trash.receipt', [
          ['quantity', state.trashReceiptCount],
          ['count', frame.formatCount(state.trashReceiptCount)],
        ]) + ' ');
        const trashLink = frame.link(doc, 'ss-inline-link', t('journal.window.sources.photos.trash.receipt.open'), TRASH_HREF);
        card.appendChild(frame.renderNotice(doc, {
          tone: 'ok',
          children: [receiptMsg, trashLink],
        }));
      }

      const pickedCount = state.picked.size;
      const bar = frame.el(doc, 'div', 'ph-picker-bar', { 'data-picker-bar': 'true' });
      if (pickedCount === 0) {
        bar.appendChild(frame.text(doc, 'span', null, t('journal.window.sources.photos.picker.bar.empty')));
      } else {
        bar.appendChild(frame.text(doc, 'strong', null, t('journal.window.sources.photos.picker.bar.selected', [
          ['count', frame.formatCount(pickedCount)],
        ])));
        const barActs = frame.el(doc, 'span', 'ph-picker-bar-actions');
        const clearBtn = frame.text(doc, 'button', 'ss-btn sm quiet', t('journal.window.sources.photos.picker.bar.clear'), {
          type: 'button',
          'data-action': 'picker-clear',
        });
        clearBtn.addEventListener('click', () => {
          state.picked.clear();
          paint();
        });
        barActs.appendChild(clearBtn);

        const moveBtn = frame.text(doc, 'button', 'ss-btn sm danger', t('journal.window.sources.photos.picker.bar.move', [
          ['quantity', pickedCount],
          ['count', frame.formatCount(pickedCount)],
        ]), {
          type: 'button',
          'data-action': 'picker-trash',
        });
        moveBtn.addEventListener('click', askTrashSelected);
        barActs.appendChild(moveBtn);
        bar.appendChild(barActs);
      }
      card.appendChild(bar);

      if (state.photosItems.length === 0) {
        if (state.photosLoaded) {
          card.appendChild(frame.text(doc, 'p', 'ss-detail-lede', t('journal.window.sources.photos.picker.empty')));
        }
      } else {
        const grid = frame.el(doc, 'div', 'ph-picker-grid');
        state.photosItems.forEach((item) => {
          const key = photoKey(item);
          const isPicked = state.picked.has(key);
          const whenLabel = formatPhotoDate(item, timeZone);

          const tile = frame.el(doc, 'div', isPicked ? 'ph-pick-card picked' : 'ph-pick-card', {
            'data-photo-key': key,
          });
          const thumbWrap = frame.el(doc, 'span', 'ph-pick-thumb');
          const img = frame.el(doc, 'img', null, {
            src: item.thumb_url,
            alt: item.caption || whenLabel,
            loading: 'lazy',
          });
          thumbWrap.appendChild(img);

          const chkLbl = frame.el(doc, 'label', 'ph-pick-chk', {
            'aria-label': t('journal.window.sources.photos.picker.select.aria', [['when', whenLabel]]),
          });
          const chk = frame.el(doc, 'input', null, {
            type: 'checkbox',
            'data-photo-checkbox': key,
          });
          chk.checked = isPicked;
          if (isPicked) chk.setAttribute('checked', 'checked');
          chk.addEventListener('change', () => {
            if (state.picked.has(key)) {
              state.picked.delete(key);
            } else {
              state.picked.set(key, {
                day: item.day,
                stream: item.stream || 'photos',
                segment: item.segment,
              });
            }
            paint();
          });
          chkLbl.appendChild(chk);
          thumbWrap.appendChild(chkLbl);
          tile.appendChild(thumbWrap);

          const meta = frame.el(doc, 'div', 'ph-pick-meta');
          meta.appendChild(frame.text(doc, 'span', 'ph-pick-date', whenLabel));
          if (item.caption) {
            meta.appendChild(frame.text(doc, 'span', 'ph-pick-caption', item.caption));
          }
          const dayHref = '/app/transcripts/' + encodeURIComponent(item.day)
            + '?stream=photos#' + encodeURIComponent(item.segment) + '/image';
          meta.appendChild(frame.link(doc, 'ss-inline-link ph-pick-day-link', t('journal.window.sources.photos.picker.open.day'), dayHref));
          tile.appendChild(meta);
          grid.appendChild(tile);
        });
        card.appendChild(grid);
      }

      if (state.photosNextCursor) {
        const moreRow = frame.el(doc, 'div', 'ss-act-row');
        const moreBtn = frame.text(doc, 'button', 'ss-btn', t('journal.window.sources.photos.picker.load.more'), {
          type: 'button',
          'data-action': 'picker-load-more',
        });
        if (state.photosLoading) {
          moreBtn.disabled = true;
          moreBtn.setAttribute('disabled', 'disabled');
        }
        moreBtn.addEventListener('click', () => fetchPhotosPage(true));
        moreRow.appendChild(moreBtn);
        card.appendChild(moreRow);
      }

      return card;
    }

    // ---- State renderers ----

    function paintReady(body) {
      body.appendChild(frame.text(doc, 'p', 'ss-detail-lede', t('journal.window.sources.photos.ready.lede')));
      const notes = frame.el(doc, 'div', 'ss-boundary-notes');
      notes.appendChild(frame.text(doc, 'span', null, t('journal.window.sources.photos.ready.note')));
      body.appendChild(notes);
    }

    function paintScanning(body, snap) {
      const scan = snap.scan || {};
      const checked = Number(scan.items_checked || 0);
      const total = Number(scan.items_total || 0);
      const dev = primaryDevice(snap.devices);
      const isPaused = snap.substate === 'scan_paused' || scan.paused === true;

      body.appendChild(frame.text(doc, 'h2', 'ss-section-heading', t('journal.window.sources.photos.scan.heading')));

      let progLine;
      if (total > 0) {
        progLine = dev
          ? t('journal.window.sources.photos.scan.progress', [
            ['checked', frame.formatCount(checked)],
            ['total', frame.formatCount(total)],
            ['device', dev],
          ])
          : t('journal.window.sources.photos.scan.progress.nodevice', [
            ['checked', frame.formatCount(checked)],
            ['total', frame.formatCount(total)],
          ]);
      } else {
        progLine = dev
          ? t('journal.window.sources.photos.scan.progress.empty', [['device', dev]])
          : t('journal.window.sources.photos.scan.progress.empty.nodevice');
      }
      body.appendChild(frame.text(doc, 'p', 'ss-detail-lede', progLine));
      body.appendChild(frame.renderProgress(doc, {
        value: checked,
        max: total > 0 ? total : 100,
        paused: isPaused,
      }));

      const stats = frame.el(doc, 'div', 'ss-stat-row');
      const box1 = frame.el(doc, 'div', 'ss-stat-box');
      box1.appendChild(frame.text(doc, 'span', 'ss-stat-val', total > 0
        ? frame.formatCount(checked) + ' / ' + frame.formatCount(total)
        : frame.formatCount(checked)));
      box1.appendChild(frame.text(doc, 'span', 'ss-stat-lbl', t('journal.window.sources.photos.scan.stat.checked')));
      stats.appendChild(box1);

      const hasActiveChoice = Boolean(snap.choice && snap.choice.range)
        || Number(scan.photos_brought_in_so_far || 0) > 0;
      const existingInJournal = Number((snap.progress && snap.progress.in_journal) || 0);
      const hasExistingPhotos = hasActiveChoice || existingInJournal > 0;
      const brought = hasExistingPhotos ? existingInJournal : 0;
      const box2 = frame.el(doc, 'div', 'ss-stat-box');
      box2.appendChild(frame.text(doc, 'span', 'ss-stat-val', !hasExistingPhotos && brought === 0
        ? t('journal.window.sources.photos.scan.stat.brought.zero')
        : frame.formatCount(brought)));
      box2.appendChild(frame.text(doc, 'span', 'ss-stat-lbl', t('journal.window.sources.photos.scan.stat.brought')));
      stats.appendChild(box2);

      const earliest = formatMonthYear(scan.earliest_item_at, timeZone) || '-';
      const box3 = frame.el(doc, 'div', 'ss-stat-box');
      box3.appendChild(frame.text(doc, 'span', 'ss-stat-val', earliest));
      box3.appendChild(frame.text(doc, 'span', 'ss-stat-lbl', t('journal.window.sources.photos.scan.stat.earliest')));
      stats.appendChild(box3);
      body.appendChild(stats);

      if (isPaused) {
        let pauseMsg;
        if (total > 0) {
          pauseMsg = dev
            ? t('journal.window.sources.photos.scan.paused', [
              ['checked', frame.formatCount(checked)],
              ['total', frame.formatCount(total)],
              ['device', dev],
            ])
            : t('journal.window.sources.photos.scan.paused.nodevice', [
              ['checked', frame.formatCount(checked)],
              ['total', frame.formatCount(total)],
            ]);
        } else {
          pauseMsg = dev
            ? t('journal.window.sources.photos.scan.paused.empty', [['device', dev]])
            : t('journal.window.sources.photos.scan.paused.empty.nodevice');
        }
        body.appendChild(frame.renderNotice(doc, { tone: 'warn', text: pauseMsg }));
      } else {
        body.appendChild(frame.text(doc, 'p', 'ss-helper-note', t(
          hasActiveChoice
            ? 'journal.window.sources.photos.scan.note.rescan'
            : 'journal.window.sources.photos.scan.note'
        )));
      }
    }

    function paintChoose(body, snap) {
      const scan = snap.scan || {};
      const ranges = snap.ranges || {};
      const dev = primaryDevice(snap.devices);
      const totalItems = Number(scan.items_total || scan.items_checked || 0);
      const earliest = formatMonthYear(
        scan.earliest_item_at || (ranges.all && ranges.all.earliest_date),
        timeZone
      );

      body.appendChild(frame.text(doc, 'h2', 'ss-section-heading', t('journal.window.sources.photos.choose.heading')));

      let summaryLine;
      if (totalItems > 0 && dev && earliest) {
        summaryLine = t('journal.window.sources.photos.choose.summary', [
          ['total', frame.formatCount(totalItems)],
          ['device', dev],
          ['earliest', earliest],
        ]);
      } else if (totalItems > 0 && dev) {
        summaryLine = t('journal.window.sources.photos.choose.summary.noearliest', [
          ['total', frame.formatCount(totalItems)],
          ['device', dev],
        ]);
      } else if (totalItems > 0 && earliest) {
        summaryLine = t('journal.window.sources.photos.choose.summary.nodevice', [
          ['total', frame.formatCount(totalItems)],
          ['earliest', earliest],
        ]);
      } else {
        summaryLine = t('journal.window.sources.photos.choose.summary.bare');
      }
      body.appendChild(frame.text(doc, 'p', 'ss-detail-lede', summaryLine));
      body.appendChild(frame.text(doc, 'p', 'ss-helper-note', t('journal.window.sources.photos.choose.rules')));

      const hasExisting = Number((snap.progress && snap.progress.in_journal) || 0) > 0
        || Number((snap.progress && snap.progress.moved_to_trash_by_owner) || 0) > 0;
      const countFor = (est) => (hasExisting && est.delta_photos !== undefined
        ? Number(est.delta_photos || 0)
        : Number(est.photos || 0));

      const options = RANGE_ORDER.map((key) => {
        const est = ranges[key] || {};
        const subLabel = key === 'all' && earliest
          ? t('journal.window.sources.selective.range.all.since', [['month', earliest]])
          : '';
        const n = countFor(est);
        const rightPrimary = key === 'none'
          ? t('journal.window.sources.photos.choose.range.none.count')
          : t('journal.window.sources.photos.choose.range.count', [
            ['quantity', n],
            ['count', frame.formatCount(n)],
          ]);
        return {
          value: key,
          label: rangeLabel(key),
          subLabel: subLabel,
          rightPrimary: rightPrimary,
        };
      });

      body.appendChild(frame.renderRangeChooser(doc, {
        heading: t('journal.window.sources.selective.range.heading'),
        name: 'photos-choose-range',
        selected: state.selectedRange,
        options: options,
        helperNote: t('journal.window.sources.photos.choose.range.helper'),
        onChange: (val) => {
          state.selectedRange = val;
          paint();
        },
      }));

      const selectedEst = ranges[state.selectedRange] || {};
      const isNone = state.selectedRange === 'none';
      const estimateView = hasExisting && selectedEst.delta_photos !== undefined
        ? {
          photos: Number(selectedEst.delta_photos || 0),
          days: Number(selectedEst.delta_days || 0),
          thoughts: Number(selectedEst.delta_thoughts || 0),
        }
        : selectedEst;
      body.appendChild(renderEstimateSection(estimateView, {
        isNone: isNone,
        isDelta: false,
        thinkingReady: snap.thinking_ready,
      }));

      // Left-out breakdown disclosure
      const facesOn = snap.profile_pics_enabled === true;
      if (isNone) {
        const fold = frame.el(doc, 'details', 'ss-fold');
        fold.appendChild(frame.text(doc, 'summary', null, t('journal.window.sources.photos.leftout.none.summary')));
        const foldBody = frame.el(doc, 'div', 'ss-fold-body');
        foldBody.appendChild(frame.text(doc, 'p', null, t(
          facesOn
            ? 'journal.window.sources.photos.leftout.none.body.faces'
            : 'journal.window.sources.photos.leftout.none.body.nofaces'
        )));
        foldBody.appendChild(frame.text(doc, 'p', 'ss-fold-note', t('journal.window.sources.photos.leftout.none.footer')));
        fold.appendChild(foldBody);
        body.appendChild(fold);
      } else if (selectedEst.left_out && Number(selectedEst.left_out.total || 0) > 0) {
        const totalOut = Number(selectedEst.left_out.total || 0);
        const summary = state.selectedRange === 'all'
          ? t('journal.window.sources.photos.leftout.choose.summary.all', [
            ['quantity', totalOut],
            ['count', frame.formatCount(totalOut)],
          ])
          : t('journal.window.sources.photos.leftout.choose.summary', [
            ['quantity', totalOut],
            ['count', frame.formatCount(totalOut)],
            ['range', rangeLabel(state.selectedRange)],
          ]);
        body.appendChild(frame.renderLeftOutDisclosure(doc, {
          summary: summary,
          items: leftOutRows(selectedEst.left_out),
          note: t(facesOn
            ? 'journal.window.sources.photos.leftout.note.choose.faces'
            : 'journal.window.sources.photos.leftout.note.choose.nofaces'),
        }));
      }

      const selectedPhotos = countFor(selectedEst);
      const ctaLabel = isNone || selectedPhotos === 0
        ? t('journal.window.sources.selective.action.start.from.today')
        : t('journal.window.sources.photos.choose.confirm', [
          ['quantity', selectedPhotos],
          ['count', frame.formatCount(selectedPhotos)],
        ]);

      const actRow = frame.el(doc, 'div', 'ss-act-row');
      const confirmBtn = frame.text(doc, 'button', 'ss-btn pri', ctaLabel, {
        type: 'button',
        'data-action': 'choose-confirm',
      });
      if (state.busy) {
        confirmBtn.disabled = true;
        confirmBtn.setAttribute('disabled', 'disabled');
      }
      confirmBtn.addEventListener('click', submitChooseRange);
      actRow.appendChild(confirmBtn);

      const turnOffBtn = frame.text(doc, 'button', 'ss-btn quiet', t('journal.window.sources.photos.action.turn.off'), {
        type: 'button',
        'data-action': 'turn-off',
      });
      if (state.busy) {
        turnOffBtn.disabled = true;
        turnOffBtn.setAttribute('disabled', 'disabled');
      }
      turnOffBtn.addEventListener('click', askTurnOff);
      actRow.appendChild(turnOffBtn);
      body.appendChild(actRow);
    }

    function paintBringingIn(body, snap) {
      const prog = snap.progress || {};
      const done = Number(prog.arrived || 0);
      const total = Number(prog.chosen_total || 0);
      const waiting = Number(prog.waiting_on_phone || 0);
      const dev = primaryDevice(snap.devices);
      const offline = devicesOffline(snap.devices);
      const icloud = icloudWaitingTotal(snap.devices);

      body.appendChild(frame.text(doc, 'h2', 'ss-section-heading', t('journal.window.sources.photos.bringing.heading')));

      const progLine = dev
        ? t('journal.window.sources.photos.bringing.progress', [
          ['quantity', total],
          ['done', frame.formatCount(done)],
          ['total', frame.formatCount(total)],
          ['device', dev],
        ])
        : t('journal.window.sources.photos.bringing.progress.nodevice', [
          ['quantity', total],
          ['done', frame.formatCount(done)],
          ['total', frame.formatCount(total)],
        ]);
      body.appendChild(frame.text(doc, 'p', 'ss-detail-lede', progLine));
      body.appendChild(frame.renderProgress(doc, {
        value: done,
        max: total > 0 ? total : 100,
        paused: false,
      }));

      if (offline && waiting > 0) {
        const offlineText = dev
          ? t('journal.window.sources.photos.bringing.offline', [
            ['quantity', waiting],
            ['device', dev],
            ['waiting', frame.formatCount(waiting)],
          ])
          : t('journal.window.sources.photos.bringing.offline.nodevice', [
            ['quantity', waiting],
            ['waiting', frame.formatCount(waiting)],
          ]);
        body.appendChild(frame.renderNotice(doc, { tone: 'warn', text: offlineText }));
      } else {
        body.appendChild(frame.text(doc, 'p', 'ss-helper-note', t('journal.window.sources.photos.bringing.honesty')));
      }

      if (icloud > 0) {
        const icloudText = dev
          ? t('journal.window.sources.photos.bringing.icloud', [
            ['quantity', icloud],
            ['count', frame.formatCount(icloud)],
            ['device', dev],
          ])
          : t('journal.window.sources.photos.bringing.icloud.nodevice', [
            ['quantity', icloud],
            ['count', frame.formatCount(icloud)],
          ]);
        body.appendChild(frame.text(doc, 'p', 'ss-helper-note', icloudText));
      }

      // Relationship to Band 4 (`status` / `/app/health/`) and journal thinking
      if (snap.thinking_ready === false) {
        body.appendChild(frame.renderNotice(doc, {
          tone: 'warn',
          text: t('journal.window.sources.photos.estimate.no.thinking'),
        }));
      } else if (snap.band4_paused) {
        const pausedMsg = dev
          ? t('journal.window.sources.photos.bringing.status.paused', [['device', dev]])
          : t('journal.window.sources.photos.bringing.status.paused.nodevice');
        body.appendChild(frame.renderNotice(doc, {
          children: [
            frame.text(doc, 'span', null, pausedMsg + ' '),
            frame.link(doc, 'ss-inline-link', t('journal.window.sources.photos.bringing.status.paused.link'), HEALTH_HREF),
          ],
        }));
      } else {
        body.appendChild(frame.renderNotice(doc, {
          children: [
            frame.text(doc, 'span', null, t('journal.window.sources.photos.bringing.status.active') + ' '),
            frame.link(doc, 'ss-inline-link', t('journal.window.sources.photos.bringing.status.active.link'), HEALTH_HREF),
          ],
        }));
      }

      const actRow = frame.el(doc, 'div', 'ss-act-row');
      const pauseBtn = frame.text(doc, 'button', 'ss-btn', t('journal.window.sources.photos.action.pause'), {
        type: 'button',
        'data-action': 'pause',
      });
      if (state.busy) {
        pauseBtn.disabled = true;
        pauseBtn.setAttribute('disabled', 'disabled');
      }
      pauseBtn.addEventListener('click', pausePhotos);
      actRow.appendChild(pauseBtn);

      const turnOffBtn = frame.text(doc, 'button', 'ss-btn danger-outline', t('journal.window.sources.photos.action.turn.off'), {
        type: 'button',
        'data-action': 'turn-off',
      });
      if (state.busy) {
        turnOffBtn.disabled = true;
        turnOffBtn.setAttribute('disabled', 'disabled');
      }
      turnOffBtn.addEventListener('click', askTurnOff);
      actRow.appendChild(turnOffBtn);
      body.appendChild(actRow);

      const pauseCap = dev
        ? t('journal.window.sources.photos.bringing.pause.caption', [['device', dev]])
        : t('journal.window.sources.photos.bringing.pause.caption.nodevice');
      body.appendChild(frame.text(doc, 'p', 'ss-act-hint', pauseCap));
    }

    function paintPaused(body, snap) {
      const prog = snap.progress || {};
      const done = Number(prog.arrived || 0);
      const total = Number(prog.chosen_total || 0);
      const waiting = Number(prog.waiting_on_phone || 0);
      const canWiden = snap.choice && snap.choice.can_widen && hasUncoveredWidenRange(snap.ranges);

      body.appendChild(frame.text(doc, 'h2', 'ss-section-heading', t('journal.window.sources.photos.paused.heading')));
      if (waiting > 0 && total > 0) {
        body.appendChild(frame.text(doc, 'p', 'ss-detail-lede', t('journal.window.sources.photos.paused.backfill', [
          ['quantity', total],
          ['done', frame.formatCount(done)],
          ['total', frame.formatCount(total)],
        ])));
        body.appendChild(frame.renderProgress(doc, { value: done, max: total, paused: true }));
      } else {
        body.appendChild(frame.text(doc, 'p', 'ss-detail-lede', t('journal.window.sources.photos.paused.steady')));
      }

      const actRow = frame.el(doc, 'div', 'ss-act-row');
      const resumeBtn = frame.text(doc, 'button', 'ss-btn pri', t('journal.window.sources.photos.action.resume'), {
        type: 'button',
        'data-action': 'resume',
      });
      if (state.busy) {
        resumeBtn.disabled = true;
        resumeBtn.setAttribute('disabled', 'disabled');
      }
      resumeBtn.addEventListener('click', resumePhotos);
      actRow.appendChild(resumeBtn);

      if (canWiden && !state.widenOpen) {
        const widenBtn = frame.text(doc, 'button', 'ss-btn', t('journal.window.sources.photos.action.widen'), {
          type: 'button',
          'data-action': 'open-widen',
        });
        widenBtn.addEventListener('click', () => {
          state.widenRange = firstUncoveredRange(snap.ranges);
          state.widenOpen = true;
          paint();
        });
        actRow.appendChild(widenBtn);
      }

      const turnOffBtn = frame.text(doc, 'button', 'ss-btn danger-outline', t('journal.window.sources.photos.action.turn.off'), {
        type: 'button',
        'data-action': 'turn-off',
      });
      if (state.busy) {
        turnOffBtn.disabled = true;
        turnOffBtn.setAttribute('disabled', 'disabled');
      }
      turnOffBtn.addEventListener('click', askTurnOff);
      actRow.appendChild(turnOffBtn);
      body.appendChild(actRow);

      if (state.widenOpen) {
        body.appendChild(renderWidenPanel(snap));
      }
    }

    function buildRangeFactValue(choice) {
      if (!choice || !choice.range) return '';
      if (choice.range === 'none') return t('journal.window.sources.photos.fact.range.today');
      const lbl = rangeLabel(choice.range);
      const startText = formatFullDate(choice.range_start_at, timeZone);
      if (startText && choice.range !== 'all') {
        return t('journal.window.sources.photos.fact.range.since', [
          ['start', startText],
        ]);
      }
      return lbl;
    }

    function paintSteady(body, snap) {
      const prog = snap.progress || {};
      const inJournal = Number(prog.in_journal || 0);
      const trashed = Number(prog.moved_to_trash_by_owner || 0);
      const names = deviceNames(snap.devices);
      const lastAdded = latestDeviceTime(snap.devices, 'last_upload_at');
      const lastScan = latestDeviceTime(snap.devices, 'last_scan_at');
      const rangeVal = buildRangeFactValue(snap.choice);

      const facts = [
        {
          key: 'in_journal',
          label: t('journal.window.sources.photos.fact.in.journal'),
          value: t('journal.window.sources.photos.fact.in.journal.value', [
            ['quantity', inJournal],
            ['count', frame.formatCount(inJournal)],
          ]),
        },
      ];
      if (rangeVal) {
        facts.push({ key: 'range', label: t('journal.window.sources.photos.fact.range'), value: rangeVal });
      }
      if (names.length > 0) {
        facts.push({
          key: 'device',
          label: t(names.length > 1
            ? 'journal.window.sources.photos.fact.devices'
            : 'journal.window.sources.photos.fact.device'),
          value: names.join(', '),
        });
      }
      if (lastAdded !== null) {
        facts.push({
          key: 'last_added',
          label: t('journal.window.sources.photos.fact.last.added'),
          value: frame.relativeTime(lastAdded, nowFn(), timeZone),
        });
      }
      if (lastScan !== null) {
        facts.push({
          key: 'last_scan',
          label: t('journal.window.sources.photos.fact.last.scan'),
          value: frame.relativeTime(lastScan, nowFn(), timeZone),
        });
      }
      if (trashed > 0) {
        facts.push({
          key: 'trashed',
          label: t('journal.window.sources.photos.fact.trashed'),
          value: frame.formatCount(trashed),
        });
      }
      body.appendChild(frame.renderFactsList(doc, facts));

      if (snap.left_out && Number(snap.left_out.total || 0) > 0) {
        const totalOut = Number(snap.left_out.total || 0);
        const facesOn = snap.profile_pics_enabled === true;
        body.appendChild(frame.renderLeftOutDisclosure(doc, {
          summary: t('journal.window.sources.photos.leftout.steady.summary', [
            ['quantity', totalOut],
            ['count', frame.formatCount(totalOut)],
          ]),
          items: leftOutRows(snap.left_out),
          note: t(facesOn
            ? 'journal.window.sources.photos.leftout.note.steady.faces'
            : 'journal.window.sources.photos.leftout.note.steady.nofaces'),
        }));
      }

      const notes = frame.el(doc, 'div', 'ss-boundary-notes');
      notes.appendChild(frame.text(doc, 'span', null, t('journal.window.sources.photos.boundary.stays')));
      body.appendChild(notes);

      const canWiden = snap.choice && snap.choice.can_widen && hasUncoveredWidenRange(snap.ranges);
      const actRow = frame.el(doc, 'div', 'ss-act-row');
      if (canWiden && !state.widenOpen) {
        const widenBtn = frame.text(doc, 'button', 'ss-btn pri', t('journal.window.sources.photos.action.widen'), {
          type: 'button',
          'data-action': 'open-widen',
        });
        widenBtn.addEventListener('click', () => {
          state.widenRange = firstUncoveredRange(snap.ranges);
          state.widenOpen = true;
          paint();
        });
        actRow.appendChild(widenBtn);
      }

      const pauseBtn = frame.text(doc, 'button', 'ss-btn', t('journal.window.sources.photos.action.pause'), {
        type: 'button',
        'data-action': 'pause',
      });
      if (state.busy) {
        pauseBtn.disabled = true;
        pauseBtn.setAttribute('disabled', 'disabled');
      }
      pauseBtn.addEventListener('click', pausePhotos);
      actRow.appendChild(pauseBtn);

      const turnOffBtn = frame.text(doc, 'button', 'ss-btn danger-outline', t('journal.window.sources.photos.action.turn.off'), {
        type: 'button',
        'data-action': 'turn-off',
      });
      if (state.busy) {
        turnOffBtn.disabled = true;
        turnOffBtn.setAttribute('disabled', 'disabled');
      }
      turnOffBtn.addEventListener('click', askTurnOff);
      actRow.appendChild(turnOffBtn);
      body.appendChild(actRow);

      if (state.widenOpen) {
        body.appendChild(renderWidenPanel(snap));
      }
    }

    function isRestrictedOnly(devices) {
      if (!Array.isArray(devices) || devices.length === 0) return false;
      let hasRestricted = false;
      for (const d of devices) {
        const acc = d && d.access_state ? String(d.access_state) : '';
        if (acc === 'restricted') {
          hasRestricted = true;
        } else if (acc === 'denied' || acc === 'limited') {
          return false;
        }
      }
      return hasRestricted;
    }

    function paintNeedsAttention(body, snap) {
      const dev = primaryDevice(snap.devices);
      const attnDev = attentionDevice(snap.devices);
      const isAlbum = snap.substate === 'album_contract_error'
        || snap.attention_reason === 'smart_album_contract_failed';
      const isRestricted = !isAlbum
        && (snap.attention_reason === 'access_restricted'
          || (snap.attention_reason === 'access_off' && isRestrictedOnly(snap.devices)));
      let alertMsg;
      if (isAlbum) {
        alertMsg = dev
          ? t('journal.window.sources.photos.attention.album', [['device', dev]])
          : t('journal.window.sources.photos.attention.album.nodevice');
      } else if (isRestricted) {
        alertMsg = attnDev
          ? t('journal.window.sources.photos.attention.restricted', [['device', attnDev]])
          : t('journal.window.sources.photos.attention.restricted.nodevice');
      } else {
        alertMsg = attnDev
          ? t('journal.window.sources.photos.attention.access', [['device', attnDev]])
          : t('journal.window.sources.photos.attention.access.nodevice');
      }
      body.appendChild(frame.renderNotice(doc, { tone: 'err', text: alertMsg }));

      const inJournal = Number((snap.progress && snap.progress.in_journal) || 0);
      const names = deviceNames(snap.devices);
      const lastAdded = latestDeviceTime(snap.devices, 'last_upload_at');
      const facts = [];
      if (inJournal > 0) {
        facts.push({
          key: 'in_journal',
          label: t('journal.window.sources.photos.fact.in.journal'),
          value: t('journal.window.sources.photos.fact.in.journal.value', [
            ['quantity', inJournal],
            ['count', frame.formatCount(inJournal)],
          ]),
        });
      }
      if (names.length > 0) {
        facts.push({
          key: 'device',
          label: t(names.length > 1
            ? 'journal.window.sources.photos.fact.devices'
            : 'journal.window.sources.photos.fact.device'),
          value: names.join(', '),
        });
      }
      if (lastAdded !== null) {
        facts.push({
          key: 'last_added',
          label: t('journal.window.sources.photos.fact.last.added'),
          value: frame.relativeTime(lastAdded, nowFn(), timeZone),
        });
      }
      if (facts.length > 0) {
        body.appendChild(frame.renderFactsList(doc, facts));
      }

      const actRow = frame.el(doc, 'div', 'ss-act-row');
      const turnOffBtn = frame.text(doc, 'button', 'ss-btn danger-outline', t('journal.window.sources.photos.action.turn.off'), {
        type: 'button',
        'data-action': 'turn-off',
      });
      if (state.busy) {
        turnOffBtn.disabled = true;
        turnOffBtn.setAttribute('disabled', 'disabled');
      }
      turnOffBtn.addEventListener('click', askTurnOff);
      actRow.appendChild(turnOffBtn);
      body.appendChild(actRow);
    }

    function paintOff(body, snap) {
      const inJournal = Number((snap.progress && snap.progress.in_journal) || 0);
      const dev = primaryDevice(snap.devices);
      const note = inJournal > 0
        ? t('journal.window.sources.photos.off.note.stay', [
          ['quantity', inJournal],
          ['count', frame.formatCount(inJournal)],
        ])
        : t('journal.window.sources.photos.off.note.none');
      body.appendChild(frame.renderNotice(doc, { text: note }));
      const reentry = dev
        ? t('journal.window.sources.photos.off.reentry', [['device', dev]])
        : t('journal.window.sources.photos.off.reentry.nodevice');
      body.appendChild(frame.text(doc, 'p', 'ss-detail-lede', reentry));
    }

    function paint() {
      if (ended()) return;
      root.textContent = '';

      if (state.disabled) {
        const shell = frame.renderDetailShell(doc, {
          title: t('journal.window.sources.photos.title'),
          state: 'unavailable',
          lede: t('journal.window.sources.photos.unavailable.lede'),
        });
        root.appendChild(shell.card);
        return;
      }

      if (state.loadFailed && !state.snap) {
        const shell = frame.renderDetailShell(doc, {
          title: t('journal.window.sources.photos.title'),
          state: 'unavailable',
        });
        shell.body.appendChild(frame.renderNotice(doc, {
          tone: 'err',
          text: t('journal.window.sources.photos.error.load'),
        }));
        const actRow = frame.el(doc, 'div', 'ss-act-row');
        const retryBtn = frame.text(doc, 'button', 'ss-btn', t('journal.window.sources.selective.action.try.again'), {
          type: 'button',
          'data-action': 'retry-load',
        });
        retryBtn.addEventListener('click', () => fetchState(false));
        actRow.appendChild(retryBtn);
        shell.body.appendChild(actRow);
        root.appendChild(shell.card);
        return;
      }

      if (!state.snap) return;

      const snap = state.snap;
      const shell = frame.renderDetailShell(doc, {
        title: t('journal.window.sources.photos.title'),
        subtitle: headerSubtitle(snap.devices),
        state: snap.state,
      });
      shell.card.setAttribute('data-photos-substate', snap.substate || '');

      if (state.errorMsg) {
        shell.body.appendChild(frame.renderNotice(doc, { tone: 'err', text: state.errorMsg }));
      }

      if (snap.state === 'needs_attention') {
        paintNeedsAttention(shell.body, snap);
      } else if (snap.state === 'off') {
        paintOff(shell.body, snap);
      } else if (snap.state === 'paused') {
        paintPaused(shell.body, snap);
      } else if (snap.substate === 'scanning' || snap.substate === 'scan_paused') {
        paintScanning(shell.body, snap);
      } else if (snap.substate === 'choose') {
        paintChoose(shell.body, snap);
      } else if (snap.substate === 'bringing_in') {
        paintBringingIn(shell.body, snap);
      } else if (snap.substate === 'steady') {
        paintSteady(shell.body, snap);
      } else {
        paintReady(shell.body);
      }

      root.appendChild(shell.card);

      if (shouldShowPicker(snap)) {
        root.appendChild(renderPickerCard());
      }
    }

    fetchState(false);

    return () => {
      clearPoll();
      closeModal();
    };
  }

  const shared = global.SolSelectiveSync
    || (typeof require === 'function' ? require('./selective_sync_frame.js') : null);
  if (shared && typeof shared.registerDetail === 'function') {
    shared.registerDetail('photos', { mount: mount });
  }
})(typeof window !== 'undefined' ? window : globalThis);
