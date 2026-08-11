// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function (global) {
  'use strict';

  const COPY = {
    SHEET_TITLE: 'SPK_SHEET_TITLE',
    SHEET_LEDE_MANY: 'SPK_SHEET_LEDE_MANY',
    SHEET_LEDE_ONE: 'SPK_SHEET_LEDE_ONE',
    SHELF_CANDIDATES: 'SPK_SHELF_CANDIDATES',
    SHELF_NO_EVIDENCE: 'SPK_SHELF_NO_EVIDENCE',
    EVIDENCE_SCREEN_MANY: 'SPK_EVIDENCE_SCREEN_MANY',
    EVIDENCE_SCREEN_ONE: 'SPK_EVIDENCE_SCREEN_ONE',
    EVIDENCE_MEETING_MANY: 'SPK_EVIDENCE_MEETING_MANY',
    EVIDENCE_MEETING_ONE: 'SPK_EVIDENCE_MEETING_ONE',
    SHELF_MENTIONS: 'SPK_SHELF_MENTIONS',
    ANCHOR: 'SPK_ANCHOR',
    ANCHOR_HAS_VOICE: 'SPK_ANCHOR_HAS_VOICE',
    SEARCH_LABEL: 'SPK_SEARCH_LABEL',
    SEARCH_PLACEHOLDER: 'SPK_SEARCH_PLACEHOLDER',
    THIS_IS_ME: 'SPK_THIS_IS_ME',
    THIS_IS_ME_GUIDANCE: 'SPK_THIS_IS_ME_GUIDANCE',
    SEARCH_NO_RESULTS: 'SPK_SEARCH_NO_RESULTS',
    CREATE_ROW: 'SPK_CREATE_ROW',
    NEAR_MATCH_BAND: 'SPK_NEAR_MATCH_BAND',
    KEEP_SEPARATE_TITLE: 'SPK_KEEP_SEPARATE_TITLE',
    KEEP_SEPARATE_BODY: 'SPK_KEEP_SEPARATE_BODY',
    KEEP_SEPARATE_CONFIRM: 'SPK_KEEP_SEPARATE_CONFIRM',
    KEEP_SEPARATE_DECLINE: 'SPK_KEEP_SEPARATE_DECLINE',
    PREVIEW_TITLE: 'SPK_PREVIEW_TITLE',
    PREVIEW_BODY_FRESH: 'SPK_PREVIEW_BODY_FRESH',
    PREVIEW_BODY_HAS_VOICE: 'SPK_PREVIEW_BODY_HAS_VOICE',
    PREVIEW_FACTS: 'SPK_PREVIEW_FACTS',
    PREVIEW_CONFIRM: 'SPK_PREVIEW_CONFIRM',
    PREVIEW_BACK: 'SPK_PREVIEW_BACK',
    RECEIPT_TITLE: 'SPK_RECEIPT_TITLE',
    RECEIPT_BODY: 'SPK_RECEIPT_BODY',
    RECEIPT_UNDO: 'SPK_RECEIPT_UNDO',
    UNDO_DONE: 'SPK_UNDO_DONE',
    UNDO_PARTIAL: 'SPK_UNDO_PARTIAL',
    EXIT_NOT_PERSON: 'SPK_EXIT_NOT_PERSON',
    EXIT_NOT_NOW: 'SPK_EXIT_NOT_NOW',
    NOT_PERSON_DONE: 'SPK_NOT_PERSON_DONE',
    NOT_NOW_DONE: 'SPK_NOT_NOW_DONE',
    ACTION_WHO_IS_THIS: 'SPK_ACTION_WHO_IS_THIS',
    LOAD_ERROR: 'SPK_LOAD_ERROR',
    SEARCH_ERROR: 'SPK_SEARCH_ERROR',
    CHECK_NAME_ERROR: 'SPK_CHECK_NAME_ERROR',
    SAMPLE_UNAVAILABLE: 'SPK_SAMPLE_UNAVAILABLE',
    ACTION_RETRY: 'SPK_ACTION_RETRY',
  };
  const DISMISS_NOT_PERSON = 'not_a_person';
  const DISMISS_QUIET = 'quiet';
  const FOCUSABLE_SELECTOR = 'button:not([disabled]), input:not([disabled]), a[href], [tabindex]:not([tabindex="-1"])';
  const UNDO_CATEGORIES = ['labels', 'corrections', 'voiceprints', 'tracker', 'sentinel', 'entity'];

  function templateText(template, values) {
    return String(template ?? '').replace(/\{([a-z_]+)\}/g, (_match, key) => {
      const value = values && Object.prototype.hasOwnProperty.call(values, key)
        ? values[key]
        : '';
      return String(value ?? '');
    });
  }

  function copyText(copy, key, values) {
    const text = copy && copy[key] ? copy[key] : '';
    return values ? templateText(text, values) : String(text);
  }

  function clear(node) {
    if (!node) return;
    if (typeof node.replaceChildren === 'function') {
      node.replaceChildren();
      return;
    }
    while (node.firstChild) node.removeChild(node.firstChild);
  }

  function el(doc, tag, className) {
    const node = doc.createElement(tag);
    if (className) node.className = className;
    return node;
  }

  function textEl(doc, tag, className, text) {
    const node = el(doc, tag, className);
    node.textContent = String(text ?? '');
    return node;
  }

  function append(parent, child) {
    parent.appendChild(child);
    return child;
  }

  function buttonEl(doc, className, text, handler) {
    const node = textEl(doc, 'button', className, text);
    node.type = 'button';
    if (handler) node.addEventListener('click', handler);
    return node;
  }

  function positiveCount(value) {
    const count = Math.trunc(Number(value));
    return Number.isFinite(count) && count > 0 ? count : 0;
  }

  function firstName(name) {
    return String(name || '').trim().split(/\s+/)[0] || String(name || '').trim();
  }

  function normalizePerson(row) {
    return {
      entity_id: String(row?.entity_id ?? row?.id ?? ''),
      name: String(row?.name ?? row?.entity_name ?? row?.entity_id ?? row?.id ?? ''),
      has_voice: Boolean(row?.has_voice),
    };
  }

  function targetSignature(target) {
    if (!target) return '';
    const reviewed = Array.isArray(target.reviewed_near_match_entity_ids)
      ? target.reviewed_near_match_entity_ids.map((item) => String(item)).sort().join(',')
      : '';
    return [
      target.mode || '',
      target.entity_id || '',
      target.name || '',
      reviewed,
    ].join('|');
  }

  function summarizeUndoReport(result) {
    const undoReport = result?.undo_report || {};
    let restored = 0;
    let skipped = 0;
    for (const category of UNDO_CATEGORIES) {
      const data = undoReport[category] || {};
      restored += positiveCount(data.restored_count);
      skipped += positiveCount(data.skipped_count);
    }
    const entity = undoReport.entity || {};
    const blockedCategories = Array.isArray(entity.blocked_categories)
      ? entity.blocked_categories
      : [];
    return {
      restored,
      skipped,
      blocked_categories: blockedCategories,
      fully_restored: (
        (result?.status === 'undone' || result?.status === 'already_undone') &&
        skipped === 0 &&
        blockedCategories.length === 0
      ),
    };
  }

  function jsonRequest(apiJson, url, payload) {
    return apiJson(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
  }

  function defaultRequestId() {
    const time = Date.now().toString(36);
    const random = Math.random().toString(36).slice(2, 10);
    return `spkwit_${time}_${random}`;
  }

  class WhoIsThisController {
    constructor(options) {
      this.options = options || {};
      this.mount = this.options.mount;
      if (!this.mount) {
        throw new Error('mount is required');
      }
      this.doc = this.options.document || this.mount.ownerDocument;
      if (!this.doc) {
        throw new Error('document is required');
      }
      this.copy = this.options.copy || {};
      this.context = this.options.context || {};
      this.apiJson = this.options.apiJson;
      if (typeof this.apiJson !== 'function') {
        throw new Error('apiJson is required');
      }
      this.logError = typeof this.options.logError === 'function'
        ? this.options.logError
        : function noop() {};
      this.formatDateShort = typeof this.options.formatDateShort === 'function'
        ? this.options.formatDateShort
        : (value) => String(value || '');
      this.onThisIsMe = typeof this.options.onThisIsMe === 'function'
        ? this.options.onThisIsMe
        : function noop() {};
      this.onIdentified = typeof this.options.onIdentified === 'function'
        ? this.options.onIdentified
        : function noop() {};
      this.onDismissed = typeof this.options.onDismissed === 'function'
        ? this.options.onDismissed
        : function noop() {};
      this.onFullyRestoredUndo = typeof this.options.onFullyRestoredUndo === 'function'
        ? this.options.onFullyRestoredUndo
        : async function noop() {};
      const debounceMs = Number(this.options.debounceMs);
      this.debounceMs = Number.isFinite(debounceMs) ? Math.max(0, debounceMs) : 150;
      this.requestIdFactory = typeof this.options.requestIdFactory === 'function'
        ? this.options.requestIdFactory
        : defaultRequestId;
      this.opened = false;
      this.cluster = null;
      this.clusterId = null;
      this.trigger = null;
      this.presence = null;
      this.notice = '';
      this.searchSeq = 0;
      this.searchTimer = null;
      this.requestSignature = '';
      this.requestId = '';
      this.mainState = this.emptyMainState();
      this.buildShell();
    }

    emptyMainState() {
      return {
        query: '',
        people: [],
        searchComplete: false,
        searchError: false,
        scrollTop: 0,
      };
    }

    buildShell() {
      this.backdrop = el(this.doc, 'div', 'spk-who-backdrop');
      this.backdrop.hidden = true;
      this.dialog = el(this.doc, 'section', 'spk-who-dialog');
      this.dialog.setAttribute('role', 'dialog');
      this.dialog.setAttribute('aria-modal', 'true');
      this.dialog.setAttribute('aria-labelledby', 'spkWhoTitle');
      this.dialog.setAttribute('tabindex', '-1');
      append(this.backdrop, this.dialog);
      append(this.mount, this.backdrop);
      this.backdrop.addEventListener('click', (event) => {
        if (event.target === this.backdrop) this.close();
      });
      this.dialog.addEventListener('keydown', (event) => this.handleDialogKeydown(event));
    }

    destroy() {
      this.close({ restoreFocus: false });
      this.backdrop.remove();
    }

    setCopy(copy) {
      this.copy = copy || {};
    }

    open(args) {
      const options = args || {};
      this.cluster = options.cluster || {};
      this.clusterId = String(options.clusterId ?? this.cluster.cluster_id ?? '').trim();
      this.trigger = options.trigger || null;
      this.mainState = this.emptyMainState();
      this.presence = null;
      this.notice = '';
      this.requestSignature = '';
      this.requestId = '';
      this.opened = true;
      if (this.trigger) {
        this.trigger.setAttribute('aria-haspopup', 'dialog');
        this.trigger.setAttribute('aria-expanded', 'true');
      }
      this.backdrop.hidden = false;
      this.renderLoading();
      return this.loadPresence();
    }

    close(options) {
      const restoreFocus = !options || options.restoreFocus !== false;
      this.opened = false;
      this.presence = null;
      this.cluster = null;
      this.clusterId = null;
      this.notice = '';
      this.mainState = this.emptyMainState();
      this.searchSeq += 1;
      if (this.searchTimer) {
        clearTimeout(this.searchTimer);
        this.searchTimer = null;
      }
      this.backdrop.hidden = true;
      clear(this.dialog);
      if (this.trigger) {
        this.trigger.setAttribute('aria-expanded', 'false');
        if (restoreFocus && typeof this.trigger.focus === 'function') {
          this.trigger.focus();
        }
      }
      this.trigger = null;
    }

    renderLoading() {
      clear(this.dialog);
      const title = textEl(this.doc, 'h2', 'spk-who-title', copyText(this.copy, COPY.SHEET_TITLE));
      title.setAttribute('id', 'spkWhoTitle');
      append(this.dialog, title);
      this.dialog.focus();
    }

    async loadPresence() {
      if (!this.clusterId) {
        this.renderLoadError(() => this.loadPresence());
        return null;
      }
      try {
        const payload = await this.apiJson(`/app/speakers/api/discovery/cluster/${encodeURIComponent(this.clusterId)}/presence`);
        if (!this.opened) return null;
        if (payload?.evidence_complete === false) {
          this.presence = null;
          this.renderLoadError(() => this.loadPresence());
          return null;
        }
        this.presence = payload || {};
        this.renderMain();
        return this.presence;
      } catch (error) {
        this.logError(error, { context: 'speakers-who-is-this:presence' });
        if (this.opened) this.renderLoadError(() => this.loadPresence());
        return null;
      }
    }

    renderFrame(stateClass) {
      clear(this.dialog);
      this.dialog.className = `spk-who-dialog ${stateClass || ''}`.trim();
      const title = textEl(this.doc, 'h2', 'spk-who-title', copyText(this.copy, COPY.SHEET_TITLE));
      title.setAttribute('id', 'spkWhoTitle');
      append(this.dialog, title);
      this.body = el(this.doc, 'div', 'spk-who-body');
      append(this.dialog, this.body);
      return this.body;
    }

    focusInitial(kind) {
      const focusTargets = {
        candidates: '.spk-who-person-action',
        search: '.spk-who-search-input',
        preview: '.spk-who-confirm',
        keep: '.spk-who-keep-confirm',
        receipt: '.spk-who-undo',
        error: '.spk-who-retry',
        dismissed: '.spk-who-dialog',
      };
      const selector = focusTargets[kind];
      const target = selector === '.spk-who-dialog'
        ? this.dialog
        : this.dialog.querySelector(selector);
      if (target && typeof target.focus === 'function') {
        target.focus();
      } else {
        this.dialog.focus();
      }
    }

    renderMain() {
      const body = this.renderFrame('spk-who-main-state');
      if (this.notice) {
        append(body, textEl(this.doc, 'p', 'spk-status spk-status-info spk-who-notice', this.notice));
        this.notice = '';
      }
      const facts = this.presence?.facts || {};
      const conversationCount = positiveCount(facts.conversation_count);
      const ledeKey = conversationCount === 1 ? COPY.SHEET_LEDE_ONE : COPY.SHEET_LEDE_MANY;
      append(body, textEl(this.doc, 'p', 'spk-who-lede', copyText(this.copy, ledeKey, {
        count: conversationCount,
      })));
      this.renderSamples(body, facts.samples || []);
      const candidates = this.presence?.candidates || {};
      const coPresence = Array.isArray(candidates.co_presence) ? candidates.co_presence : [];
      const mentions = Array.isArray(candidates.mention) ? candidates.mention : [];
      if (coPresence.length) {
        append(body, textEl(this.doc, 'h3', 'spk-who-shelf-title', copyText(this.copy, COPY.SHELF_CANDIDATES)));
        const list = el(this.doc, 'div', 'spk-who-people');
        coPresence.forEach((candidate) => append(list, this.personRow(normalizePerson(candidate), {
          evidence: this.evidenceLine(candidate),
        })));
        append(body, list);
      }
      if (mentions.length) {
        append(body, textEl(this.doc, 'h3', 'spk-who-shelf-title spk-who-shelf-muted', copyText(this.copy, COPY.SHELF_MENTIONS)));
        const list = el(this.doc, 'div', 'spk-who-mentions');
        mentions.forEach((candidate) => append(list, this.personRow(normalizePerson(candidate), {
          quiet: true,
        })));
        append(body, list);
      }
      if (!coPresence.length && !mentions.length) {
        append(body, textEl(this.doc, 'p', 'spk-who-no-evidence', copyText(this.copy, COPY.SHELF_NO_EVIDENCE)));
      }
      this.renderSearch(body);
      this.renderExitActions(body);
      if (this.body && this.mainState.scrollTop) {
        this.body.scrollTop = this.mainState.scrollTop;
      }
      if (!coPresence.length && !mentions.length) {
        this.focusInitial('search');
      } else {
        this.focusInitial('candidates');
      }
    }

    renderSamples(body, samples) {
      const visible = Array.isArray(samples) ? samples.slice(0, 3) : [];
      if (!visible.length) return;
      const shelf = el(this.doc, 'div', 'spk-who-samples');
      visible.forEach((sample) => append(shelf, this.sampleRow(sample)));
      append(body, shelf);
    }

    sampleLabel(sample) {
      const day = String(sample?.day || '');
      const dateLabel = this.formatDateShort(day);
      const place = String(sample?.setting || sample?.stream || '').trim();
      return [dateLabel, place].filter(Boolean).join(' · ');
    }

    sampleRow(sample) {
      const row = el(this.doc, 'div', 'spk-who-sample');
      append(row, textEl(this.doc, 'span', 'spk-who-sample-label', this.sampleLabel(sample)));
      const audioUrl = sample?.audio_url;
      if (!audioUrl) {
        this.markSampleUnavailable(row);
        return row;
      }
      const audio = el(this.doc, 'audio', 'spk-who-sample-audio');
      audio.controls = true;
      audio.preload = 'metadata';
      audio.src = String(audioUrl);
      audio.addEventListener('error', () => {
        this.markSampleUnavailable(row, audio);
      });
      append(row, audio);
      return row;
    }

    markSampleUnavailable(row, audio) {
      if (audio) {
        audio.hidden = true;
        audio.setAttribute('aria-hidden', 'true');
      }
      if (row.querySelector('.spk-who-sample-unavailable')) return;
      const status = textEl(this.doc, 'span', 'spk-who-sample-unavailable', copyText(this.copy, COPY.SAMPLE_UNAVAILABLE));
      status.setAttribute('role', 'status');
      row.setAttribute('aria-disabled', 'true');
      append(row, status);
    }

    evidenceLine(candidate) {
      const screen = positiveCount(candidate?.screen_conversations);
      if (screen > 0) {
        return copyText(this.copy, screen === 1 ? COPY.EVIDENCE_SCREEN_ONE : COPY.EVIDENCE_SCREEN_MANY, {
          count: screen,
        });
      }
      const meetings = positiveCount(candidate?.meeting_days);
      if (meetings > 0) {
        return copyText(this.copy, meetings === 1 ? COPY.EVIDENCE_MEETING_ONE : COPY.EVIDENCE_MEETING_MANY, {
          count: meetings,
        });
      }
      return '';
    }

    personRow(person, options) {
      const row = el(this.doc, 'div', options?.quiet ? 'spk-who-person spk-who-person-quiet' : 'spk-who-person');
      const action = buttonEl(this.doc, 'spk-overview-btn spk-who-person-action', person.name, () => {
        this.enterPreview({
          mode: 'attach',
          entity_id: person.entity_id,
          name: person.name,
          has_voice: Boolean(person.has_voice),
        });
      });
      append(row, action);
      if (options?.evidence) {
        append(row, textEl(this.doc, 'p', 'spk-card-line spk-who-evidence', options.evidence));
      }
      append(row, textEl(this.doc, 'p', 'spk-card-line spk-who-anchor', copyText(this.copy, person.has_voice ? COPY.ANCHOR_HAS_VOICE : COPY.ANCHOR)));
      return row;
    }

    renderSearch(body) {
      const section = el(this.doc, 'div', 'spk-who-search');
      const label = textEl(this.doc, 'label', 'spk-who-search-label', copyText(this.copy, COPY.SEARCH_LABEL));
      label.setAttribute('for', 'spkWhoSearch');
      append(section, label);
      const input = el(this.doc, 'input', 'spk-who-search-input');
      input.id = 'spkWhoSearch';
      input.type = 'search';
      input.value = this.mainState.query;
      input.placeholder = copyText(this.copy, COPY.SEARCH_PLACEHOLDER);
      input.addEventListener('input', () => {
        this.mainState.query = input.value;
        this.scheduleSearch(input.value);
      });
      append(section, input);
      this.searchResults = el(this.doc, 'div', 'spk-who-search-results');
      append(section, this.searchResults);
      append(body, section);
      this.renderSearchResults();
    }

    scheduleSearch(query) {
      this.mainState.query = String(query || '');
      if (this.searchTimer) {
        clearTimeout(this.searchTimer);
        this.searchTimer = null;
      }
      if (!this.mainState.query.trim()) {
        this.searchSeq += 1;
        this.mainState.people = [];
        this.mainState.searchComplete = false;
        this.mainState.searchError = false;
        this.renderSearchResults();
        return Promise.resolve(null);
      }
      if (this.debounceMs === 0) {
        return this.runSearch(this.mainState.query);
      }
      this.searchTimer = setTimeout(() => {
        this.searchTimer = null;
        this.runSearch(this.mainState.query);
      }, this.debounceMs);
      return Promise.resolve(null);
    }

    async runSearch(query) {
      const currentSeq = this.searchSeq + 1;
      this.searchSeq = currentSeq;
      const q = String(query || '').trim();
      if (!q) return null;
      try {
        const payload = await this.apiJson(`/app/speakers/api/people/search?q=${encodeURIComponent(q)}`);
        if (currentSeq !== this.searchSeq || !this.opened) return null;
        this.mainState.query = String(payload?.query ?? q);
        this.mainState.people = Array.isArray(payload?.people) ? payload.people.map(normalizePerson) : [];
        this.mainState.searchComplete = true;
        this.mainState.searchError = false;
        this.renderSearchResults();
        return payload;
      } catch (error) {
        this.logError(error, { context: 'speakers-who-is-this:people-search' });
        if (currentSeq !== this.searchSeq || !this.opened) return null;
        this.mainState.people = [];
        this.mainState.searchComplete = true;
        this.mainState.searchError = true;
        this.renderSearchResults();
        return null;
      }
    }

    renderSearchResults() {
      if (!this.searchResults) return;
      clear(this.searchResults);
      if (this.mainState.searchError) {
        append(this.searchResults, textEl(this.doc, 'p', 'spk-status spk-status-error', copyText(this.copy, COPY.SEARCH_ERROR)));
        append(this.searchResults, buttonEl(this.doc, 'spk-overview-btn spk-who-search-retry', copyText(this.copy, COPY.ACTION_RETRY), () => {
          this.runSearch(this.mainState.query);
        }));
        return;
      }
      if (!this.mainState.searchComplete) return;
      this.mainState.people.forEach((person) => append(this.searchResults, this.personRow(person, {
        quiet: true,
      })));
      if (!this.mainState.people.length) {
        append(this.searchResults, textEl(this.doc, 'p', 'spk-card-line spk-who-no-results', copyText(this.copy, COPY.SEARCH_NO_RESULTS, {
          query: this.mainState.query,
        })));
      }
      append(this.searchResults, buttonEl(this.doc, 'spk-overview-btn spk-who-create-row', copyText(this.copy, COPY.CREATE_ROW, {
        query: this.mainState.query,
      }), () => this.resolveCreateName(this.mainState.query)));
    }

    renderExitActions(body) {
      const actions = el(this.doc, 'div', 'spk-who-actions');
      append(actions, buttonEl(this.doc, 'spk-overview-btn spk-who-this-is-me', copyText(this.copy, COPY.THIS_IS_ME), () => {
        const cluster = this.cluster;
        const clusterId = this.clusterId;
        const trigger = this.trigger;
        this.close({ restoreFocus: false });
        this.onThisIsMe({ cluster, clusterId, trigger });
      }));
      append(actions, buttonEl(this.doc, 'spk-overview-btn spk-who-exit-not-person', copyText(this.copy, COPY.EXIT_NOT_PERSON), () => {
        this.dismissCluster(DISMISS_NOT_PERSON);
      }));
      append(actions, buttonEl(this.doc, 'spk-overview-btn spk-who-exit-not-now', copyText(this.copy, COPY.EXIT_NOT_NOW), () => {
        this.dismissCluster(DISMISS_QUIET);
      }));
      append(body, actions);
    }

    captureMainScroll() {
      if (this.body) this.mainState.scrollTop = positiveCount(this.body.scrollTop);
    }

    enterPreview(target) {
      this.captureMainScroll();
      const normalized = {
        mode: target.mode,
        entity_id: target.entity_id || '',
        name: target.name || '',
        has_voice: Boolean(target.has_voice),
        reviewed_near_match_entity_ids: Array.isArray(target.reviewed_near_match_entity_ids)
          ? target.reviewed_near_match_entity_ids.slice()
          : [],
      };
      const signature = targetSignature(normalized);
      if (signature !== this.requestSignature) {
        this.requestSignature = signature;
        this.requestId = this.requestIdFactory();
      }
      this.previewTarget = normalized;
      this.renderPreview();
    }

    renderPreview() {
      const target = this.previewTarget;
      const body = this.renderFrame('spk-who-preview-state');
      this.dialog.querySelector('.spk-who-title').textContent = copyText(this.copy, COPY.PREVIEW_TITLE, {
        name: target.name,
      });
      append(body, textEl(this.doc, 'p', 'spk-who-preview-body', copyText(this.copy, target.has_voice ? COPY.PREVIEW_BODY_HAS_VOICE : COPY.PREVIEW_BODY_FRESH, {
        name: target.name,
      })));
      const facts = this.presence?.facts || {};
      append(body, textEl(this.doc, 'p', 'spk-card-line spk-who-preview-facts', copyText(this.copy, COPY.PREVIEW_FACTS, {
        statements: positiveCount(facts.statement_count),
        conversations: positiveCount(facts.conversation_count),
      })));
      const actions = el(this.doc, 'div', 'spk-who-actions');
      append(actions, buttonEl(this.doc, 'spk-overview-btn spk-overview-btn-primary spk-who-confirm', copyText(this.copy, COPY.PREVIEW_CONFIRM, {
        first_name: firstName(target.name),
      }), () => this.commitPreview()));
      append(actions, buttonEl(this.doc, 'spk-overview-btn spk-who-preview-return', copyText(this.copy, COPY.PREVIEW_BACK), () => this.renderMain()));
      append(body, actions);
      this.focusInitial('preview');
    }

    async resolveCreateName(query) {
      const name = String(query || '').trim();
      if (!name) return;
      try {
        const result = await jsonRequest(this.apiJson, '/app/speakers/api/discovery/identify', {
          cluster_id: this.clusterId,
          name,
          create_new: true,
          resolve_only: true,
          entity_type: 'Person',
        });
        this.handleResolveResult(name, result);
      } catch (error) {
        this.logError(error, { context: 'speakers-who-is-this:resolve-name' });
        this.renderCheckNameError();
      }
    }

    handleResolveResult(name, result) {
      const status = result?.status;
      if (status === 'resolved') {
        const person = normalizePerson(result);
        this.renderNearMatches([person], {
          createName: name,
          allowCreate: false,
        });
        return;
      }
      if (status === 'ambiguous') {
        this.renderNearMatches((result.candidates || []).map(normalizePerson), {
          createName: name,
          allowCreate: true,
        });
        return;
      }
      if (status === 'no_match') {
        const matches = (result.candidates || []).map(normalizePerson);
        if (!matches.length) {
          this.enterPreview({
            mode: 'create',
            name,
            reviewed_near_match_entity_ids: [],
          });
          return;
        }
        this.renderNearMatches(matches, {
          createName: name,
          allowCreate: true,
        });
        return;
      }
      this.renderCheckNameError();
    }

    renderCheckNameError() {
      const body = this.renderFrame('spk-who-load-error-state');
      append(body, textEl(this.doc, 'p', 'spk-status spk-status-error', copyText(this.copy, COPY.CHECK_NAME_ERROR)));
      append(body, buttonEl(this.doc, 'spk-overview-btn spk-who-retry', copyText(this.copy, COPY.ACTION_RETRY), () => this.renderMain()));
      this.focusInitial('error');
    }

    renderNearMatches(matches, options) {
      this.captureMainScroll();
      const shown = Array.isArray(matches) ? matches.filter((match) => match.entity_id && match.name) : [];
      if (!shown.length && options.allowCreate) {
        this.enterPreview({
          mode: 'create',
          name: options.createName,
          reviewed_near_match_entity_ids: [],
        });
        return;
      }
      const body = this.renderFrame('spk-who-near-state');
      append(body, textEl(this.doc, 'h3', 'spk-who-shelf-title', copyText(this.copy, COPY.NEAR_MATCH_BAND)));
      shown.forEach((person) => append(body, this.personRow(person, {
        quiet: true,
      })));
      if (options.allowCreate) {
        append(body, buttonEl(this.doc, 'spk-overview-btn spk-who-create-row', copyText(this.copy, COPY.CREATE_ROW, {
          query: options.createName,
        }), () => this.renderKeepSeparate(options.createName, shown)));
      }
      append(body, buttonEl(this.doc, 'spk-overview-btn spk-who-preview-return', copyText(this.copy, COPY.PREVIEW_BACK), () => this.renderMain()));
      this.focusInitial('candidates');
    }

    renderKeepSeparate(name, matches) {
      const top = matches[0];
      const body = this.renderFrame('spk-who-keep-state');
      this.dialog.querySelector('.spk-who-title').textContent = copyText(this.copy, COPY.KEEP_SEPARATE_TITLE, {
        name: top.name,
      });
      append(body, textEl(this.doc, 'p', 'spk-who-keep-body', copyText(this.copy, COPY.KEEP_SEPARATE_BODY, {
        name: top.name,
      })));
      const actions = el(this.doc, 'div', 'spk-who-actions');
      append(actions, buttonEl(this.doc, 'spk-overview-btn spk-overview-btn-primary spk-who-keep-confirm', copyText(this.copy, COPY.KEEP_SEPARATE_CONFIRM), () => {
        this.enterPreview({
          mode: 'create',
          name,
          reviewed_near_match_entity_ids: matches.map((match) => match.entity_id),
        });
      }));
      append(actions, buttonEl(this.doc, 'spk-overview-btn spk-who-keep-decline', copyText(this.copy, COPY.KEEP_SEPARATE_DECLINE, {
        name: top.name,
      }), () => {
        this.enterPreview({
          mode: 'attach',
          entity_id: top.entity_id,
          name: top.name,
          has_voice: Boolean(top.has_voice),
        });
      }));
      append(body, actions);
      this.focusInitial('keep');
    }

    async commitPreview() {
      const target = this.previewTarget;
      if (!target) return;
      const payload = {
        cluster_id: this.clusterId,
        request_id: this.requestId,
        entity_type: 'Person',
      };
      if (target.mode === 'attach') {
        payload.entity_id = target.entity_id;
      } else {
        payload.name = target.name;
        payload.create_new = true;
        payload.reviewed_near_match_entity_ids = target.reviewed_near_match_entity_ids || [];
      }
      try {
        const result = await jsonRequest(this.apiJson, '/app/speakers/api/discovery/identify', payload);
        if (result?.status !== 'identified') {
          if (this.shouldRefreshReviewedSet(result, target)) {
            await this.refreshCreateNameGate(target.name);
            return;
          }
          this.logError(new Error(`unexpected identify status: ${result?.status || ''}`), {
            context: 'speakers-who-is-this:commit',
            result,
          });
          this.renderLoadError(() => this.commitPreview());
          return;
        }
        this.renderReceipt(result);
        this.onIdentified({ clusterId: this.clusterId, cluster: this.cluster, result });
      } catch (error) {
        if (this.shouldRefreshReviewedSet(error, target)) {
          this.logError(error, { context: 'speakers-who-is-this:stale-reviewed-set' });
          await this.refreshCreateNameGate(target.name);
          return;
        }
        this.logError(error, { context: 'speakers-who-is-this:commit' });
        this.renderLoadError(() => this.commitPreview());
      }
    }

    shouldRefreshReviewedSet(result, target) {
      if (!target || target.mode !== 'create' || !target.name) return false;
      const code = String(
        result?.invalid_request_code
        || result?.payload?.invalid_request_code
        || ''
      );
      return code === 'reviewed_near_match_set_mismatch';
    }

    async refreshCreateNameGate(name) {
      this.requestSignature = '';
      this.requestId = '';
      this.previewTarget = null;
      await this.resolveCreateName(name);
    }

    renderReceipt(result, partialMessage) {
      this.receiptResult = result;
      const body = this.renderFrame('spk-who-receipt-state');
      this.dialog.querySelector('.spk-who-title').textContent = copyText(this.copy, COPY.RECEIPT_TITLE, {
        name: result?.entity_name || result?.target_entity_name || this.previewTarget?.name || '',
      });
      append(body, textEl(this.doc, 'p', 'spk-who-receipt-body', copyText(this.copy, COPY.RECEIPT_BODY, {
        name: result?.entity_name || result?.target_entity_name || this.previewTarget?.name || '',
      })));
      if (partialMessage) {
        append(body, textEl(this.doc, 'p', 'spk-status spk-status-error spk-who-undo-partial', partialMessage));
      }
      append(body, buttonEl(this.doc, 'spk-overview-btn spk-who-undo', copyText(this.copy, COPY.RECEIPT_UNDO), () => this.undoReceipt()));
      this.focusInitial('receipt');
    }

    async undoReceipt() {
      const operationId = this.receiptResult?.operation_id;
      if (!operationId) {
        this.renderLoadError(() => this.undoReceipt());
        return;
      }
      try {
        const result = await jsonRequest(this.apiJson, '/app/speakers/api/discovery/identify/undo', {
          operation_id: operationId,
        });
        const summary = summarizeUndoReport(result);
        if (summary.fully_restored) {
          this.requestSignature = '';
          this.requestId = '';
          this.notice = copyText(this.copy, COPY.UNDO_DONE);
          await this.refreshFullyRestoredUndo(result);
          return;
        }
        this.renderReceipt(this.receiptResult, copyText(this.copy, COPY.UNDO_PARTIAL, {
          restored: summary.restored,
          skipped: summary.skipped,
        }));
      } catch (error) {
        this.logError(error, { context: 'speakers-who-is-this:undo' });
        this.renderLoadError(() => this.undoReceipt());
      }
    }

    async refreshFullyRestoredUndo(result) {
      try {
        await this.onFullyRestoredUndo({
          clusterId: this.clusterId,
          cluster: this.cluster,
          result,
        });
        this.presence = null;
        this.mainState = this.emptyMainState();
        const refreshed = await this.loadPresence();
        if (!refreshed) {
          throw new Error('presence refresh failed');
        }
      } catch (error) {
        this.logError(error, { context: 'speakers-who-is-this:full-undo-refresh' });
        if (this.opened) {
          this.renderLoadError(() => this.refreshFullyRestoredUndo(result));
        }
      }
    }

    async dismissCluster(disposition) {
      try {
        const result = await jsonRequest(this.apiJson, '/app/speakers/api/discovery/dismiss', {
          cluster_id: this.clusterId,
          disposition,
        });
        this.renderDismissed(disposition);
        this.onDismissed({ clusterId: this.clusterId, cluster: this.cluster, result });
      } catch (error) {
        this.logError(error, { context: 'speakers-who-is-this:dismiss' });
        this.renderLoadError(() => this.dismissCluster(disposition));
      }
    }

    renderDismissed(disposition) {
      const body = this.renderFrame('spk-who-dismissed-state');
      const key = disposition === DISMISS_NOT_PERSON ? COPY.NOT_PERSON_DONE : COPY.NOT_NOW_DONE;
      append(body, textEl(this.doc, 'p', 'spk-status spk-status-info spk-who-dismissed', copyText(this.copy, key)));
      this.focusInitial('dismissed');
    }

    renderLoadError(retry) {
      const body = this.renderFrame('spk-who-load-error-state');
      append(body, textEl(this.doc, 'p', 'spk-status spk-status-error', copyText(this.copy, COPY.LOAD_ERROR)));
      append(body, buttonEl(this.doc, 'spk-overview-btn spk-who-retry', copyText(this.copy, COPY.ACTION_RETRY), retry));
      this.focusInitial('error');
    }

    focusableElements() {
      return Array.from(this.dialog.querySelectorAll(FOCUSABLE_SELECTOR))
        .filter((node) => !node.disabled && !node.hidden);
    }

    handleDialogKeydown(event) {
      if (event.key === 'Escape') {
        event.preventDefault();
        this.close();
        return;
      }
      if (event.key !== 'Tab') return;
      const focusable = this.focusableElements();
      if (!focusable.length) {
        event.preventDefault();
        this.dialog.focus();
        return;
      }
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (event.shiftKey && event.target === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && event.target === last) {
        event.preventDefault();
        first.focus();
      }
    }
  }

  function init(options) {
    return new WhoIsThisController(options);
  }

  const SpeakersWhoIsThis = {
    init,
    summarizeUndoReport,
    templateText,
  };
  global.SpeakersWhoIsThis = SpeakersWhoIsThis;
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = SpeakersWhoIsThis;
  }
})(typeof window !== 'undefined' ? window : globalThis);
