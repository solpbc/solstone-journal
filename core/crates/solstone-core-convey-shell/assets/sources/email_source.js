// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// `/app/sources/email` detail view on the shared selective-sync frame
// (`SolSelectiveSync`). Handles connect, whole-mailbox header scan,
// choice before sync, bringing in, steady state + AC 5 accounting,
// widening, per-run Trash + recovery, split needs-attention, and disconnect.

(function (global) {
  'use strict';

  const EMAIL_API = '/app/sources/api/email';
  const CHOICE_PREVIEW_API = '/app/sources/api/email/choice-preview';
  const CONNECT_API = '/app/sources/api/email/connect';
  const SCAN_RESUME_API = '/app/sources/api/email/scan/resume';
  const SCAN_CANCEL_API = '/app/sources/api/email/scan/cancel';
  const CHOICE_API = '/app/sources/api/email/choice';
  const SYNC_API = '/app/sources/api/email/sync';
  const PASSWORD_API = '/app/sources/api/email/password';
  const TRASH_RECOVER_API = '/app/settings/api/trash/recover';
  const HEALTH_HREF = '/app/health/';
  const TRASH_HREF = '/app/settings/#trash';
  const GOOGLE_APP_PASSWORDS_URL = 'https://myaccount.google.com/apppasswords';
  const POLL_MS = 2000;

  const RANGE_ORDER = ['30d', '6m', '1y', 'all', 'none'];
  const TYPE_ORDER = ['replies', 'new_messages', 'forwards'];

  function t(key, args) {
    return global.JournalI18n.render(key, args || []);
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

  function outsideSubLabel(range) {
    switch (range) {
      case '30d': return t('journal.window.sources.email.steady.fact.outside.thirty.days');
      case '6m': return t('journal.window.sources.email.steady.fact.outside.six.months');
      case '1y': return t('journal.window.sources.email.steady.fact.outside.one.year');
      case 'none': return t('journal.window.sources.email.steady.fact.outside.none');
      default: return '';
    }
  }

  function typeLabel(type) {
    switch (type) {
      case 'replies': return t('journal.window.sources.email.type.replies');
      case 'new_messages': return t('journal.window.sources.email.type.new');
      case 'forwards': return t('journal.window.sources.email.type.forwards');
      default: return String(type || '');
    }
  }

  function typeDesc(type) {
    switch (type) {
      case 'replies': return t('journal.window.sources.email.type.replies.desc');
      case 'new_messages': return t('journal.window.sources.email.type.new.desc');
      case 'forwards': return t('journal.window.sources.email.type.forwards.desc');
      default: return '';
    }
  }

  function mapErrorCode(code) {
    switch (code) {
      case 'auth_failed':
      case 'password_rejected':
        return t('journal.window.sources.email.error.auth');
      case 'all_mail_hidden':
      case 'imap_disabled':
        return t('journal.window.sources.email.error.imap');
      case 'unreachable':
        return t('journal.window.sources.email.error.unreachable');
      case 'busy':
        return t('journal.window.sources.email.error.busy');
      default:
        return t('journal.window.sources.email.error.action');
    }
  }

  function leftOutItems(lo) {
    if (!lo) return [];
    return [
      { label: t('journal.window.sources.email.reason.calendar'), count: lo.calendar_responses || 0 },
      { label: t('journal.window.sources.email.reason.auto'), count: lo.automatic_replies || 0 },
      { label: t('journal.window.sources.email.reason.drafts'), count: lo.drafts || 0 },
      { label: t('journal.window.sources.email.reason.bounces'), count: lo.delivery_reports || 0 },
    ];
  }

  function leftOutInlineSummary(lo, frame) {
    return leftOutItems(lo)
      .map((item) => item.label + ': ' + frame.formatCount(item.count))
      .join(' \u00b7 ');
  }

  function providerLabel(provider) {
    if (!provider || String(provider).toLowerCase() === 'gmail') {
      return t('journal.window.sources.email.provider.gmail');
    }
    return String(provider);
  }

  function accountSubtitle(snap) {
    const prov = providerLabel(snap && snap.provider);
    if (snap && snap.account) return snap.account + ' \u00b7 ' + prov;
    return '';
  }

  function parseJsonSafe(response) {
    if (!response || typeof response.json !== 'function') return Promise.resolve(null);
    return response.json().catch(() => null);
  }

  function mount(ctx, root, frame) {
    const doc = global.document;
    const ended = () => !!ctx.disposed;
    const nowFn = typeof ctx.now === 'function' ? ctx.now : () => Date.now();
    const timeZone = ctx.timeZone || Intl.DateTimeFormat().resolvedOptions().timeZone;

    const state = {
      status: null,
      loadFailed: false,
      reported: false,
      busy: false,
      errorMsg: '',
      errorFocusPw: false,
      noticeTone: '',
      noticeText: '',
      // Connect form inputs (preserved across redraws)
      emailInput: '',
      passwordInput: '',
      showPassword: false,
      reconnectMode: false,
      // Password replacement (steady or needs_attention)
      replacePwOpen: false,
      newPasswordInput: '',
      showNewPassword: false,
      // Choice & widen selection state
      widenMode: false,
      selectedRange: '30d',
      selectedTypes: ['replies', 'new_messages', 'forwards'],
      preview: null,
      previewSeq: 0,
      previewError: '',
      // Past syncs trash receipt
      runReceipt: null, // { messages, trashIds, recovered, recoverError }
      // Polling handle
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
      if (ended() || !state.status) return;
      const phase = state.status.phase;
      if (phase === 'scanning' || phase === 'fetching') {
        state.pollTimer = ctx.setTimeout(() => {
          state.pollTimer = 0;
          fetchStatus(true);
        }, POLL_MS);
      }
    }

    async function apiRequest(url, method, bodyObj) {
      const init = { method: method || 'GET', credentials: 'same-origin' };
      if (method === 'GET') {
        init.cache = 'no-store';
      }
      if (bodyObj !== undefined) {
        init.headers = { 'Content-Type': 'application/json' };
        init.body = JSON.stringify(bodyObj);
      }
      try {
        const res = await ctx.fetch(url, init);
        if (ended()) return null;
        const body = await parseJsonSafe(res);
        if (ended()) return null;
        return { ok: !!(res && res.ok), status: res ? res.status : 0, body };
      } catch (_) {
        if (ended()) return null;
        return { ok: false, status: 0, body: null };
      }
    }

    async function fetchPreview() {
      const seq = ++state.previewSeq;
      state.previewError = '';
      const qs = '?range=' + encodeURIComponent(state.selectedRange) +
        '&types=' + encodeURIComponent(state.selectedTypes.join(','));
      const res = await apiRequest(CHOICE_PREVIEW_API + qs, 'GET');
      if (ended() || !res || seq !== state.previewSeq) return;
      if (res.ok && res.body && typeof res.body === 'object') {
        state.preview = res.body;
        state.previewError = '';
      } else {
        state.previewError = t('journal.window.sources.email.error.preview');
      }
      paint();
    }

    function applyStatus(snap, fromPoll) {
      const prevPhase = state.status ? state.status.phase : null;
      state.status = snap;
      state.loadFailed = false;
      if (snap && snap.account && !state.emailInput) {
        state.emailInput = snap.account;
      }
      if (snap && snap.phase === 'awaiting_choice' && prevPhase !== 'awaiting_choice') {
        state.selectedRange = '30d';
        state.selectedTypes = ['replies', 'new_messages', 'forwards'];
        state.widenMode = false;
        state.reconnectMode = false;
        fetchPreview();
      } else if (snap && snap.phase === 'steady' && prevPhase !== 'steady') {
        state.widenMode = false;
        state.reconnectMode = false;
      }
      if (!fromPoll && snap && snap.state === 'needs_attention') {
        state.widenMode = false;
        state.reconnectMode = false;
      }
      paint();
      schedulePollIfNeeded();
    }

    async function fetchStatus(fromPoll) {
      const res = await apiRequest(EMAIL_API, 'GET');
      if (ended() || !res) return;
      if (!res.ok || !res.body || typeof res.body !== 'object' || typeof res.body.phase !== 'string') {
        if (!fromPoll) {
          state.loadFailed = true;
          paint();
          reportReady('error');
        } else {
          schedulePollIfNeeded();
        }
        return;
      }
      applyStatus(res.body, fromPoll);
      reportReady('content');
    }

    // ---- Actions ----

    async function submitConnect() {
      const email = state.emailInput.trim();
      const pw = state.passwordInput.trim();
      if (!email) {
        state.errorMsg = t('journal.window.sources.email.error.missing.email');
        state.errorFocusPw = false;
        paint();
        return;
      }
      if (!pw) {
        state.errorMsg = t('journal.window.sources.email.error.missing.password');
        state.errorFocusPw = true;
        paint();
        return;
      }
      state.busy = true;
      state.errorMsg = '';
      state.errorFocusPw = false;
      paint();
      const res = await apiRequest(CONNECT_API, 'POST', {
        provider: 'gmail',
        email: email,
        app_password: pw,
      });
      if (ended() || !res) return;
      state.busy = false;
      if (res.ok && res.body && res.body.status) {
        state.passwordInput = '';
        state.showPassword = false;
        state.reconnectMode = false;
        state.errorMsg = '';
        applyStatus(res.body.status, false);
        return;
      }
      const code = res.body && typeof res.body.code === 'string' ? res.body.code : '';
      state.errorMsg = mapErrorCode(code);
      state.errorFocusPw = code === 'auth_failed' || code === 'password_rejected' || code === 'invalid_input';
      paint();
      if (state.errorFocusPw) {
        const pwInput = root.querySelector('#email-app-password');
        if (pwInput && typeof pwInput.focus === 'function') pwInput.focus();
      }
    }

    async function cancelScan() {
      state.busy = true;
      state.errorMsg = '';
      paint();
      const res = await apiRequest(SCAN_CANCEL_API, 'POST', {});
      if (ended() || !res) return;
      state.busy = false;
      if (res.ok && res.body && res.body.status) {
        applyStatus(res.body.status, false);
      } else {
        state.errorMsg = mapErrorCode(res.body && res.body.code);
        paint();
      }
    }

    async function resumeScan() {
      state.busy = true;
      state.errorMsg = '';
      paint();
      const res = await apiRequest(SCAN_RESUME_API, 'POST', {});
      if (ended() || !res) return;
      state.busy = false;
      if (res.ok && res.body && res.body.status) {
        applyStatus(res.body.status, false);
      } else {
        state.errorMsg = mapErrorCode(res.body && res.body.code);
        paint();
      }
    }

    async function submitChoice() {
      if (state.selectedTypes.length === 0) return;
      state.busy = true;
      state.errorMsg = '';
      paint();
      const res = await apiRequest(CHOICE_API, 'POST', {
        range: state.selectedRange,
        types: state.selectedTypes,
      });
      if (ended() || !res) return;
      state.busy = false;
      if (res.ok && res.body && res.body.status) {
        state.widenMode = false;
        state.noticeTone = '';
        state.noticeText = '';
        applyStatus(res.body.status, false);
      } else {
        state.errorMsg = mapErrorCode(res.body && res.body.code);
        paint();
      }
    }

    async function triggerSync() {
      state.busy = true;
      state.errorMsg = '';
      state.noticeText = '';
      paint();
      const res = await apiRequest(SYNC_API, 'POST', {});
      if (ended() || !res) return;
      state.busy = false;
      if (res.ok && res.body && res.body.status) {
        state.noticeTone = 'ok';
        state.noticeText = t('journal.window.sources.email.steady.check.started');
        applyStatus(res.body.status, false);
      } else {
        state.errorMsg = mapErrorCode(res.body && res.body.code);
        paint();
      }
    }

    async function submitNewPassword() {
      const pw = state.newPasswordInput.trim();
      if (!pw) {
        state.errorMsg = t('journal.window.sources.email.error.missing.password');
        paint();
        return;
      }
      state.busy = true;
      state.errorMsg = '';
      paint();
      const res = await apiRequest(PASSWORD_API, 'PUT', { app_password: pw });
      if (ended() || !res) return;
      state.busy = false;
      if (res.ok && res.body && res.body.status) {
        state.newPasswordInput = '';
        state.showNewPassword = false;
        state.replacePwOpen = false;
        state.noticeTone = 'ok';
        state.noticeText = t('journal.window.sources.email.attention.password.saved');
        applyStatus(res.body.status, false);
      } else {
        const code = res.body && typeof res.body.code === 'string' ? res.body.code : '';
        state.errorMsg = mapErrorCode(code);
        paint();
        const input = root.querySelector('#email-new-password');
        if (input && typeof input.focus === 'function') input.focus();
      }
    }

    function askDisconnect() {
      closeModal();
      state.activeModal = frame.openConfirmModal(doc, ctx, root, {
        id: 'disconnect-gmail',
        title: t('journal.window.sources.email.disconnect.modal.title'),
        body: t('journal.window.sources.email.disconnect.modal.body'),
        cancelLabel: t('journal.window.sources.selective.action.cancel'),
        confirmLabel: t('journal.window.sources.selective.action.disconnect'),
        danger: true,
        onCancel: () => { state.activeModal = null; },
        onConfirm: async (close) => {
          const res = await apiRequest(EMAIL_API, 'DELETE');
          if (ended() || !res) return;
          close(false);
          state.activeModal = null;
          if (res.ok && res.body && res.body.status) {
            state.widenMode = false;
            state.replacePwOpen = false;
            state.reconnectMode = false;
            state.errorMsg = '';
            state.noticeText = '';
            applyStatus(res.body.status, false);
          } else {
            state.errorMsg = mapErrorCode(res.body && res.body.code);
            paint();
          }
        },
      });
    }

    async function askTrashRun(run) {
      state.errorMsg = '';
      const previewRes = await apiRequest(EMAIL_API + '/runs/' + encodeURIComponent(run.id) + '/delete', 'GET');
      if (ended() || !previewRes) return;
      if (!previewRes.ok || !previewRes.body) {
        state.errorMsg = t('journal.window.sources.email.error.action');
        paint();
        return;
      }
      const info = previewRes.body;
      const msgCount = Number(info.messages || run.live_messages || run.messages || 0);
      const countStr = frame.formatCount(msgCount);
      const dateStr = frame.formatShortDate(info.synced_at || run.synced_at, timeZone);

      closeModal();
      state.activeModal = frame.openConfirmModal(doc, ctx, root, {
        id: 'trash-run',
        title: t('journal.window.sources.email.runs.modal.title'),
        body: t('journal.window.sources.email.runs.modal.body', [
          ['quantity', msgCount],
          ['count', countStr],
          ['date', dateStr],
        ]),
        cancelLabel: t('journal.window.sources.selective.action.cancel'),
        confirmLabel: t('journal.window.sources.email.runs.modal.confirm', [
          ['quantity', msgCount],
          ['count', countStr],
        ]),
        danger: true,
        onCancel: () => { state.activeModal = null; },
        onConfirm: async (close) => {
          const delRes = await apiRequest(
            EMAIL_API + '/runs/' + encodeURIComponent(run.id),
            'DELETE',
            { erase: false, backups: false }
          );
          if (ended() || !delRes) return;
          close(false);
          state.activeModal = null;
          if (delRes.ok && delRes.body) {
            const moved = Number(delRes.body.moved_messages || msgCount);
            state.runReceipt = {
              messages: moved,
              trashIds: Array.isArray(delRes.body.trash_ids) ? delRes.body.trash_ids : [],
              recovered: false,
              recoverError: false,
            };
            await fetchStatus(false);
          } else {
            state.errorMsg = t('journal.window.sources.email.error.action');
            paint();
          }
        },
      });
    }

    async function recoverRunReceipt() {
      if (!state.runReceipt || !state.runReceipt.trashIds.length) return;
      state.busy = true;
      paint();
      const res = await apiRequest(TRASH_RECOVER_API, 'POST', { ids: state.runReceipt.trashIds });
      if (ended() || !res) return;
      state.busy = false;
      const failedCount = res.body && Array.isArray(res.body.failed) ? res.body.failed.length : 0;
      if (res.ok && failedCount === 0) {
        state.runReceipt = Object.assign({}, state.runReceipt, { recovered: true, recoverError: false });
        await fetchStatus(false);
      } else {
        state.runReceipt = Object.assign({}, state.runReceipt, { recoverError: true });
        paint();
      }
    }

    // ---- Shared sub-renderers ----

    function renderPasswordBox(opts) {
      const wrap = frame.el(doc, 'div', 'ss-field-grp');
      if (opts.showGoogleLink) {
        const p = frame.el(doc, 'p', 'ss-act-hint');
        p.appendChild(frame.link(
          doc,
          'ss-inline-link',
          t('journal.window.sources.email.connect.guide.step1.link'),
          GOOGLE_APP_PASSWORDS_URL,
          { target: '_blank', rel: 'noopener noreferrer' }
        ));
        wrap.appendChild(p);
      }
      const lbl = frame.text(doc, 'label', 'ss-field-lbl', opts.label, { for: opts.id });
      wrap.appendChild(lbl);
      const row = frame.el(doc, 'div', 'ss-pw-wrap');
      const input = frame.el(doc, 'input', 'ss-input', {
        id: opts.id,
        type: opts.shown ? 'text' : 'password',
        autocomplete: 'off',
        spellcheck: 'false',
        placeholder: t('journal.window.sources.email.connect.placeholder.password'),
      });
      input.value = opts.value;
      input.addEventListener('input', (e) => opts.onInput(e.target ? e.target.value : input.value));
      row.appendChild(input);

      const toggle = frame.text(
        doc,
        'button',
        'ss-pw-toggle',
        opts.shown
          ? t('journal.window.sources.selective.action.hide')
          : t('journal.window.sources.selective.action.show'),
        { type: 'button', 'aria-pressed': opts.shown ? 'true' : 'false', 'data-pw-toggle': opts.id }
      );
      toggle.addEventListener('click', opts.onToggle);
      row.appendChild(toggle);
      wrap.appendChild(row);
      return wrap;
    }

    function renderChoiceControls(body, isWiden) {
      const snap = state.status;
      const preview = state.preview;
      const covered = (preview && Array.isArray(preview.covered_ranges))
        ? preview.covered_ranges
        : (snap && snap.choice && Array.isArray(snap.choice.covered_ranges) ? snap.choice.covered_ranges : []);
      const earliest = (preview && preview.earliest_month) || (snap && snap.scan && snap.scan.earliest_month) || '';
      const ranges = (preview && preview.ranges) || { '30d': 0, '6m': 0, '1y': 0, all: 0, none: 0 };
      const rangeDeltas = (preview && preview.range_deltas) || ranges;

      if (state.previewError) {
        body.appendChild(frame.renderNotice(doc, { tone: 'err', text: state.previewError }));
      }

      const rangeOptions = RANGE_ORDER.map((preset) => {
        const totalN = Number(ranges[preset] || 0);
        const deltaN = Number(rangeDeltas[preset] || 0);
        const isCovered = isWiden && covered.indexOf(preset) !== -1 && deltaN === 0;
        const subLabel = preset === 'all' && earliest
          ? t('journal.window.sources.selective.range.all.since', [['month', earliest]])
          : '';
        let rightPrimary = '';
        let rightSub = '';
        let rightDelta = false;
        if (preset === 'none') {
          rightPrimary = t('journal.window.sources.email.choose.range.none.count');
        } else if (isCovered) {
          rightPrimary = t('journal.window.sources.selective.range.already.brought.in');
        } else if (isWiden && deltaN > 0) {
          rightPrimary = t('journal.window.sources.email.choose.range.delta', [
            ['quantity', deltaN],
            ['count', frame.formatCount(deltaN)],
          ]);
          rightDelta = true;
        } else {
          rightPrimary = t('journal.window.sources.email.choose.range.count', [
            ['quantity', totalN],
            ['count', frame.formatCount(totalN)],
          ]);
        }
        return {
          value: preset,
          label: rangeLabel(preset),
          subLabel,
          covered: isCovered,
          rightPrimary,
          rightSub,
          rightDelta,
        };
      });

      body.appendChild(frame.renderRangeChooser(doc, {
        heading: t('journal.window.sources.selective.range.heading'),
        name: 'email-range',
        options: rangeOptions,
        selected: state.selectedRange,
        helperNote: isWiden ? '' : t('journal.window.sources.email.choose.range.helper'),
        onChange: (nextRange) => {
          state.selectedRange = nextRange;
          paint();
          fetchPreview();
        },
      }));

      // Email-specific slot: phase-1 message-type checkboxes
      const typeSec = frame.el(doc, 'section', 'ss-choice-sec');
      typeSec.appendChild(frame.text(doc, 'h2', null, t('journal.window.sources.email.choose.types.heading')));
      const typeList = frame.el(doc, 'div', 'ss-choice-list');
      const typeCounts = (preview && preview.types_in_selected_range) || { replies: 0, new_messages: 0, forwards: 0 };

      TYPE_ORDER.forEach((mt) => {
        const checked = state.selectedTypes.indexOf(mt) !== -1;
        const row = frame.el(doc, 'label', checked ? 'ss-choice-row sel' : 'ss-choice-row', { 'data-type': mt });
        const left = frame.el(doc, 'span', 'ss-choice-row-l');
        const cb = frame.el(doc, 'input', null, { type: 'checkbox', name: 'email-type', value: mt });
        cb.checked = checked;
        if (checked) cb.setAttribute('checked', 'checked');
        cb.addEventListener('change', () => {
          const idx = state.selectedTypes.indexOf(mt);
          if (idx === -1) state.selectedTypes = state.selectedTypes.concat([mt]);
          else state.selectedTypes = state.selectedTypes.filter((x) => x !== mt);
          paint();
          fetchPreview();
        });
        left.appendChild(cb);
        const textWrap = frame.el(doc, 'span', null);
        textWrap.appendChild(frame.text(doc, 'span', 'ss-choice-row-t', typeLabel(mt)));
        textWrap.appendChild(frame.text(doc, 'span', 'ss-choice-row-d', typeDesc(mt)));
        left.appendChild(textWrap);
        row.appendChild(left);
        row.appendChild(frame.text(doc, 'span', 'ss-choice-row-r', frame.formatCount(typeCounts[mt] || 0)));
        typeList.appendChild(row);
      });
      typeSec.appendChild(typeList);

      const noTypes = state.selectedTypes.length === 0;
      if (noTypes) {
        typeSec.appendChild(frame.renderNotice(doc, {
          tone: 'err',
          text: t('journal.window.sources.email.choose.types.validation'),
        }));
      }

      if (isWiden && snap && snap.choice && Array.isArray(snap.choice.types)) {
        const untickedAny = snap.choice.types.some((orig) => state.selectedTypes.indexOf(orig) === -1);
        if (untickedAny) {
          typeSec.appendChild(frame.text(doc, 'p', 'ss-helper-note', t('journal.window.sources.email.widen.untick.note'), { 'data-untick-note': 'true' }));
        }
      }
      body.appendChild(typeSec);

      // 3-unit processing estimate (solstone's own units only, never money)
      const est = isWiden
        ? ((preview && preview.delta_estimate) || { segments: 0, days: 0, thoughts: 0 })
        : ((preview && preview.estimate) || { segments: 0, days: 0, thoughts: 0 });
      const effectiveCount = isWiden
        ? Number((preview && preview.delta_messages) || 0)
        : Number((preview && preview.selected_messages) || 0);
      const isZero = noTypes || effectiveCount === 0 || (!isWiden && state.selectedRange === 'none');

      const gloss1 = frame.text(doc, 'span', null, t('journal.window.sources.email.choose.estimate.gloss1'));
      const gloss2 = frame.el(doc, 'span', null);
      gloss2.appendChild(frame.text(doc, 'span', null, t('journal.window.sources.email.choose.estimate.gloss2.before') + ' '));
      gloss2.appendChild(frame.link(doc, 'ss-inline-link', t('journal.window.sources.email.choose.estimate.gloss2.link'), HEALTH_HREF));
      gloss2.appendChild(frame.text(doc, 'span', null, '.'));

      const units = isWiden
        ? [
          t('journal.window.sources.selective.estimate.new.segments', [['quantity', est.segments], ['count', frame.formatCount(est.segments)]]),
          t('journal.window.sources.selective.estimate.more.days', [['quantity', est.days], ['count', frame.formatCount(est.days)]]),
          t('journal.window.sources.selective.estimate.thoughts', [['quantity', est.thoughts], ['count', frame.formatCount(est.thoughts)]]),
        ]
        : [
          t('journal.window.sources.selective.estimate.segments', [['quantity', est.segments], ['count', frame.formatCount(est.segments)]]),
          t('journal.window.sources.selective.estimate.days', [['quantity', est.days], ['count', frame.formatCount(est.days)]]),
          t('journal.window.sources.selective.estimate.thoughts', [['quantity', est.thoughts], ['count', frame.formatCount(est.thoughts)]]),
        ];

      body.appendChild(frame.renderEstimateBand(doc, {
        heading: isWiden
          ? t('journal.window.sources.selective.estimate.delta.heading')
          : t('journal.window.sources.selective.estimate.heading'),
        isZero,
        zeroLine: t('journal.window.sources.email.choose.estimate.zero'),
        units,
        glossNodes: [gloss1, gloss2],
      }));

      if (state.errorMsg) {
        body.appendChild(frame.renderNotice(doc, { tone: 'err', text: state.errorMsg }));
      }

      const actions = frame.el(doc, 'div', 'ss-act-row');
      let primaryLabel;
      if (isWiden) {
        primaryLabel = effectiveCount > 0
          ? t('journal.window.sources.email.widen.confirm', [['quantity', effectiveCount], ['count', frame.formatCount(effectiveCount)]])
          : t('journal.window.sources.selective.action.save.choice');
      } else {
        primaryLabel = (state.selectedRange === 'none' || effectiveCount === 0)
          ? t('journal.window.sources.selective.action.start.from.today')
          : t('journal.window.sources.email.choose.confirm', [['quantity', effectiveCount], ['count', frame.formatCount(effectiveCount)]]);
      }
      const confirmBtn = frame.text(doc, 'button', 'ss-btn pri', primaryLabel, {
        type: 'button',
        'data-email-action': 'confirm-choice',
      });
      if (noTypes || state.busy) {
        confirmBtn.disabled = true;
        confirmBtn.setAttribute('disabled', 'disabled');
      }
      confirmBtn.addEventListener('click', submitChoice);
      actions.appendChild(confirmBtn);

      if (isWiden) {
        const cancelBtn = frame.text(doc, 'button', 'ss-btn', t('journal.window.sources.selective.action.cancel'), {
          type: 'button',
          'data-email-action': 'cancel-widen',
        });
        cancelBtn.addEventListener('click', () => {
          state.widenMode = false;
          state.errorMsg = '';
          paint();
        });
        actions.appendChild(cancelBtn);
      } else {
        const discBtn = frame.text(doc, 'button', 'ss-btn quiet', t('journal.window.sources.selective.action.disconnect'), {
          type: 'button',
          'data-email-action': 'disconnect',
        });
        discBtn.addEventListener('click', askDisconnect);
        actions.appendChild(discBtn);
      }
      body.appendChild(actions);
    }

    // ---- Screen renderers ----

    function renderConnectScreen(body) {
      // Provider picker (v1 Gmail; multi-provider list layout)
      const provGrp = frame.el(doc, 'div', 'ss-field-grp');
      provGrp.appendChild(frame.text(doc, 'span', 'ss-field-lbl', t('journal.window.sources.email.connect.provider.label')));
      const provRow = frame.el(doc, 'div', 'ss-prov-row');
      const gmailOpt = frame.el(doc, 'div', 'ss-prov-opt sel', { 'data-provider': 'gmail' });
      const provText = frame.el(doc, 'div', null);
      provText.appendChild(frame.text(doc, 'span', 'ss-prov-name', t('journal.window.sources.email.provider.gmail')));
      provText.appendChild(frame.text(doc, 'span', 'ss-prov-sub', t('journal.window.sources.email.provider.auth.kind')));
      gmailOpt.appendChild(provText);
      provRow.appendChild(gmailOpt);
      provGrp.appendChild(provRow);
      provGrp.appendChild(frame.text(doc, 'p', 'ss-prov-note', t('journal.window.sources.email.connect.provider.note')));
      body.appendChild(provGrp);

      // Two-tier Trust Claim Disclosure
      const trust = frame.el(doc, 'section', 'ss-trust-block');
      trust.appendChild(frame.text(doc, 'p', 'ss-trust-surface', t('journal.window.sources.email.connect.trust.surface')));
      const trustFold = frame.el(doc, 'details', 'ss-fold');
      trustFold.appendChild(frame.text(doc, 'summary', null, t('journal.window.sources.email.connect.trust.summary')));
      const trustBody = frame.el(doc, 'div', 'ss-fold-body');
      const bullets = frame.el(doc, 'ul', 'ss-trust-bullets');
      [
        'journal.window.sources.email.connect.trust.bullet1',
        'journal.window.sources.email.connect.trust.bullet2',
        'journal.window.sources.email.connect.trust.bullet3',
        'journal.window.sources.email.connect.trust.bullet4',
      ].forEach((k) => bullets.appendChild(frame.text(doc, 'li', null, t(k))));
      trustBody.appendChild(bullets);
      trustFold.appendChild(trustBody);
      trust.appendChild(trustFold);
      body.appendChild(trust);

      // 3-step Google app password guide
      const guide = frame.el(doc, 'section', 'ss-guide-box');
      guide.appendChild(frame.text(doc, 'h2', null, t('journal.window.sources.email.connect.guide.heading')));
      const steps = frame.el(doc, 'ol', 'ss-guide-steps');
      const step1 = frame.el(doc, 'li', null);
      step1.appendChild(frame.link(
        doc,
        'ss-inline-link',
        t('journal.window.sources.email.connect.guide.step1.link'),
        GOOGLE_APP_PASSWORDS_URL,
        { target: '_blank', rel: 'noopener noreferrer' }
      ));
      step1.appendChild(frame.text(doc, 'span', null, ' ' + t('journal.window.sources.email.connect.guide.step1.after')));
      steps.appendChild(step1);
      steps.appendChild(frame.text(doc, 'li', null, t('journal.window.sources.email.connect.guide.step2')));
      steps.appendChild(frame.text(doc, 'li', null, t('journal.window.sources.email.connect.guide.step3')));
      guide.appendChild(steps);

      const helpFold = frame.el(doc, 'details', 'ss-fold');
      helpFold.appendChild(frame.text(doc, 'summary', null, t('journal.window.sources.email.connect.help.summary')));
      const helpBody = frame.el(doc, 'div', 'ss-fold-body');
      helpBody.appendChild(frame.text(doc, 'p', null, t('journal.window.sources.email.connect.help.body')));
      helpFold.appendChild(helpBody);
      guide.appendChild(helpFold);
      body.appendChild(guide);

      // Credentials form
      const form = frame.el(doc, 'form', null, { 'data-email-form': 'connect' });
      form.addEventListener('submit', (e) => {
        if (e && typeof e.preventDefault === 'function') e.preventDefault();
        submitConnect();
      });

      const grid = frame.el(doc, 'div', 'ss-input-grid');
      const emailField = frame.el(doc, 'div', null);
      emailField.appendChild(frame.text(doc, 'label', 'ss-field-lbl', t('journal.window.sources.email.connect.field.email'), { for: 'email-address' }));
      const emailInput = frame.el(doc, 'input', 'ss-input', {
        id: 'email-address',
        type: 'email',
        autocomplete: 'email',
        spellcheck: 'false',
        placeholder: t('journal.window.sources.email.connect.placeholder.email'),
      });
      emailInput.value = state.emailInput;
      emailInput.addEventListener('input', (e) => {
        state.emailInput = e.target ? e.target.value : emailInput.value;
      });
      emailField.appendChild(emailInput);
      grid.appendChild(emailField);

      const pwField = frame.el(doc, 'div', null);
      pwField.appendChild(frame.text(doc, 'label', 'ss-field-lbl', t('journal.window.sources.email.connect.field.password'), { for: 'email-app-password' }));
      const pwWrap = frame.el(doc, 'div', 'ss-pw-wrap');
      const pwInput = frame.el(doc, 'input', 'ss-input', {
        id: 'email-app-password',
        type: state.showPassword ? 'text' : 'password',
        autocomplete: 'off',
        spellcheck: 'false',
        placeholder: t('journal.window.sources.email.connect.placeholder.password'),
      });
      pwInput.value = state.passwordInput;
      pwInput.addEventListener('input', (e) => {
        state.passwordInput = e.target ? e.target.value : pwInput.value;
      });
      pwWrap.appendChild(pwInput);
      const pwToggle = frame.text(
        doc,
        'button',
        'ss-pw-toggle',
        state.showPassword
          ? t('journal.window.sources.selective.action.hide')
          : t('journal.window.sources.selective.action.show'),
        { type: 'button', 'aria-pressed': state.showPassword ? 'true' : 'false', 'data-pw-toggle': 'email-app-password' }
      );
      pwToggle.addEventListener('click', () => {
        state.showPassword = !state.showPassword;
        paint();
      });
      pwWrap.appendChild(pwToggle);
      pwField.appendChild(pwWrap);
      grid.appendChild(pwField);
      form.appendChild(grid);

      if (state.errorMsg) {
        form.appendChild(frame.renderNotice(doc, { tone: 'err', text: state.errorMsg }));
      }

      const actions = frame.el(doc, 'div', 'ss-act-row');
      const submitBtn = frame.text(
        doc,
        'button',
        'ss-btn pri',
        state.busy
          ? t('journal.window.sources.email.connect.submitting')
          : t('journal.window.sources.email.connect.submit'),
        { type: 'submit', 'data-email-action': 'connect' }
      );
      if (state.busy) {
        submitBtn.disabled = true;
        submitBtn.setAttribute('disabled', 'disabled');
      }
      actions.appendChild(submitBtn);
      if (state.reconnectMode) {
        const cancelBtn = frame.text(doc, 'button', 'ss-btn', t('journal.window.sources.selective.action.cancel'), { type: 'button' });
        cancelBtn.addEventListener('click', () => {
          state.reconnectMode = false;
          state.errorMsg = '';
          paint();
        });
        actions.appendChild(cancelBtn);
      }
      form.appendChild(actions);
      form.appendChild(frame.text(doc, 'p', 'ss-act-hint', t('journal.window.sources.email.connect.reassurance')));
      body.appendChild(form);
    }

    function renderScanScreen(body, paused) {
      const scan = (state.status && state.status.scan) || {};
      const checked = Number(scan.checked || 0);
      const total = Number(scan.total || 0);
      const sentFound = Number(scan.sent_found || 0);
      const earliest = scan.earliest_month || '\u2014';

      body.appendChild(frame.text(doc, 'h2', 'ss-section-heading', t('journal.window.sources.email.scan.heading')));
      body.appendChild(frame.renderProgress(doc, {
        value: checked,
        max: total,
        paused: paused,
        label: t('journal.window.sources.email.scan.progress', [
          ['checked', frame.formatCount(checked)],
          ['total', frame.formatCount(total)],
        ]),
      }));

      const stats = frame.el(doc, 'div', 'ss-stat-row');
      [
        [frame.formatCount(sentFound), t('journal.window.sources.email.scan.stat.sent')],
        [earliest, t('journal.window.sources.email.scan.stat.earliest')],
        ['0', t('journal.window.sources.email.scan.stat.bodies')],
      ].forEach((pair) => {
        const box = frame.el(doc, 'div', 'ss-stat-box');
        box.appendChild(frame.text(doc, 'span', 'ss-stat-val', pair[0]));
        box.appendChild(frame.text(doc, 'span', 'ss-stat-lbl', pair[1]));
        stats.appendChild(box);
      });
      body.appendChild(stats);

      if (paused) {
        body.appendChild(frame.renderNotice(doc, {
          tone: 'warn',
          text: t('journal.window.sources.email.scan.paused.notice', [
            ['checked', frame.formatCount(checked)],
            ['total', frame.formatCount(total)],
          ]),
        }));
      } else {
        body.appendChild(frame.text(doc, 'p', 'ss-act-hint', t('journal.window.sources.email.scan.reassurance')));
      }

      if (state.errorMsg) {
        body.appendChild(frame.renderNotice(doc, { tone: 'err', text: state.errorMsg }));
      }

      const actions = frame.el(doc, 'div', 'ss-act-row');
      if (paused) {
        const resumeBtn = frame.text(doc, 'button', 'ss-btn pri', t('journal.window.sources.email.scan.resume'), {
          type: 'button',
          'data-email-action': 'resume-scan',
        });
        if (state.busy) { resumeBtn.disabled = true; resumeBtn.setAttribute('disabled', 'disabled'); }
        resumeBtn.addEventListener('click', resumeScan);
        actions.appendChild(resumeBtn);

        const discBtn = frame.text(doc, 'button', 'ss-btn quiet', t('journal.window.sources.selective.action.disconnect'), {
          type: 'button',
          'data-email-action': 'disconnect',
        });
        discBtn.addEventListener('click', askDisconnect);
        actions.appendChild(discBtn);
      } else {
        const cancelBtn = frame.text(doc, 'button', 'ss-btn', t('journal.window.sources.email.scan.cancel'), {
          type: 'button',
          'data-email-action': 'cancel-scan',
        });
        if (state.busy) { cancelBtn.disabled = true; cancelBtn.setAttribute('disabled', 'disabled'); }
        cancelBtn.addEventListener('click', cancelScan);
        actions.appendChild(cancelBtn);
      }
      body.appendChild(actions);
    }

    function renderChooseScreen(body) {
      const snap = state.status || {};
      const scan = snap.scan || {};
      const acc = snap.accounting || {};
      const lo = (state.preview && state.preview.left_out) || acc.left_out || { total: 0 };
      const scannedCount = frame.formatCount(scan.checked || scan.total || 0);
      const eligibleCount = Number(scan.eligible_sent || 0);
      const earliest = (state.preview && state.preview.earliest_month) || scan.earliest_month || '';

      body.appendChild(frame.text(doc, 'h2', 'ss-section-heading', t('journal.window.sources.email.choose.heading')));

      let summaryLine;
      if (eligibleCount === 0) {
        summaryLine = t('journal.window.sources.email.choose.summary.no.sent', [['scanned', scannedCount]]);
      } else if (lo.total > 0 && earliest) {
        summaryLine = t('journal.window.sources.email.choose.summary', [
          ['scanned', scannedCount],
          ['eligible', frame.formatCount(eligibleCount)],
          ['earliest', earliest],
          ['left_out', frame.formatCount(lo.total)],
        ]);
      } else {
        summaryLine = t('journal.window.sources.email.choose.summary.no.leftout', [
          ['scanned', scannedCount],
          ['eligible', frame.formatCount(eligibleCount)],
          ['earliest', earliest || t('journal.window.sources.selective.range.all')],
        ]);
      }
      body.appendChild(frame.text(doc, 'p', 'ss-detail-lede', summaryLine));

      const capWarning = (state.preview && state.preview.folder_cap_warning) || scan.folder_cap_warning;
      if (capWarning) {
        body.appendChild(frame.renderNotice(doc, {
          tone: 'warn',
          text: t('journal.window.sources.email.choose.folder.cap'),
        }));
      }

      if (lo.total > 0) {
        body.appendChild(frame.renderLeftOutDisclosure(doc, {
          summary: t('journal.window.sources.email.choose.left.out.summary', [['count', frame.formatCount(lo.total)]]),
          items: leftOutItems(lo),
          note: t('journal.window.sources.email.choose.left.out.note'),
        }));
      }

      renderChoiceControls(body, false);
    }

    function renderFetchingScreen(body) {
      const sync = (state.status && state.status.sync) || {};
      const done = Number(sync.done || 0);
      const total = Number(sync.total || 0);

      body.appendChild(frame.text(doc, 'h2', 'ss-section-heading', t('journal.window.sources.email.fetch.heading')));
      body.appendChild(frame.renderProgress(doc, {
        value: done,
        max: total,
        paused: false,
        label: t('journal.window.sources.email.fetch.progress', [
          ['done', frame.formatCount(done)],
          ['total', frame.formatCount(total)],
        ]),
      }));
      body.appendChild(frame.text(doc, 'p', 'ss-act-hint', t('journal.window.sources.email.fetch.explanation')));
      const actions = frame.el(doc, 'div', 'ss-act-row');
      actions.appendChild(frame.link(doc, 'ss-action-link', t('journal.window.sources.email.fetch.status.link'), HEALTH_HREF));
      body.appendChild(actions);
    }

    function renderSteadyScreen(body) {
      const snap = state.status || {};
      if (state.widenMode) {
        body.appendChild(frame.text(doc, 'h2', 'ss-section-heading', t('journal.window.sources.email.widen.heading')));
        body.appendChild(frame.text(doc, 'p', 'ss-detail-lede', t('journal.window.sources.email.widen.intro')));
        renderChoiceControls(body, true);
        return;
      }

      if (state.noticeText) {
        body.appendChild(frame.renderNotice(doc, { tone: state.noticeTone || 'ok', text: state.noticeText }));
      }
      if (state.errorMsg) {
        body.appendChild(frame.renderNotice(doc, { tone: 'err', text: state.errorMsg }));
      }

      // Per-run Trash inline receipt
      if (state.runReceipt) {
        if (state.runReceipt.recovered) {
          body.appendChild(frame.renderNotice(doc, {
            tone: 'ok',
            text: t('journal.window.sources.email.runs.receipt.recovered'),
          }));
        } else if (state.runReceipt.recoverError) {
          const children = [
            frame.text(doc, 'span', null, t('journal.window.sources.email.runs.receipt.recover.failed')),
            frame.el(doc, 'span', 'ss-receipt-actions'),
          ];
          children[1].appendChild(frame.link(doc, 'ss-action-link', t('journal.window.sources.email.runs.receipt.open.trash'), TRASH_HREF));
          body.appendChild(frame.renderNotice(doc, { tone: 'err', children }));
        } else {
          const movedCount = state.runReceipt.messages;
          const actSpan = frame.el(doc, 'span', 'ss-receipt-actions');
          if (state.runReceipt.trashIds.length > 0) {
            const recBtn = frame.text(doc, 'button', 'ss-receipt-btn', t('journal.window.sources.email.runs.receipt.recover'), {
              type: 'button',
              'data-email-action': 'recover-run',
            });
            recBtn.addEventListener('click', recoverRunReceipt);
            actSpan.appendChild(recBtn);
          }
          actSpan.appendChild(frame.link(doc, 'ss-action-link', t('journal.window.sources.email.runs.receipt.open.trash'), TRASH_HREF));
          body.appendChild(frame.renderNotice(doc, {
            tone: 'ok',
            children: [
              frame.text(doc, 'span', null, t('journal.window.sources.email.runs.receipt.moved', [
                ['quantity', movedCount],
                ['count', frame.formatCount(movedCount)],
              ])),
              actSpan,
            ],
          }));
        }
      }

      // AC 5 facts block
      const acc = snap.accounting || {};
      const scan = snap.scan || {};
      const sync = snap.sync || {};
      const choice = snap.choice || null;
      const lo = acc.left_out || { total: 0 };
      const nowMs = nowFn();

      const facts = [
        {
          key: 'sent',
          label: t('journal.window.sources.email.steady.fact.sent'),
          value: frame.formatCount(acc.sent_total || 0),
          sub: scan.earliest_month
            ? t('journal.window.sources.email.steady.fact.sent.span', [['earliest', scan.earliest_month]])
            : '',
        },
        {
          key: 'in_journal',
          label: t('journal.window.sources.email.steady.fact.in.journal'),
          value: frame.formatCount(acc.in_journal || 0),
        },
      ];
      if ((acc.removed_by_owner || 0) > 0) {
        facts.push({
          key: 'deleted',
          label: t('journal.window.sources.email.steady.fact.deleted'),
          value: frame.formatCount(acc.removed_by_owner),
        });
      }
      const allTypesTicked = Boolean(
        choice && Array.isArray(choice.types) && TYPE_ORDER.every((mt) => choice.types.indexOf(mt) !== -1)
      );
      facts.push({
        key: 'outside',
        label: t('journal.window.sources.email.steady.fact.outside'),
        value: frame.formatCount(acc.outside_choice || 0),
        sub: (acc.outside_choice || 0) > 0 && choice && allTypesTicked ? outsideSubLabel(choice.range) : '',
      });
      facts.push({
        key: 'left_out',
        label: t('journal.window.sources.email.steady.fact.left.out'),
        value: frame.formatCount(lo.total || 0),
        sub: (lo.total || 0) > 0 ? leftOutInlineSummary(lo, frame) : '',
      });
      if (choice) {
        const typesStr = (choice.types || []).map(typeLabel).join(', ');
        facts.push({
          key: 'choice',
          label: t('journal.window.sources.email.steady.fact.choice'),
          value: rangeLabel(choice.range) + (typesStr ? ' \u00b7 ' + typesStr : ''),
        });
      }
      facts.push({
        key: 'last_added',
        label: t('journal.window.sources.email.steady.fact.last.added'),
        value: sync.last_added_at
          ? frame.relativeTime(sync.last_added_at, nowMs, timeZone)
          : t('journal.window.sources.copy.nothing.nothing.added.yet'),
      });
      if (sync.last_checked_at) {
        facts.push({
          key: 'last_checked',
          label: t('journal.window.sources.email.steady.fact.last.checked'),
          value: frame.relativeTime(sync.last_checked_at, nowMs, timeZone),
        });
      }
      body.appendChild(frame.renderFactsList(doc, facts));

      // Quiet boundary notes
      const notes = frame.el(doc, 'div', 'ss-boundary-notes');
      notes.appendChild(frame.text(doc, 'span', null, t('journal.window.sources.email.steady.boundary1')));
      notes.appendChild(frame.text(doc, 'span', null, t('journal.window.sources.email.steady.boundary2')));
      notes.appendChild(frame.text(doc, 'span', null, t('journal.window.sources.email.steady.boundary3')));
      body.appendChild(notes);

      // Actions
      const actions = frame.el(doc, 'div', 'ss-act-row');
      const widenBtn = frame.text(doc, 'button', 'ss-btn pri', t('journal.window.sources.email.steady.action.change'), {
        type: 'button',
        'data-email-action': 'open-widen',
      });
      widenBtn.addEventListener('click', () => {
        state.widenMode = true;
        state.replacePwOpen = false;
        state.errorMsg = '';
        state.selectedRange = (choice && choice.range) || '30d';
        state.selectedTypes = (choice && Array.isArray(choice.types) && choice.types.length > 0)
          ? choice.types.slice()
          : ['replies', 'new_messages', 'forwards'];
        paint();
        fetchPreview();
      });
      actions.appendChild(widenBtn);

      const checkBtn = frame.text(doc, 'button', 'ss-btn', t('journal.window.sources.email.steady.action.check'), {
        type: 'button',
        'data-email-action': 'check-now',
      });
      if (state.busy) { checkBtn.disabled = true; checkBtn.setAttribute('disabled', 'disabled'); }
      checkBtn.addEventListener('click', triggerSync);
      actions.appendChild(checkBtn);

      const pwBtn = frame.text(doc, 'button', 'ss-btn', t('journal.window.sources.email.steady.action.password'), {
        type: 'button',
        'data-email-action': 'toggle-password',
      });
      pwBtn.addEventListener('click', () => {
        state.replacePwOpen = !state.replacePwOpen;
        state.errorMsg = '';
        paint();
      });
      actions.appendChild(pwBtn);

      const discBtn = frame.text(doc, 'button', 'ss-btn quiet', t('journal.window.sources.selective.action.disconnect'), {
        type: 'button',
        'data-email-action': 'disconnect',
      });
      discBtn.addEventListener('click', askDisconnect);
      actions.appendChild(discBtn);
      body.appendChild(actions);

      // Inline password replacement form (when opened)
      if (state.replacePwOpen) {
        const pwSec = frame.el(doc, 'div', 'ss-choice-sec', { 'data-password-panel': 'steady' });
        pwSec.appendChild(renderPasswordBox({
          id: 'email-new-password',
          label: t('journal.window.sources.email.attention.password.field'),
          value: state.newPasswordInput,
          shown: state.showNewPassword,
          showGoogleLink: true,
          onInput: (v) => { state.newPasswordInput = v; },
          onToggle: () => { state.showNewPassword = !state.showNewPassword; paint(); },
        }));
        const pwActs = frame.el(doc, 'div', 'ss-act-row');
        const saveBtn = frame.text(doc, 'button', 'ss-btn pri', t('journal.window.sources.email.attention.password.save'), {
          type: 'button',
          'data-email-action': 'save-password',
        });
        if (state.busy) { saveBtn.disabled = true; saveBtn.setAttribute('disabled', 'disabled'); }
        saveBtn.addEventListener('click', submitNewPassword);
        pwActs.appendChild(saveBtn);

        const cancelBtn = frame.text(doc, 'button', 'ss-btn', t('journal.window.sources.selective.action.cancel'), { type: 'button' });
        cancelBtn.addEventListener('click', () => {
          state.replacePwOpen = false;
          state.newPasswordInput = '';
          state.errorMsg = '';
          paint();
        });
        pwActs.appendChild(cancelBtn);
        pwSec.appendChild(pwActs);
        body.appendChild(pwSec);
      }

      // Collapsed `past syncs` disclosure (per-run Trash, never whole-source delete)
      const runs = Array.isArray(snap.runs) ? snap.runs : [];
      if (runs.length > 0) {
        const fold = frame.el(doc, 'details', 'ss-fold', { 'data-past-syncs': 'true' });
        fold.appendChild(frame.text(doc, 'summary', null, t('journal.window.sources.email.runs.summary')));
        const foldBody = frame.el(doc, 'div', 'ss-fold-body');
        runs.forEach((run) => {
          const row = frame.el(doc, 'div', 'ss-batch-row', { 'data-run-id': run.id });
          const datePart = frame.formatShortDate(run.synced_at, timeZone);
          const rangePart = run.range_label ? rangeLabel(run.range_label) : '';
          const msgCount = Number(run.live_messages != null ? run.live_messages : run.messages || 0);
          const msgPart = t('journal.window.sources.email.choose.range.count', [
            ['quantity', msgCount],
            ['count', frame.formatCount(msgCount)],
          ]);
          const parts = [datePart, rangePart, msgPart].filter(Boolean);
          row.appendChild(frame.text(doc, 'span', null, parts.join(' \u00b7 ')));

          const trashBtn = frame.text(doc, 'button', 'ss-btn sm', t('journal.window.sources.email.runs.move.to.trash'), {
            type: 'button',
            'data-trash-run': run.id,
          });
          trashBtn.addEventListener('click', () => askTrashRun(run));
          row.appendChild(trashBtn);
          foldBody.appendChild(row);
        });
        fold.appendChild(foldBody);
        body.appendChild(fold);
      }
    }

    function renderNeedsAttentionScreen(body) {
      const snap = state.status || {};
      const reason = snap.attention_reason || 'password_rejected';
      const isPassword = reason === 'password_rejected' || reason === 'auth_failed';
      const isImap = reason === 'all_mail_hidden' || reason === 'imap_disabled';

      if (isPassword) {
        body.appendChild(frame.renderNotice(doc, {
          tone: 'err',
          text: t('journal.window.sources.email.attention.password.alert'),
        }));
        body.appendChild(renderPasswordBox({
          id: 'email-new-password',
          label: t('journal.window.sources.email.attention.password.field'),
          value: state.newPasswordInput,
          shown: state.showNewPassword,
          showGoogleLink: true,
          onInput: (v) => { state.newPasswordInput = v; },
          onToggle: () => { state.showNewPassword = !state.showNewPassword; paint(); },
        }));
        if (state.errorMsg) {
          body.appendChild(frame.renderNotice(doc, { tone: 'err', text: state.errorMsg }));
        }
        const actions = frame.el(doc, 'div', 'ss-act-row');
        const saveBtn = frame.text(doc, 'button', 'ss-btn pri', t('journal.window.sources.email.attention.password.save'), {
          type: 'button',
          'data-email-action': 'save-password',
        });
        if (state.busy) { saveBtn.disabled = true; saveBtn.setAttribute('disabled', 'disabled'); }
        saveBtn.addEventListener('click', submitNewPassword);
        actions.appendChild(saveBtn);

        const discBtn = frame.text(doc, 'button', 'ss-btn quiet', t('journal.window.sources.selective.action.disconnect'), {
          type: 'button',
          'data-email-action': 'disconnect',
        });
        discBtn.addEventListener('click', askDisconnect);
        actions.appendChild(discBtn);
        body.appendChild(actions);
        return;
      }

      // IMAP blocked or unreachable: distinct recovery notice + try again (NO password prompt)
      body.appendChild(frame.renderNotice(doc, {
        tone: 'err',
        text: isImap
          ? t('journal.window.sources.email.error.imap')
          : t('journal.window.sources.email.error.unreachable'),
      }));
      if (state.errorMsg) {
        body.appendChild(frame.renderNotice(doc, { tone: 'err', text: state.errorMsg }));
      }
      const actions = frame.el(doc, 'div', 'ss-act-row');
      const retryBtn = frame.text(doc, 'button', 'ss-btn pri', t('journal.window.sources.selective.action.try.again'), {
        type: 'button',
        'data-email-action': 'retry-sync',
      });
      if (state.busy) { retryBtn.disabled = true; retryBtn.setAttribute('disabled', 'disabled'); }
      retryBtn.addEventListener('click', triggerSync);
      actions.appendChild(retryBtn);

      const discBtn = frame.text(doc, 'button', 'ss-btn quiet', t('journal.window.sources.selective.action.disconnect'), {
        type: 'button',
        'data-email-action': 'disconnect',
      });
      discBtn.addEventListener('click', askDisconnect);
      actions.appendChild(discBtn);
      body.appendChild(actions);
    }

    function renderDisconnectedScreen(body) {
      const acc = (state.status && state.status.accounting) || {};
      const inJournal = Number(acc.in_journal || 0);
      const note = inJournal > 0
        ? t('journal.window.sources.email.disconnected.note', [
          ['quantity', inJournal],
          ['count', frame.formatCount(inJournal)],
        ])
        : t('journal.window.sources.email.disconnected.note.none');
      body.appendChild(frame.text(doc, 'p', 'ss-detail-lede', note));
      const actions = frame.el(doc, 'div', 'ss-act-row');
      const reconnectBtn = frame.text(doc, 'button', 'ss-btn pri', t('journal.window.sources.email.disconnected.reconnect'), {
        type: 'button',
        'data-email-action': 'reconnect',
      });
      reconnectBtn.addEventListener('click', () => {
        state.reconnectMode = true;
        state.errorMsg = '';
        paint();
      });
      actions.appendChild(reconnectBtn);
      body.appendChild(actions);
    }

    // ---- Top-level paint ----

    function paint() {
      if (ended()) return;
      closeModal();
      root.textContent = '';

      if (state.loadFailed) {
        const shell = frame.renderDetailShell(doc, {
          title: t('journal.window.sources.email.title'),
          state: 'unavailable',
        });
        shell.body.appendChild(frame.renderNotice(doc, {
          tone: 'err',
          text: t('journal.window.sources.email.error.load'),
        }));
        const actions = frame.el(doc, 'div', 'ss-act-row');
        const retryBtn = frame.text(doc, 'button', 'ss-btn', t('journal.window.sources.selective.action.try.again'), {
          type: 'button',
          'data-email-action': 'retry-load',
        });
        retryBtn.addEventListener('click', () => {
          state.loadFailed = false;
          fetchStatus(false);
        });
        actions.appendChild(retryBtn);
        shell.body.appendChild(actions);
        root.appendChild(shell.card);
        return;
      }

      if (!state.status) return;
      const snap = state.status;
      const phase = snap.phase;
      const showConnect = phase === 'not_connected' || (phase === 'disconnected' && state.reconnectMode);

      const shell = frame.renderDetailShell(doc, {
        title: t('journal.window.sources.email.title'),
        subtitle: showConnect ? '' : accountSubtitle(snap),
        state: showConnect ? 'ready_to_set_up' : snap.state,
        lede: showConnect ? t('journal.window.sources.email.connect.lede') : '',
      });

      if (showConnect) {
        renderConnectScreen(shell.body);
      } else if (snap.state === 'needs_attention' || phase === 'needs_attention') {
        renderNeedsAttentionScreen(shell.body);
      } else if (phase === 'scanning') {
        renderScanScreen(shell.body, false);
      } else if (phase === 'scan_paused') {
        renderScanScreen(shell.body, true);
      } else if (phase === 'awaiting_choice') {
        renderChooseScreen(shell.body);
      } else if (phase === 'fetching') {
        renderFetchingScreen(shell.body);
      } else if (phase === 'disconnected' || snap.state === 'off') {
        renderDisconnectedScreen(shell.body);
      } else {
        renderSteadyScreen(shell.body);
      }

      root.appendChild(shell.card);
    }

    fetchStatus(false).catch((error) => {
      if (!ended()) global.logError?.(error, { context: 'sources-email' });
    });

    return {
      dispose() {
        clearPoll();
        closeModal();
      },
    };
  }

  if (global.SolSelectiveSync && typeof global.SolSelectiveSync.registerDetail === 'function') {
    global.SolSelectiveSync.registerDetail('email', { mount });
  }
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = { mount };
  }
})(typeof window !== 'undefined' ? window : globalThis);
