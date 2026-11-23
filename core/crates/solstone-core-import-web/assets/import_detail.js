// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  'use strict';

  // A draw writes only what differs from what the page already shows, so a
  // re-read that changes nothing leaves the page as it is. The same setters
  // are in import's own script.
  function writeText(el, text) {
    if (!el) return;
    const next = String(text ?? '');
    // Equal text over a link or other markup is still a change.
    const nodes = el.childNodes;
    const same = el.textContent === next && (!nodes || (nodes.length === 0 ? next === '' : nodes.length === 1 && nodes[0].nodeType === 3));
    if (!same) el.textContent = next;
  }
  // An attribute set to its own value still counts as a change to the page.
  function writeAttr(el, name, value) {
    if (!el) return;
    if (value === null || value === undefined || value === false) {
      if (el.hasAttribute(name)) el.removeAttribute(name);
      return;
    }
    const next = value === true ? '' : String(value);
    if (el.getAttribute(name) !== next) el.setAttribute(name, next);
  }
  function writeHidden(el, hidden) {
    if (el && el.hidden !== !!hidden) el.hidden = !!hidden;
  }

  // --- owner-facing strings ---
  const strings = Object.freeze({
    unknown: '—',
    drawer_label: window.JournalI18n.render('journal.window.import.strings.drawer.label.bookkeeping'),
    leads_title: window.JournalI18n.render('journal.window.import.strings.leads.title.where.this.landed'),
    activities: window.JournalI18n.render('journal.window.import.strings.activities.activities'),
    view_day: window.JournalI18n.render('journal.window.import.strings.view.day.view.that.day'),
    upload_facts: window.JournalI18n.render('journal.window.import.strings.upload.facts.upload.facts'),
    processing_facts: window.JournalI18n.render('journal.window.import.strings.processing.facts.processing.facts'),
    merge_summary: window.JournalI18n.render('journal.window.import.strings.merge.summary.merge.summary'),
    merge_highlights: window.JournalI18n.render('journal.window.import.strings.merge.highlights.merge.highlights'),
    artifact_paths: window.JournalI18n.render('journal.window.import.strings.artifact.paths.artifact.paths'),
    created_files: window.JournalI18n.render('journal.window.import.strings.created.files.created.files'),
    original_file: window.JournalI18n.render('journal.window.import.strings.original.file.original.file'),
    file_size: window.JournalI18n.render('journal.window.import.strings.file.size.file.size'),
    mime_type: window.JournalI18n.render('journal.window.import.strings.mime.type.mime.type'),
    uploaded_at: window.JournalI18n.render('journal.window.import.strings.uploaded.at.uploaded.at'),
    detected_time: window.JournalI18n.render('journal.window.import.strings.detected.time.detected.time'),
    import_time: window.JournalI18n.render('journal.window.import.strings.import.time.import.time'),
    date_source: window.JournalI18n.render('journal.window.import.strings.date.source.date.source'),
    date_source_photo: window.JournalI18n.render('journal.window.import.strings.date.source.photo.from.photo'),
    date_source_fallback: window.JournalI18n.render('journal.window.import.strings.date.source.fallback.import.time.no.date.in.photo'),
    date_source_owner: window.JournalI18n.render('journal.window.import.strings.date.source.owner.chosen.by.you'),
    photo_fallback_notice: window.JournalI18n.render('journal.window.import.strings.photo.fallback.notice.no.date.in.photo.placed.at'),
    facets: window.JournalI18n.render('journal.window.import.strings.facets.facets'),
    setting: window.JournalI18n.render('journal.window.import.strings.setting.setting'),
    status: window.JournalI18n.render('journal.window.import.strings.status.status'),
    source: window.JournalI18n.render('journal.window.import.strings.source.source'),
    target_day: window.JournalI18n.render('journal.window.import.strings.target.day.target.day'),
    date_range: window.JournalI18n.render('journal.window.import.strings.date.range.date.range'),
    entries: window.JournalI18n.render('journal.window.import.strings.entries.entries'),
    entities: window.JournalI18n.render('journal.window.import.strings.entities.entities'),
    file: window.JournalI18n.render('journal.window.import.strings.file.file'),
    files: window.JournalI18n.render('journal.window.import.strings.files.files'),
    completed_at: window.JournalI18n.render('journal.window.import.strings.completed.at.completed.at'),
    failed_at: window.JournalI18n.render('journal.window.import.strings.failed.at.failed.at'),
    failed_stage: window.JournalI18n.render('journal.window.import.strings.failed.stage.failed.stage'),
    error: window.JournalI18n.render('journal.window.import.strings.error.error'),
    segments: window.JournalI18n.render('journal.window.import.strings.segments.segments'),
    imports: window.JournalI18n.render('journal.window.import.strings.imports.imports'),
    decisions: window.JournalI18n.render('journal.window.import.strings.decisions.decisions'),
    staging: window.JournalI18n.render('journal.window.import.strings.staging.staging'),
    staged_entities: window.JournalI18n.render('journal.window.import.strings.staged.entities.staged.entities'),
    summary_errors: window.JournalI18n.render('journal.window.import.strings.summary.errors.summary.errors'),
    raw_payload: window.JournalI18n.render('journal.window.import.strings.raw.payload.raw.payload'),
    drawer_unavailable: window.JournalI18n.render('journal.window.import.strings.drawer.unavailable.drawer.renderer.unavailable'),
    completed: window.JournalI18n.render('journal.window.import.strings.completed.completed'),
    failed: window.JournalI18n.render('journal.window.import.strings.failed.failed'),
    pending: window.JournalI18n.render('journal.window.import.strings.pending.pending'),
    running: window.JournalI18n.render('journal.window.import.strings.running.running'),
    unconfirmed: window.JournalI18n.render('journal.window.import.strings.unconfirmed.import.unconfirmed'),
    unconfirmed_explanation: window.JournalI18n.render('journal.window.import.strings.unconfirmed.explanation.this.import.couldn.t.be.confirmed'),
    status_unavailable: window.JournalI18n.render('journal.window.import.strings.status.unavailable.import.status.unavailable'),
    image_description_gap_detail: window.JournalI18n.render('journal.window.import.strings.image.description.gap.detail.image.saved.without.visual.description'),
    processing: window.JournalI18n.render('journal.window.import.strings.processing.processing'),
    saved: window.JournalI18n.render('journal.window.import.strings.saved.saved'),
    not_started: window.JournalI18n.render('journal.window.import.strings.not.started.not.started'),
    start_unknown: window.JournalI18n.render('journal.window.import.strings.start.unknown.start.status.unknown'),
    // An Apple Health import's own states, as its row says them. Owner copy: voice-checked.
    body_partly_saved: window.JournalI18n.render('journal.window.import.strings.body.partly.saved.partly.saved'),
    body_stopped: window.JournalI18n.render('journal.window.import.strings.body.stopped.stopped'),
    body_failed: window.JournalI18n.render('journal.window.import.strings.body.failed.not.brought.in'),
    body_didnt_start: window.JournalI18n.render('journal.window.import.strings.body.didnt.start.didn.t.start'),
    start_unknown_detail: window.JournalI18n.render('journal.window.import.strings.start.unknown.detail.this.file.is.saved.but.its'),
    starting: window.JournalI18n.render('journal.window.import.strings.starting.starting'),
    preview_import: window.JournalI18n.render('journal.window.import.strings.preview.import.preview.import'),
    preparing_preview: window.JournalI18n.render('journal.window.import.strings.preparing.preview.preparing.preview'),
    start_import: window.JournalI18n.render('journal.window.import.strings.start.import.start.import'),
    check_status: window.JournalI18n.render('journal.window.import.strings.check.status.check.status'),
    checking_status: window.JournalI18n.render('journal.window.import.strings.checking.status.checking.status'),
    nothing_to_import: window.JournalI18n.render('journal.window.import.strings.nothing.to.import.nothing.to.import'),
    preview_failed: window.JournalI18n.render('journal.window.import.strings.preview.failed.couldn.t.prepare.a.preview.try'),
    status_failed: window.JournalI18n.render('journal.window.import.strings.status.failed.couldn.t.load.the.import.s'),
    start_unconfirmed: window.JournalI18n.render('journal.window.import.strings.start.unconfirmed.couldn.t.confirm.whether.the.import'),
    failed_line: window.JournalI18n.render('journal.window.import.strings.failed.line.failed.while.processing'),
    files_created: window.JournalI18n.render('journal.window.import.strings.files.created.files.created'),
    owner_identity_differs: window.JournalI18n.render('journal.window.import.strings.owner.identity.differs.owner.identity.differs'),
    identity_differs: window.JournalI18n.render('journal.window.import.strings.identity.differs.identity.differs'),
    profile_link: window.JournalI18n.render('journal.window.import.strings.profile.link.review.profile.settings'),
    collision_title: window.JournalI18n.render('journal.window.import.strings.collision.title.owner.identity.differs.between.journals'),
    unavailable_description: window.JournalI18n.render('journal.window.import.strings.unavailable.description.unavailable.description'),
    unavailable_pages: window.JournalI18n.render('journal.window.import.strings.unavailable.pages.unavailable.pages'),
    has_gaps: window.JournalI18n.render('journal.window.import.strings.has.gaps.contains.gaps'),
    untimed_notice: window.JournalI18n.render('journal.window.import.strings.untimed.notice.no.turn.times.found.in.this'),
    untimed_ask: window.JournalI18n.render('journal.window.import.strings.untimed.ask.ask.us.to.add.it'),
    delete_import: window.JournalI18n.render('journal.window.import.strings.delete.import.delete.this.import'),
    ready_to_review: window.JournalI18n.render('journal.window.import.ox.status.ready.to.review'),
    try_preview_again: window.JournalI18n.render('journal.window.import.ox.detail.try.preview.again'),
    found_heading: window.JournalI18n.render('journal.window.import.ox.detail.found.heading'),
    space_insufficient: window.JournalI18n.render('journal.window.import.ox.detail.space.insufficient'),
    nothing_new: window.JournalI18n.render('journal.window.import.ox.detail.nothing.new'),
    facet_label: window.JournalI18n.render('journal.window.import.ox.detail.facet.label'),
    no_facet: window.JournalI18n.render('journal.window.import.ox.detail.no.facet'),
    notebook_label: window.JournalI18n.render('journal.window.import.handoff.notebook'),
    context_label: window.JournalI18n.render('journal.window.import.ox.detail.context.label'),
    context_hint: window.JournalI18n.render('journal.window.import.ox.detail.context.hint'),
    expiry_note: window.JournalI18n.render('journal.window.import.ox.detail.expiry.note'),
    discard_button: window.JournalI18n.render('journal.window.import.ox.detail.discard.button'),
    discard_title: window.JournalI18n.render('journal.window.import.ox.detail.discard.title'),
    discard_body: window.JournalI18n.render('journal.window.import.ox.detail.discard.body'),
    discard_confirm: window.JournalI18n.render('journal.window.import.ox.detail.discard.confirm'),
    discard_cancel: window.JournalI18n.render('journal.window.import.ox.detail.discard.cancel'),
    discard_failed: window.JournalI18n.render('journal.window.import.response.error.your.journal.couldn.t.finish.that'),
    expired_badge: window.JournalI18n.render('journal.window.import.ox.detail.expired.badge'),
    expired_body: window.JournalI18n.render('journal.window.import.ox.detail.expired.body'),
    expired_recovery: window.JournalI18n.render('journal.window.import.ox.detail.expired.recovery'),
    back_to_imports: window.JournalI18n.render('journal.window.import.ox.detail.back.to.imports'),
    file_size_units: Object.freeze(['b', 'kb', 'mb', 'gb'])
  });
  // --- end owner-facing strings ---

  // The support form, filled in as a report from the import app. The command line's
  // untimed notice links the same place (`cli_render::UNTIMED_TRANSCRIPT_SUPPORT_URL`).
  const UNTIMED_TRANSCRIPT_SUPPORT_URL =
    'https://support.solstone.app/#report=v1&app=import&state=transcript%20times%20not%20recognized';

  function escapeHtml(value) {
    return String(value ?? '').replace(/[&<>"']/g, (char) => ({
      '&': '&amp;',
      '<': '&lt;',
      '>': '&gt;',
      '"': '&quot;',
      "'": '&#39;'
    })[char]);
  }

  function hasValue(value) {
    return value !== undefined && value !== null && value !== '';
  }

  function asObject(value) {
    return value && typeof value === 'object' && !Array.isArray(value) ? value : null;
  }

  function asArray(value) {
    return Array.isArray(value) ? value : [];
  }

  function numberValue(value) {
    if (!hasValue(value)) return null;
    const number = Number(value);
    return Number.isFinite(number) ? number : null;
  }

  function basename(path) {
    return String(path ?? '').split('/').filter(Boolean).pop() || String(path ?? '');
  }

  function formatFileSize(bytes) {
    const size = numberValue(bytes);
    if (size === null) return null;
    const units = strings.file_size_units;
    if (size === 0) return `0 ${units[0]}`;
    const index = Math.min(units.length - 1, Math.floor(Math.log(size) / Math.log(1024)));
    return `${Number((size / Math.pow(1024, index)).toFixed(1))} ${units[index]}`;
  }

  function formatTimestamp(timestamp) {
    if (!hasValue(timestamp)) return null;
    const value = String(timestamp);
    const match = value.match(/^(\d{4})(\d{2})(\d{2})_(\d{2})(\d{2})(\d{2})$/);
    if (!match) return value;
    const [, year, month, day, hour, minute, sec] = match;
    return `${year}-${month}-${day} ${hour}:${minute}:${sec}`;
  }

  function formatDateTime(value) {
    if (!hasValue(value)) return null;
    const text = String(value);
    // Only a real instant (one carrying its offset) goes onto the journal's
    // clock. A bare wall time is already the journal's own and shows as sent.
    const isInstant = /(?:[zZ]|[+-]\d{2}:?\d{2})$/.test(text);
    if (isInstant && window.JournalClock?.formatInstant) {
      const probe = new Date(text);
      if (!Number.isNaN(probe.getTime())) return window.JournalClock.formatInstant(probe);
    }
    const isoMatch = text.match(/^(\d{4}-\d{2}-\d{2})[tT ](\d{2}:\d{2})(?::(\d{2}))?/);
    if (isoMatch) {
      return `${isoMatch[1]} ${isoMatch[2]}${isoMatch[3] ? `:${isoMatch[3]}` : ''}`;
    }
    const date = new Date(text);
    if (Number.isNaN(date.getTime())) return text;
    return date.toISOString().slice(0, 19).replace('T', ' ');
  }

  function formatDateRange(range) {
    const values = asArray(range).filter(hasValue);
    if (!values.length) return null;
    if (values.length === 1 || values[0] === values[1]) return String(values[0]);
    return `${values[0]} - ${values[1]}`;
  }

  function quantityArgs(count, name = 'count', quantity = 'quantity', display = String(count)) {
    return [[quantity, window.JournalI18n.cardinalQuantity(count)], [name, String(display)]];
  }

  function formatCount(value, message) {
    const count = numberValue(value);
    return count === null ? null : window.JournalI18n.render(message, quantityArgs(count));
  }

  function formatUnavailablePages(value) {
    if (!hasValue(value)) return null;
    if (Array.isArray(value)) {
      if (!value.length) return null;
      return value.join(', ');
    }
    const num = numberValue(value);
    if (num !== null) {
      return String(num);
    }
    return String(value);
  }

  function formatDuration(startIso, endIso, display) {
    if (!hasValue(startIso) || !hasValue(endIso)) return null;
    const start = new Date(String(startIso));
    const end = new Date(String(endIso));
    const startMs = start.getTime();
    const endMs = end.getTime();
    if (!Number.isFinite(startMs) || !Number.isFinite(endMs) || endMs < startMs) {
      return null;
    }
    return formatSeconds((endMs - startMs) / 1000, display);
  }

  function formatSeconds(seconds, display = 'elapsed') {
    const prefix = `journal.window.import.detail.duration.${display}`;
    if (seconds < 60) return window.JournalI18n.render(`${prefix}.short`);
    if (seconds < 90 * 60) {
      const minutes = Math.max(1, Math.round(seconds / 60));
      return window.JournalI18n.render(`${prefix}.minute`, quantityArgs(minutes));
    }
    if (seconds < 36 * 60 * 60) {
      const hours = Math.max(1, Math.round(seconds / 3600));
      return window.JournalI18n.render(`${prefix}.hour`, quantityArgs(hours));
    }
    const days = Math.max(1, Math.round(seconds / 86400));
    return window.JournalI18n.render(`${prefix}.day`, quantityArgs(days));
  }

  function statusClass(code) {
    return ['success', 'failed', 'running', 'unconfirmed', 'unavailable', 'review'].includes(code) ? code : 'pending';
  }

  function deriveStatus(data) {
    const principalCollision = asObject(data?.principal_collision);
    const canonicalStatus = data?.status;
    // An Apple Health import that didn't save everything reads as its row does.
    const bodyLabel = {
      partly_saved: strings.body_partly_saved,
      stopped: strings.body_stopped,
      failed: strings.body_failed,
      didnt_start: strings.body_didnt_start,
    }[data?.body_state];
    if (bodyLabel) {
      const warn = data.body_state === 'partly_saved' || data.body_state === 'stopped';
      return { code: data.body_state === 'failed' ? 'body_failed' : data.body_state, status: bodyLabel, chipText: bodyLabel, chipTone: warn ? 'warn' : 'danger', open: true, body: true };
    }

    if (canonicalStatus === 'failed') {
      return {
        code: 'failed', status: strings.failed,
        chipText: strings.failed,
        chipTone: 'danger',
        open: true
      };
    }
    if (canonicalStatus === 'unconfirmed') {
      return {
        code: 'unconfirmed', status: strings.unconfirmed,
        chipText: strings.unconfirmed,
        chipTone: 'warn',
        open: true
      };
    }
    if (canonicalStatus === 'unavailable') {
      return {
        code: 'unavailable', status: strings.status_unavailable,
        chipText: strings.status_unavailable,
        chipTone: 'warn',
        open: true
      };
    }
    if (canonicalStatus === 'success') {
      if (principalCollision) {
        return {
          code: 'success', status: strings.completed,
          chipText: strings.identity_differs,
          chipTone: 'warn',
          open: true
        };
      }
      return { code: 'success', status: strings.completed, chipText: '', chipTone: '', open: false };
    }
    if (canonicalStatus === 'running') {
      return { code: 'running', status: strings.running, chipText: '', chipTone: '', open: false };
    }
    if (data?.saved_start?.state === 'eligible' && !savedFlows.get(data?.timestamp)?.revoked) {
      return { code: 'review', status: strings.ready_to_review, chipText: strings.ready_to_review, chipTone: '', open: false };
    }
    if (data?.saved_start?.active === true) {
      return { code: 'running', status: strings.running, chipText: strings.starting, chipTone: '', open: false };
    }
    if (data?.saved_start) {
      return { code: 'start_unknown', status: strings.start_unknown, chipText: strings.start_unknown, chipTone: 'warn', open: true };
    }
    return { code: 'pending', status: strings.pending, chipText: '', chipTone: '', open: false };
  }

  function composeDrawerLine(data) {
    const importJson = asObject(data?.import_json) || {};
    const derived = deriveStatus(data);

    if (derived.body) return data?.body_words || derived.status;
    if (derived.code === 'saved' || derived.code === 'review') return strings.ready_to_review;
    if (derived.code === 'start_unknown') return strings.start_unknown_detail;
    if (data?.saved_start?.active === true && data?.status !== 'running') return strings.starting;

    if (derived.code === 'failed') return strings.failed_line;
    if (derived.code === 'unconfirmed') return strings.unconfirmed_explanation;
    if (derived.code === 'unavailable') return strings.status_unavailable;
    if (derived.code === 'running' || derived.code === 'pending') {
      return strings.processing;
    }

    // The card above already states the entry and file counts; restating them on
    // the drawer summary made a nine-line page spend two lines on a repeat
    // (G3-218). The drawer line carries only what the card does not.
    const clauses = [];
    const durationMs = numberValue(data?.duration_ms);
    const completedIn = durationMs !== null
      ? formatSeconds(durationMs / 1000, 'completed')
      : formatDuration(importJson.upload_datetime, data?.finished_at, 'completed');
    if (completedIn) clauses.push(completedIn);
    if (!clauses.length) {
      clauses.push(strings.completed);
    }
    if (asObject(data?.principal_collision)) {
      clauses.push(strings.owner_identity_differs);
    }
    return clauses.join(' · ');
  }

  function resolveDay(data) {
    const importedJson = asObject(data?.imported_json) || {};
    const range = importedJson.date_range || data?.date_range || null;
    const day = importedJson.target_day || data?.target_day || range?.[0] || null;
    return hasValue(day) ? String(day) : null;
  }

  function createdFileHref(path) {
    const match = String(path ?? '').match(/(?:^|\/)chronicle\/(\d{8})\//);
    return match ? `/app/transcripts/${encodeURIComponent(match[1])}` : null;
  }

  // A fact that does not apply to this import is dropped, not dashed: a clean
  // success owes the owner nothing about "failed at" or "unavailable pages",
  // and a page of dashes reads as a list of things that went wrong. `always` is
  // for the facts the status itself makes meaningful -- the counts an import
  // that ran should have, the time a finished import finished, the reason a
  // failed one failed. There the blank is the answer, so it is shown.
  function kvRow(label, value, options = {}) {
    const { always = false, wall = false } = options;
    const wallAttr = wall ? ' data-wall-time' : '';
    if (!hasValue(value)) {
      return always
        ? `<dt>${escapeHtml(label)}</dt><dd${wallAttr}>${escapeHtml(strings.unknown)}</dd>`
        : '';
    }
    return `<dt>${escapeHtml(label)}</dt><dd${wallAttr}>${escapeHtml(String(value))}</dd>`;
  }

  function sectionHtml(title, rows) {
    const kept = rows.filter(Boolean);
    if (!kept.length) return '';
    return `<section class="import-drawer-section"><h3>${escapeHtml(title)}</h3><dl class="drawer-kv">${kept.join('')}</dl></section>`;
  }

  function renderMeta(data) {
    const importJson = asObject(data?.import_json) || {};
    const importedJson = asObject(data?.imported_json) || {};
    const derived = deriveStatus(data);
    const fileName = hasValue(importJson.original_filename)
      ? String(importJson.original_filename)
      : (importedJson.source_type || data?.source_type) === 'generic'
        ? strings.file
        : (data?.original_filename ? String(data.original_filename) : strings.unknown);
    const fileSize = formatFileSize(importJson.file_size || data?.file_size);
    const uploadTime = formatDateTime(importJson.upload_datetime || data?.upload_datetime);
    const fileText = fileSize ? `${fileName} (${fileSize})` : fileName;
    const uploadHtml = uploadTime
      ? ` · <span data-wall-time>${escapeHtml(strings.uploaded_at)}: ${escapeHtml(uploadTime)}</span>`
      : '';

    return `<span>${escapeHtml(fileText)}</span>${uploadHtml} <span class="status-badge ${statusClass(derived.code)}">${escapeHtml(derived.status)}</span>`;
  }

  function knownSourceLabel(type) {
    const known = {
      document: 'journal.window.import.source.document.name',
      image: 'journal.window.import.source.image.name',
      text: 'journal.window.import.import.source.names.text.text',
    };
    const label = Object.hasOwn(known, type) ? known[type] : null;
    return label ? window.JournalI18n.render(label) : '';
  }

  // The page's own word for a source it knows comes first: an importer's
  // recorded name ("Image", "Text") was never a catalog string.
  function displaySource(data, importedJson) {
    return knownSourceLabel(data?.source_type) || knownSourceLabel(importedJson.source_type)
      || data?.source_display || importedJson.source_display
      || data?.source_type || importedJson.source_type;
  }

  function renderLeadsCard(data) {
    const importedJson = asObject(data?.imported_json) || {};
    const derived = deriveStatus(data);
    const day = resolveDay(data);
    const source = displaySource(data, importedJson);
    const facts = [
      derived.status,
      source,
      day ? window.JournalFormat.day(day) : '',
      formatCount(importedJson.entries_written ?? data?.entries_written, 'journal.window.import.detail.count.entries'),
      formatCount((importedJson.entities_seeded ?? data?.entities_seeded) ?? null, 'journal.window.import.detail.count.entities'),
      formatCount(importedJson.total_files_created ?? data?.total_files_created, 'journal.window.import.detail.count.files')
    ].filter(hasValue);
    const encodedDay = day ? encodeURIComponent(day) : '';
    const links = day
      ? `<div class="import-leads-links"><a href="#content">${window.JournalI18n.renderHtml('journal.window.import.link.label.view.imported.content')}</a><a href="/app/transcripts/${escapeHtml(encodedDay)}">${escapeHtml(strings.view_day)}</a></div>`
      : '';

    const untimed = data?.untimed_transcript === true
      ? `<p class="import-leads-notice">${escapeHtml(strings.untimed_notice)} <a href="${escapeHtml(UNTIMED_TRANSCRIPT_SUPPORT_URL)}" target="_blank" rel="noopener noreferrer">${escapeHtml(strings.untimed_ask)}</a>.</p>`
      : '';
    const importJson = asObject(data?.import_json) || {};
    const photoFallback = photoDateSource(importJson) === 'date_source_fallback'
      ? `<p class="import-leads-notice" data-photo-date-source="upload_fallback">${escapeHtml(strings.photo_fallback_notice)}</p>`
      : '';

    return `<section class="import-leads-card"><h2>${escapeHtml(strings.leads_title)}</h2><div class="import-leads-facts">${facts.map((fact) => `<span>${escapeHtml(fact)}</span>`).join('')}</div>${untimed}${photoFallback}${links}</section>`;
  }

  function photoDateSource(importJson) {
    if (String(importJson?.source || '').toLowerCase() !== 'image') return null;
    if (hasValue(importJson.owner_timestamp)) return 'date_source_owner';
    const method = importJson.timestamp_detection_method;
    if (method === 'deterministic') return 'date_source_photo';
    if (method === 'upload_fallback' || method === 'path_fallback') return 'date_source_fallback';
    return null;
  }

  function uploadFacts(importJson) {
    return sectionHtml(strings.upload_facts, [
      kvRow(strings.original_file, importJson.original_filename),
      kvRow(strings.file_size, formatFileSize(importJson.file_size)),
      kvRow(strings.mime_type, importJson.mime_type),
      kvRow(strings.uploaded_at, formatDateTime(importJson.upload_datetime), { wall: true }),
      kvRow(strings.detected_time, formatTimestamp(importJson.detected_timestamp)),
      kvRow(strings.import_time, formatTimestamp(importJson.user_timestamp)),
      kvRow(strings.date_source, strings[photoDateSource(importJson)]),
      kvRow(strings.setting, importJson.setting)
    ]);
  }

  function processingFacts(data) {
    const importedJson = asObject(data?.imported_json) || {};
    const derived = deriveStatus(data);
    const source = displaySource(data, importedJson);
    const unavailPages = typeof data?.unavailable_pages === 'number'
      ? (data.unavailable_pages > 0
          ? window.JournalI18n.render('journal.window.import.detail.preview.pages.unavailable', quantityArgs(data.unavailable_pages))
          : null)
      : formatUnavailablePages(data?.unavailable_pages);
    const unavailDesc = data?.unavailable_description
      ? strings.image_description_gap_detail
      : null;
    // An import that reached an end state should be able to say what it wrote,
    // even if the answer is "we don't know" -- so the counts carry a dash. The
    // failure keys belong to a failure and the completion time to a completion;
    // on any other status they are not unknown, they are not applicable.
    const finished = derived.code === 'success';
    const didFail = derived.code === 'failed';
    const countsApply = finished
      || derived.code === 'unconfirmed'
      || derived.code === 'unavailable';
    return sectionHtml(strings.processing_facts, [
      kvRow(strings.status, derived.status),
      kvRow(strings.source, source),
      kvRow(strings.target_day, importedJson.target_day || data?.target_day),
      kvRow(strings.date_range, formatDateRange(importedJson.date_range || data?.date_range)),
      kvRow(strings.entries, formatCount(importedJson.entries_written ?? data?.entries_written, 'journal.window.import.detail.count.entries'), { always: countsApply }),
      kvRow(strings.entities, formatCount((importedJson.entities_seeded ?? data?.entities_seeded) ?? null, 'journal.window.import.detail.count.entities'), { always: countsApply }),
      kvRow(strings.files, formatCount(importedJson.total_files_created ?? data?.total_files_created, 'journal.window.import.detail.count.files'), { always: countsApply }),
      kvRow(strings.unavailable_description, unavailDesc),
      kvRow(strings.unavailable_pages, unavailPages),
      data?.has_gaps ? kvRow(strings.has_gaps, window.JournalI18n.render('journal.window.import.answer.yes')) : '',
      finished ? kvRow(strings.completed_at, formatDateTime(data?.finished_at), { always: true, wall: true }) : '',
      didFail ? kvRow(strings.failed_at, formatDateTime(data?.finished_at), { always: true, wall: true }) : '',
      kvRow(strings.failed_stage, derived.body ? null : data?.error_stage, { always: didFail }),
      kvRow(strings.error, derived.body ? null : data?.error, { always: didFail })
    ]);
  }

  function renderMergeSummary(data) {
    const summary = asObject(data?.merge_summary);
    if (!summary) return '';
    const counter = (key, message) => {
      const value = numberValue(summary[key]);
      return value === null ? '' : window.JournalI18n.render(message, quantityArgs(value));
    };
    const row = (label, clauses) => {
      const kept = clauses.filter(Boolean);
      return kept.length ? kvRow(label, kept.join(' · ')) : '';
    };
    return sectionHtml(strings.merge_summary, [
      row(
        strings.segments,
        [
          counter('segments_copied', 'journal.window.import.detail.merge.copied'),
          counter('segments_skipped', 'journal.window.import.detail.merge.skipped'),
          counter('segments_errored', 'journal.window.import.detail.merge.errored')
        ]
      ),
      row(
        strings.entities,
        [
          counter('entities_created', 'journal.window.import.detail.merge.created'),
          counter('entities_merged', 'journal.window.import.detail.merge.merged'),
          counter('entities_staged', 'journal.window.import.detail.merge.staged')
        ]
      ),
      row(
        strings.facets,
        [
          counter('facets_created', 'journal.window.import.detail.merge.created'),
          counter('facets_merged', 'journal.window.import.detail.merge.merged'),
          numberValue(summary.entity_links_skipped)
            ? counter('entity_links_skipped', 'journal.window.import.detail.merge.links.left.out')
            : ''
        ]
      ),
      row(
        strings.imports,
        [
          counter('imports_copied', 'journal.window.import.detail.merge.copied'),
          counter('imports_skipped', 'journal.window.import.detail.merge.skipped')
        ]
      ),
      ...asArray(summary.body_zones_differ).map((differs) =>
        `<p class="import-zone-differs">${escapeHtml(zoneDiffersLine(differs))}</p>`
      ),
      bodyDaysHiddenLine(numberValue(summary.body_days_hidden))
    ]);
  }

  // How many days of body data the archive brought hidden; nothing when none.
  function bodyDaysHiddenLine(days) {
    if (!days) return '';
    const line = window.JournalI18n.render('journal.window.import.detail.merge.body.days.hidden', quantityArgs(days));
    return `<p class="import-zone-differs" data-copy-key="body_days_hidden">${escapeHtml(line)}</p>`;
  }

  function zoneDiffersLine(differs) {
    const hours = numberValue(differs?.hours);
    const source = differs?.source || '';
    return hours === 0
      ? window.JournalI18n.render('journal.window.import.detail.merge.clock.under.hour', [['source', String(source)]])
      : window.JournalI18n.render('journal.window.import.detail.merge.clock.hours', [...quantityArgs(hours), ['source', String(source)]]);
  }

  function renderCollisionCallout(data) {
    const principalCollision = asObject(data?.principal_collision);
    if (!principalCollision) return '';
    return `
      <div class="import-collision-callout">
        <h3>${escapeHtml(strings.collision_title)}</h3>
        <p>${window.JournalI18n.renderHtml(principalCollision.staged
          ? 'journal.window.import.detail.merge.owner.collision.staged'
          : 'journal.window.import.detail.merge.owner.collision', [
          ['journal_owner', String(principalCollision.target_name || '')], ['archive_owner', String(principalCollision.source_name || '')],
        ])}</p>
        <a href="/app/settings#profile">${escapeHtml(strings.profile_link)}</a>
      </div>
    `;
  }

  function evidenceSection(title, items) {
    const kept = items.filter(Boolean);
    if (!kept.length) return '';
    return `<section class="import-drawer-section"><h3>${escapeHtml(title)}</h3><ul class="drawer-evidence">${kept.join('')}</ul></section>`;
  }

  function renderMergeHighlights(data) {
    const stagedObj = asObject(data?.staged_entities);
    const stagedRows = asArray(stagedObj?.rows);
    const omittedCount = numberValue(stagedObj?.omitted) || 0;
    const sections = [];
    const staged = stagedRows.map((item) => {
      const source = item?.source_name || '';
      const path = item?.staging_path || '';
      return `<li class="drawer-evidence-row"><span class="drawer-evidence-title">${escapeHtml(source)}</span><span class="ev-meta"><code>${escapeHtml(path)}</code></span></li>`;
    });
    if (omittedCount > 0) {
      staged.push(`<li class="drawer-evidence-row drawer-evidence-omitted" data-omitted="${escapeHtml(String(omittedCount))}"><span class="drawer-evidence-title">${window.JournalI18n.renderHtml('journal.window.import.detail.merge.staged.omitted', quantityArgs(omittedCount))}</span></li>`);
    }
    const summaryErrors = asArray(data?.summary_errors).map((item) => (
      `<li class="drawer-evidence-row"><span class="drawer-evidence-title">${escapeHtml(item || '')}</span></li>`
    ));

    sections.push(evidenceSection(strings.staged_entities, staged));
    sections.push(evidenceSection(strings.summary_errors, summaryErrors));
    return sections.join('');
  }

  function renderArtifactPaths(data) {
    const paths = asObject(data?.merge_artifact_paths);
    if (!paths) return '';
    return sectionHtml(strings.artifact_paths, [
      kvRow(strings.decisions, paths.decisions),
      kvRow(strings.staging, paths.staging)
    ]);
  }

  function renderCreatedFiles(importedJson) {
    const files = asArray(importedJson?.all_created_files).map((path) => {
      const name = basename(path);
      const href = createdFileHref(path);
      const title = href
        ? `<a class="drawer-evidence-title" href="${escapeHtml(href)}">${escapeHtml(name)}</a>`
        : `<span class="drawer-evidence-title">${escapeHtml(name)}</span>`;
      return `<li class="drawer-evidence-row">${title}</li>`;
    });
    return evidenceSection(strings.created_files, files);
  }

  function renderProvenance(data, importedJson) {
    const processedAt = formatDateTime(data?.finished_at);
    const clauses = [];
    if (processedAt) clauses.push(window.JournalI18n.render('journal.window.import.detail.provenance.processed', [['finished_at', String(processedAt)]]));
    if (hasValue(importedJson?.source_type)) {
      clauses.push(window.JournalI18n.render('journal.window.import.detail.provenance.importer', [['source_type', String(importedJson.source_type)]]));
    }
    if (clauses.length === 0) return '';
    const wallAttr = processedAt ? ' data-wall-time' : '';
    return `<p class="drawer-provenance"${wallAttr}>${clauses.map(escapeHtml).join(' · ')}</p>`;
  }

  function isStrava(data) {
    return String(data?.source_type || '').toLowerCase() === 'strava';
  }

  // Kinds that don't delete as a whole here: a journal archive merge, which
  // isn't one thing. The server's `deletable` says so first.
  const DELETES_ELSEWHERE = new Set(['journal_archive', 'archive']);

  function importSourceType(data) {
    const importedJson = asObject(data?.imported_json) || {};
    const importJson = asObject(data?.import_json) || {};
    return String(data?.source_type || importedJson.source_type || importJson.source_type || '').toLowerCase();
  }

  // An import that has finished one way or another, of a kind that moves to
  // the trash whole. Nothing on the page says why one can't: it has no delete.
  function importDeletable(data) {
    if (typeof data?.deletable === 'boolean') return data.deletable;
    if (!hasValue(data?.timestamp) || !hasValue(data?.status)) return false;
    if (data.status === 'running' || data.status === 'pending') return false;
    const source = importSourceType(data);
    return Boolean(source) && !DELETES_ELSEWHERE.has(source);
  }

  function renderImportDelete(data) {
    if (!importDeletable(data)) return '';
    return `<section class="import-delete"><button type="button" class="import-secondary-btn" data-import-delete="${escapeHtml(data.timestamp)}" data-import-delete-source="${escapeHtml(importSourceType(data))}" data-import-delete-display="${escapeHtml(data?.source_display || '')}">${escapeHtml(strings.delete_import)}</button></section>`;
  }

  // The delete button is pressed through import's own mount, which opens its dialog.

  function rawBlock(data) {
    // A Strava import's publication record can list tens of thousands of pieces.
    if (isStrava(data)) return '';
    const importJson = data?.import_json ?? null;
    const importedJson = data?.imported_json ?? null;
    if (importJson === null && importedJson === null) return '';
    const payload = {
      import_json: importJson,
      imported_json: importedJson
    };
    return `<details class="drawer-raw"><summary>${escapeHtml(strings.raw_payload)}</summary><pre>${escapeHtml(JSON.stringify(payload, null, 2))}</pre></details>`;
  }

  function renderDrawerBody(data) {
    const importJson = asObject(data?.import_json) || {};
    const importedJson = asObject(data?.imported_json) || {};
    return [
      renderCollisionCallout(data),
      uploadFacts(importJson),
      processingFacts(data),
      renderMergeSummary(data),
      renderMergeHighlights(data),
      renderArtifactPaths(data),
      renderCreatedFiles(importedJson),
      renderProvenance(data, importedJson),
      rawBlock(data)
    ].join('');
  }

  const savedFlows = new Map();

  // Import's dispose: a flow keeps its markup and its page's refresh, so a
  // gone page's flows go with it, as a page load ended them.
  function forgetSavedStarts() {
    for (const flow of savedFlows.values()) {
      flow.controller?.abort?.();
      flow.controller = null;
      if (flow.escapeHandler && typeof document !== 'undefined' && typeof document.removeEventListener === 'function') {
        document.removeEventListener('keydown', flow.escapeHandler);
        flow.escapeHandler = null;
      }
    }
    savedFlows.clear();
  }

  function savedBinding(data) {
    const metadata = data?.import_json || {};
    return JSON.stringify([data?.timestamp, metadata.import_id, metadata.file_path,
      metadata.source_hash, metadata.file_size, metadata.folder_snapshot,
      metadata.source_timestamp, metadata.setting, metadata.body_facet, metadata.notebook]);
  }

  // A saved file has its size; a copied folder has none, but its own copied tree and snapshot.
  // An audio archive also needs its recording time zone, which only the workspace asks for.
  function savedSource(metadata, id) {
    if (Number.isSafeInteger(metadata.file_size) && metadata.file_size > 0) return true;
    return metadata.file_size === null && metadata.source_hint !== 'audio_archive'
      && typeof metadata.folder_snapshot === 'string'
      && /^[0-9a-f]{64}$/.test(metadata.folder_snapshot)
      && metadata.file_path.endsWith(`/imports/${id}/tree`);
  }

  function savedEligible(data) {
    const metadata = data?.import_json;
    return !savedFlows.get(data?.timestamp)?.revoked
      && data?.status === 'pending' && data?.saved_start?.state === 'eligible'
      && metadata?.import_id === data.timestamp && typeof metadata.file_path === 'string'
      && typeof metadata.source_timestamp === 'string' && !metadata.is_local_path
      && savedSource(metadata, data.timestamp);
  }

  function hasSavedFlow(data) {
    const flow = savedFlows.get(data?.timestamp);
    return savedEligible(data) || (flow && ['starting', 'unknown'].includes(flow.phase)
      && !['running', 'success', 'failed', 'unconfirmed', 'unavailable'].includes(data?.status));
  }

  function sourceHintOf(data) {
    const m = data?.import_json || {};
    return String(m.source_hint || m.source || data?.source_type || '').trim().toLowerCase();
  }

  function appleStatsFrom(held) {
    const stats = [];
    const pushCount = (val, key) => {
      if (Number.isSafeInteger(val) && val > 0) {
        stats.push({
          value: Number(val).toLocaleString(),
          label: window.JournalI18n.render(key, [['quantity', Math.trunc(val)]]).trim(),
        });
      }
    };
    pushCount(held.workouts, 'journal.window.import.preview.apple.workouts.label');
    pushCount(held.nights, 'journal.window.import.preview.apple.nights.label');
    pushCount(held.readings, 'journal.window.import.preview.apple.readings.label');
    if (stats.length < 3) pushCount(held.mindful_sessions, 'journal.window.import.preview.apple.mindful.label');
    if (stats.length < 3) pushCount(held.daily_totals, 'journal.window.import.preview.apple.daily.totals.label');
    if (stats.length < 3) pushCount(held.logged, 'journal.window.import.preview.apple.logged.label');
    if (Array.isArray(held.span) && held.span.length >= 2 && held.span[0] && held.span[1]) {
      const range = window.JournalI18n.render('journal.window.import.preview.date.range', [
        ['first_date', String(held.span[0])], ['last_date', String(held.span[1])],
      ]);
      const y1 = String(held.span[0]).slice(0, 4);
      const y2 = String(held.span[1]).slice(0, 4);
      stats.push({
        value: y1 && y2 && y1 !== y2 ? `${y1}–${y2}` : (y1 || range),
        label: range,
      });
    }
    return stats;
  }

  function previewSummary(replyOrSize) {
    const isEnvelope = replyOrSize && typeof replyOrSize === 'object' && 'size' in replyOrSize;
    const size = isEnvelope ? replyOrSize.size : replyOrSize;
    const space = isEnvelope ? replyOrSize.space : null;
    if (!size || typeof size !== 'object') throw new Error('invalid Preview');

    if (size.schema === 'solstone.import.evernote.preview.v1') {
      const ready = size.ready === true;
      const lines = Array.isArray(size.lines) ? size.lines.filter(Boolean).map(String) : [];
      return {
        kind: 'evernote',
        ready,
        summary: lines.length ? lines.join(' ') : (ready ? '' : strings.nothing_new),
        notebook: typeof size.notebook === 'string' ? size.notebook : '',
        needsBodyConfirm: Boolean(size.go_ahead),
        spaceFits: true,
        spaceLine: '',
        stats: [],
        extraLines: [],
      };
    }

    if (size.schema === 'solstone.import.audio-archive.size.v1') {
      if (!['members', 'source_bytes', 'days', 'tiles'].every(k => Number.isSafeInteger(size[k]) && size[k] >= 0)) {
        throw new Error('invalid Preview');
      }
      const ready = size.tiles > 0;
      const parts = [
        window.JournalI18n.render('journal.window.import.preview.archive.source.files', quantityArgs(size.members)),
        window.JournalI18n.render('journal.window.import.preview.archive.days', quantityArgs(size.days)),
      ];
      return {
        kind: 'audio_archive',
        ready,
        summary: ready ? parts.join(' · ') : strings.nothing_new,
        notebook: '',
        needsBodyConfirm: Boolean(size.go_ahead),
        spaceFits: true,
        spaceLine: '',
        stats: [],
        extraLines: [],
      };
    }

    if (!Number.isSafeInteger(size.recordings) || size.recordings < 0
        || !Number.isSafeInteger(size.days) || size.days < 0) throw new Error('invalid Preview');
    const other = size.other_units;
    if (other !== null && other !== undefined
        && (!Array.isArray(other) || typeof other[0] !== 'string'
          || !Number.isSafeInteger(other[1]) || other[1] < 0)) throw new Error('invalid Preview');

    if (size.apple_health && typeof size.apple_health === 'object') {
      const held = size.apple_health;
      const tally = (message, value) => window.JournalI18n.render(message,
        quantityArgs(value, 'count', 'quantity', Number(value).toLocaleString()));
      const parts = [];
      if (held.span) {
        parts.push(window.JournalI18n.render('journal.window.import.preview.date.range', [
          ['first_date', String(held.span[0])], ['last_date', String(held.span[1])],
        ]));
      }
      if (held.workouts) parts.push(tally('journal.window.import.preview.apple.workouts', held.workouts));
      if (held.nights) parts.push(tally('journal.window.import.preview.apple.nights', held.nights));
      if (held.mindful_sessions) parts.push(tally('journal.window.import.preview.apple.mindful', held.mindful_sessions));
      if (held.readings) parts.push(tally('journal.window.import.preview.apple.readings', held.readings));
      if (held.daily_totals) parts.push(tally('journal.window.import.preview.apple.daily.totals', held.daily_totals));
      if (held.logged) parts.push(tally('journal.window.import.preview.apple.logged', held.logged));
      const extraLines = [];
      const left = Object.keys(held.not_read || {}).length;
      if (left) extraLines.push(tally('journal.window.import.preview.apple.left.out', left));
      if (size.listed_moves?.count) {
        const moved = size.listed_moves.count;
        const moveText = window.JournalI18n.render('journal.window.import.preview.apple.move.listed', quantityArgs(moved, 'count', 'quantity', Number(moved).toLocaleString()));
        extraLines.push(moveText);
      }
      const spaceFits = !space || space.fits !== false;
      let spaceLine = '';
      if (space) {
        spaceLine = spaceFits
          ? window.JournalI18n.render('journal.window.import.ox.detail.space.available', [
              ['need', String(space.need_words || '')],
              ['free', String(space.free_words || '')],
            ])
          : strings.space_insufficient;
      }
      const hasItems = Boolean(other?.[1]) || Boolean(held.workouts) || Boolean(held.nights)
        || Boolean(held.readings) || Boolean(held.daily_totals) || Boolean(held.logged) || Boolean(held.mindful_sessions);
      return {
        kind: 'apple_health',
        ready: hasItems && spaceFits,
        summary: hasItems ? parts.join(' · ') : strings.nothing_new,
        notebook: '',
        needsBodyConfirm: true,
        spaceFits,
        spaceLine,
        stats: appleStatsFrom(held),
        extraLines,
      };
    }

    const ready = size.recordings > 0 || (other && other[1] > 0) || Boolean(size.listed_moves?.count);
    const recordings = window.JournalI18n.render('journal.window.import.detail.preview.recordings', [
      ...quantityArgs(size.recordings, 'recordings', 'recording_quantity'), ...quantityArgs(size.days, 'days', 'day_quantity'),
    ]);
    const parts = [recordings];
    if (other) {
      parts.push(window.JournalI18n.render('journal.window.import.detail.preview.other.units', [
        ['count', String(other[1])], ['unit', String(other[0])],
      ]));
    }
    const extraLines = [];
    if (size.listed_moves?.count && Array.isArray(size.listed_moves.from)) {
      extraLines.push(window.JournalI18n.render('journal.window.import.preview.workouts.move.listed', [
        ...quantityArgs(size.listed_moves.count), ['sources', String(size.listed_moves.from.join(', '))],
      ]));
    }
    return {
      kind: 'size',
      ready: Boolean(ready),
      summary: ready ? parts.join(' · ') : strings.nothing_new,
      notebook: '',
      needsBodyConfirm: Boolean(size.go_ahead),
      spaceFits: true,
      spaceLine: '',
      stats: [],
      extraLines,
    };
  }

  function isDefiniteStartRefusal(err) {
    if (!err || typeof err !== 'object') return false;
    const status = err.status;
    const code = err.reasonCode || err.payload?.reason_code || '';
    const kind = err.payload?.kind || '';
    if (status === 400) return true;
    if (status === 409 && (code === 'body_space' || code === 'import_preview_required' || kind === 'source_changed')) {
      return true;
    }
    return false;
  }

  function errorTextFrom(err, fallback) {
    return (err && (err.serverMessage || err.payload?.error || (err.status ? err.message : ''))) || fallback;
  }

  function mountSavedStart(root, data, refresh, restoreFocus = false, options = null) {
    const slot = root.querySelector('[data-saved-start-slot]');
    const id = data.timestamp;
    const originals = options && typeof options.render === 'function' ? options : (options?.originals || null);
    const bodyFacetApi = options?.bodyFacet || null;
    const onDiscard = typeof options?.onDiscard === 'function' ? options.onDiscard : null;
    const autoPreview = options?.autoPreview !== false;

    let flow = savedFlows.get(id);
    const binding = savedBinding(data);
    if (!flow) {
      if (!slot) return;
      flow = {
        id, binding, phase: 'idle', token: null, preview: null, summary: '', error: '',
        revision: 0, revoked: false, inFlight: false, autoAttempted: false,
        controller: null, discardOpen: false, discardInFlight: false, discardError: '',
        escapeHandler: null, facetState: null, facetLoaded: false, facetPending: false,
        settingDraft: String(data?.import_json?.setting || ''),
        notebookDraft: String(data?.import_json?.notebook || ''),
        metaRevision: 0,
        lastPreviewHtml: '', lastOptionsHtml: '', lastDiscardHtml: '',
      };
      savedFlows.set(id, flow);
    } else if (flow.binding !== binding || (!savedEligible(data) && ['idle', 'ready', 'previewing', 'failed'].includes(flow.phase))) {
      flow.revision += 1;
      flow.controller?.abort?.();
      flow.controller = null;
      flow.binding = binding;
      flow.token = null;
      flow.preview = null;
      flow.summary = '';
      flow.autoAttempted = false;
      flow.settingDraft = String(data?.import_json?.setting || '');
      flow.notebookDraft = String(data?.import_json?.notebook || '');
      if (!['starting', 'unknown'].includes(flow.phase)) flow.phase = 'idle';
    }
    if (!savedEligible(data)) {
      flow.revoked = true;
      flow.controller?.abort?.();
      flow.controller = null;
      flow.token = null;
      flow.preview = null;
      flow.summary = '';
      flow.phase = 'unknown';
    }
    flow.data = data;
    if (!slot) { flow.revision += 1; return; }
    flow.root = root;
    flow.slot = slot;
    flow.refresh = refresh;

    const hint = sourceHintOf(data);
    const isBodySource = hint === 'apple_health' || hint === 'strava';
    const isEvernote = hint === 'evernote';

    function current() {
      return flow.root.isConnected !== false && flow.slot.isConnected !== false
        && savedFlows.get(id) === flow;
    }

    function querySlot(sel) {
      const found = flow.slot.querySelector(sel);
      return found && found !== flow.slot.querySelector('[data-saved-start-action]') ? found : null;
    }

    function facetBlocking() {
      return Boolean(isBodySource && flow.facetState?.offer && String(flow.facetState.offer.state).startsWith('ask_'));
    }

    function renderFoundMarkup() {
      const p = flow.preview;
      if (!p) return '';
      let html = `<h3 class="import-review-heading">${escapeHtml(strings.found_heading)}</h3>`;
      if (p.stats && p.stats.length) {
        html += '<div class="import-preview-grid">'
          + p.stats.map(s => `<div class="import-preview-stat"><b>${escapeHtml(s.value)}</b><span>${escapeHtml(s.label)}</span></div>`).join('')
          + '</div>';
      } else if (p.summary) {
        html += `<p class="import-preview-summary">${escapeHtml(p.summary)}</p>`;
      }
      if (p.extraLines && p.extraLines.length) {
        html += p.extraLines.map(line => `<p class="import-preview-extra">${escapeHtml(line)}</p>`).join('');
      }
      if (p.spaceLine) {
        html += p.spaceFits
          ? `<p class="import-preview-space">${escapeHtml(p.spaceLine)}</p>`
          : `<div class="import-preview-space-warn" role="alert">${escapeHtml(p.spaceLine)}</div>`;
      }
      return html;
    }

    function renderOptionsBox() {
      const box = querySlot('[data-saved-start-options]');
      if (!box || typeof box.querySelector !== 'function') return;
      if (flow.phase === 'unknown' || !savedEligible(flow.data)) {
        if (flow.lastOptionsHtml) {
          box.innerHTML = '';
          flow.lastOptionsHtml = '';
        }
        return;
      }

      // Build facet / notebook / context controls without clobbering focused inputs.
      const focusInside = typeof document !== 'undefined' && box.contains && box.contains(document.activeElement);
      if (focusInside) return;

      const pieces = [];
      if (isBodySource && flow.facetState?.offer) {
        const offer = flow.facetState.offer;
        const copy = flow.facetState.copy || {};
        const facetTitle = name => ((flow.facetState.facets || []).find(f => f.name === name)?.title || name || '');
        if (String(offer.state).startsWith('ask_')) {
          const q = offer.state === 'ask_existing'
            ? (typeof window.JournalI18n.render === 'function'
                ? window.JournalI18n.render(`journal.window.import.body.facet.${hint === 'apple_health' ? 'apple' : 'shared'}.ask.named`, [['facet_name', facetTitle(offer.facet)]])
                : String(copy[offer.state] || ''))
            : String(copy[offer.state] || '');
          pieces.push(
            `<div class="import-option-group" data-review-facet-ask>`
            + `<p class="import-option-question">${escapeHtml(q)}</p>`
            + (copy.no_means ? `<p class="import-field-hint">${escapeHtml(copy.no_means)}</p>` : '')
            + `<div class="import-action-row">`
            + `<button type="button" class="import-secondary-btn" data-review-facet-answer="yes">${escapeHtml(copy.yes || 'yes')}</button>`
            + `<button type="button" class="import-secondary-btn" data-review-facet-answer="no">${escapeHtml(copy.no || 'no')}</button>`
            + `</div></div>`
          );
        } else {
          const metaFacet = flow.data?.import_json?.body_facet;
          const currentChoice = metaFacet?.choice === 'none'
            ? ''
            : (metaFacet?.choice === 'facet' ? String(metaFacet.facet || '') : (offer.state === 'chosen' ? String(offer.facet || '') : ''));
          const opts = (flow.facetState.facets || []).map(item =>
            `<option value="${escapeHtml(item.name)}"${item.name === currentChoice ? ' selected' : ''}>${escapeHtml(item.title)}</option>`
          );
          opts.push(`<option value=""${currentChoice ? '' : ' selected'}>${escapeHtml(copy.none_option || strings.no_facet)}</option>`);
          pieces.push(
            `<div class="import-option-group">`
            + `<label for="detailFacetSelect">${escapeHtml(strings.facet_label)}</label>`
            + `<select id="detailFacetSelect" data-review-facet-select>${opts.join('')}</select>`
            + `</div>`
          );
        }
      }

      if (isEvernote) {
        const nbVal = flow.notebookDraft || flow.preview?.notebook || '';
        pieces.push(
          `<div class="import-option-group">`
          + `<label for="detailNotebookInput">${escapeHtml(strings.notebook_label)}</label>`
          + `<input id="detailNotebookInput" type="text" autocomplete="off" data-review-notebook value="${escapeHtml(nbVal)}">`
          + `</div>`
        );
      }

      if (!isBodySource && !isEvernote) {
        pieces.push(
          `<div class="import-option-group">`
          + `<label for="detailContextInput">${escapeHtml(strings.context_label)}</label>`
          + `<input id="detailContextInput" type="text" autocomplete="off" data-review-context value="${escapeHtml(flow.settingDraft)}" placeholder="${escapeHtml(strings.context_hint)}">`
          + `</div>`
        );
      }

      if (isBodySource) {
        pieces.push(`<p class="import-expiry-note">${escapeHtml(strings.expiry_note)}</p>`);
      }

      const nextHtml = pieces.join('');
      if (flow.lastOptionsHtml !== nextHtml) {
        flow.lastOptionsHtml = nextHtml;
        box.innerHTML = nextHtml;
        box.querySelectorAll?.('[data-review-facet-answer]')?.forEach(btn => {
          btn.onclick = () => answerFacet(btn.getAttribute('data-review-facet-answer'));
        });
        const sel = box.querySelector?.('[data-review-facet-select]');
        if (sel) {
          sel.onchange = () => updateMetaAndRepreview({
            body_facet: sel.value ? { choice: 'facet', facet: sel.value } : { choice: 'none' },
          });
        }
        const nb = box.querySelector?.('[data-review-notebook]');
        if (nb) {
          nb.oninput = () => { flow.notebookDraft = nb.value; };
          nb.onchange = () => { flow.notebookDraft = nb.value; };
        }
        const ctxIn = box.querySelector?.('[data-review-context]');
        if (ctxIn) {
          ctxIn.oninput = () => { flow.settingDraft = ctxIn.value; };
          ctxIn.onchange = () => {
            const trimmed = ctxIn.value.trim();
            flow.settingDraft = trimmed;
            if (trimmed !== String(flow.data?.import_json?.setting || '')) {
              return updateMetaAndRepreview({ setting: trimmed });
            }
          };
        }
      }
    }

    async function loadFacetIfNeeded() {
      if (!isBodySource || !bodyFacetApi || flow.facetLoaded || flow.facetPending) return;
      flow.facetPending = true;
      try {
        const res = await bodyFacetApi.load(hint);
        if (!current()) return;
        flow.facetState = res && res.offer ? res : null;
        flow.facetLoaded = true;
      } catch (_) {
        flow.facetLoaded = true;
      } finally {
        flow.facetPending = false;
        draw();
      }
    }

    async function answerFacet(answer) {
      if (!bodyFacetApi || !flow.facetState?.offer || flow.facetPending) return;
      flow.facetPending = true;
      flow.error = '';
      draw();
      try {
        const res = await bodyFacetApi.answer(answer, flow.facetState.offer, hint);
        if (!current()) return;
        if (res?.offer) {
          flow.facetState = res;
          if (!String(res.offer.state).startsWith('ask_')) {
            const chosen = res.offer.state === 'chosen'
              ? { choice: 'facet', facet: res.offer.facet }
              : { choice: 'none' };
            await updateMetaAndRepreview({ body_facet: chosen });
          }
        }
      } catch (err) {
        if (!current()) return;
        flow.error = errorTextFrom(err, strings.preview_failed);
        try {
          const fresh = await bodyFacetApi.load(hint);
          if (current() && fresh?.offer) flow.facetState = fresh;
        } catch (_) { /* ignore */ }
      } finally {
        flow.facetPending = false;
        draw();
      }
    }

    async function updateMetaAndRepreview(patch) {
      if (!savedEligible(flow.data) || !current()) return;
      const metadata = flow.data.import_json;
      const metaRev = ++flow.metaRevision;
      flow.controller?.abort?.();
      flow.controller = null;
      flow.token = null;
      flow.error = '';
      const metaBody = {
        import_id: id,
        path: metadata.file_path,
        ...patch,
      };
      try {
        await window.apiJson('/app/import/api/meta', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(metaBody),
        });
        if (!current() || metaRev !== flow.metaRevision) return;
        if ('setting' in patch) metadata.setting = patch.setting;
        if ('body_facet' in patch) metadata.body_facet = patch.body_facet;
        if ('notebook' in patch) metadata.notebook = patch.notebook;
        flow.binding = savedBinding(flow.data);
        flow.phase = 'idle';
        await runPreview();
      } catch (err) {
        if (!current() || metaRev !== flow.metaRevision) return;
        flow.error = errorTextFrom(err, strings.preview_failed);
        flow.phase = 'failed';
        draw();
      }
    }

    function renderDiscardBox() {
      const box = querySlot('[data-saved-start-discard]');
      if (!box) return;
      if (!flow.discardOpen) {
        if (flow.lastDiscardHtml) {
          box.innerHTML = '';
          flow.lastDiscardHtml = '';
        }
        if (flow.escapeHandler && typeof document !== 'undefined' && typeof document.removeEventListener === 'function') {
          document.removeEventListener('keydown', flow.escapeHandler);
          flow.escapeHandler = null;
        }
        return;
      }
      const nextDiscardHtml = `<div class="import-discard-confirm" role="dialog" aria-modal="false" aria-labelledby="discardConfirmTitle">`
        + `<h4 id="discardConfirmTitle">${escapeHtml(strings.discard_title)}</h4>`
        + `<p>${escapeHtml(strings.discard_body)}</p>`
        + (flow.discardError ? `<p role="alert" class="import-discard-error">${escapeHtml(flow.discardError)}</p>` : '')
        + `<div class="import-action-row">`
        + `<button type="button" class="import-secondary-btn" data-discard-cancel>${escapeHtml(strings.discard_cancel)}</button>`
        + `<button type="button" class="import-danger-btn" data-discard-confirm${flow.discardInFlight ? ' aria-disabled="true"' : ''}>${escapeHtml(strings.discard_confirm)}</button>`
        + `</div></div>`;
      if (flow.lastDiscardHtml === nextDiscardHtml) return;
      flow.lastDiscardHtml = nextDiscardHtml;
      box.innerHTML = nextDiscardHtml;
      const cancelBtn = box.querySelector?.('[data-discard-cancel]');
      const confirmBtn = box.querySelector?.('[data-discard-confirm]');
      const closeConfirm = () => {
        if (flow.discardInFlight) return;
        flow.discardOpen = false;
        flow.discardError = '';
        draw();
        const trigger = querySlot('[data-saved-start-discard-trigger]');
        trigger?.focus?.();
      };
      if (cancelBtn) cancelBtn.onclick = closeConfirm;
      if (confirmBtn) confirmBtn.onclick = () => confirmDiscard();
      if (!flow.escapeHandler && typeof document !== 'undefined' && typeof document.addEventListener === 'function') {
        flow.escapeHandler = event => {
          if (event.key === 'Escape' && flow.discardOpen) {
            event.preventDefault();
            closeConfirm();
          }
        };
        document.addEventListener('keydown', flow.escapeHandler);
      }
      if (!flow.discardInFlight && cancelBtn?.focus) cancelBtn.focus();
    }

    async function confirmDiscard() {
      if (!current() || flow.discardInFlight) return;
      flow.discardInFlight = true;
      flow.discardError = '';
      flow.controller?.abort?.();
      flow.controller = null;
      renderDiscardBox();
      try {
        await window.apiJson(`/app/import/api/${encodeURIComponent(id)}/discard`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: '{}',
        });
        if (!current()) return;
        flow.discardOpen = false;
        flow.revoked = true;
        savedFlows.delete(id);
        if (onDiscard) onDiscard(id);
        else await flow.refresh();
      } catch (err) {
        if (!current()) return;
        if (err?.status === 404) {
          flow.discardOpen = false;
          flow.revoked = true;
          savedFlows.delete(id);
          if (onDiscard) onDiscard(id);
          else await flow.refresh();
          return;
        }
        if (err?.status === 409) {
          flow.discardOpen = false;
          flow.discardError = '';
          flow.revoked = true;
          await flow.refresh();
          return;
        }
        flow.discardError = errorTextFrom(err, strings.discard_failed);
      } finally {
        flow.discardInFlight = false;
        if (current() && flow.discardOpen) renderDiscardBox();
      }
    }

    function draw() {
      if (!current()) return;
      const focused = restoreFocus || flow.slot.contains(document.activeElement);
      restoreFocus = false;
      const busy = flow.inFlight || ['previewing', 'starting', 'checking'].includes(flow.phase);
      const unknown = flow.phase === 'unknown';
      if (!savedEligible(flow.data) && !unknown && !busy) {
        flow.slot.innerHTML = '';
        return;
      }
      const fileName = flow.data?.import_json?.original_filename || flow.data?.original_filename || strings.file;
      const checkingText = window.JournalI18n.render('journal.window.import.ox.detail.checking', [['name', String(fileName)]]);
      const blocked = !unknown && !busy && (
        facetBlocking()
        || (flow.phase === 'ready' && flow.preview && (!flow.preview.ready || !flow.preview.spaceFits))
      );
      const label = unknown
        ? strings.check_status
        : flow.phase === 'previewing' || (flow.inFlight && flow.phase === 'idle')
          ? strings.preparing_preview
          : flow.phase === 'starting'
            ? strings.starting
            : flow.phase === 'checking'
              ? strings.checking_status
              : flow.phase === 'ready'
                ? strings.start_import
                : flow.phase === 'failed' || (flow.preview && flow.preview.spaceFits === false)
                  ? strings.try_preview_again
                  : strings.preview_import;

      if (!flow.slot.querySelector('[data-saved-start-status]')) {
        flow.lastPreviewHtml = '';
        flow.lastOptionsHtml = '';
        flow.lastDiscardHtml = '';
        flow.slot.innerHTML = '<section class="import-saved-start">'
          + '<p role="status" aria-live="polite" data-saved-start-status></p>'
          + '<div data-saved-start-preview></div>'
          + '<div data-saved-start-options></div>'
          + '<div data-saved-start-originals></div>'
          + '<div class="import-action-row" data-saved-start-actions></div>'
          + '<div data-saved-start-discard></div>'
          + '<div role="alert" data-saved-start-error></div></section>';
        const source = flow.data.import_json?.source_hint || flow.data.import_json?.source;
        originals?.render?.(flow.slot.querySelector('[data-saved-start-originals]'), source);
      }

      // The primary button is drawn once and kept, so its focus stays on the same node.
      let button = flow.slot.querySelector('[data-saved-start-action]');
      if (!button) {
        const actionsEl = flow.slot.querySelector('[data-saved-start-actions]');
        actionsEl.innerHTML = '<button type="button" class="import-primary-btn" data-saved-start-action></button>'
          + '<button type="button" class="import-secondary-btn import-discard-trigger" data-saved-start-discard-trigger></button>';
        button = flow.slot.querySelector('[data-saved-start-action]');
      }
      writeText(button, label);
      writeAttr(button, 'aria-disabled', String(busy || blocked));
      button.onclick = () => act(button);

      const discardBtn = querySlot('[data-saved-start-discard-trigger]');
      if (discardBtn) {
        writeText(discardBtn, strings.discard_button);
        writeHidden(discardBtn, unknown || !savedEligible(flow.data));
        discardBtn.onclick = () => {
          if (flow.inFlight && flow.phase === 'starting') return;
          flow.discardOpen = true;
          flow.discardError = '';
          renderDiscardBox();
        };
      }

      const statusText = flow.phase === 'previewing'
        ? checkingText
        : busy
          ? label
          : (flow.preview && flow.preview.stats && flow.preview.stats.length ? '' : flow.summary);
      writeText(flow.slot.querySelector('[data-saved-start-status]'), statusText);
      const previewEl = querySlot('[data-saved-start-preview]');
      if (previewEl) {
        const nextPreview = flow.phase === 'ready' || (flow.preview && !busy) ? renderFoundMarkup() : '';
        if (flow.lastPreviewHtml !== nextPreview) {
          flow.lastPreviewHtml = nextPreview;
          previewEl.innerHTML = nextPreview;
        }
      }
      renderOptionsBox();
      renderDiscardBox();
      writeText(flow.slot.querySelector('[data-saved-start-error]'), flow.error);
      if (focused && document.activeElement !== button) button.focus();
    }

    async function runPreview() {
      if (!current() || flow.inFlight || !savedEligible(flow.data)) return;
      const metadata = flow.data.import_json;
      const payload = {
        import_id: id,
        path: metadata.file_path,
        timestamp: metadata.source_timestamp,
        setting: flow.settingDraft || metadata.setting || '',
      };
      const revision = flow.revision;
      flow.controller?.abort?.();
      flow.controller = typeof AbortController === 'function' ? new AbortController() : null;
      const signal = flow.controller ? flow.controller.signal : undefined;
      flow.inFlight = true;
      flow.phase = 'previewing';
      flow.error = '';
      draw();
      try {
        const reply = await window.apiJson('/app/import/api/preview', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload),
          ...(signal ? { signal } : {}),
        });
        if (revision !== flow.revision || !current()) return;
        const preview = previewSummary(reply);
        if (typeof reply.preview_token !== 'string' || !reply.preview_token) throw new Error('invalid Preview');
        flow.preview = preview;
        flow.summary = preview.summary;
        if (isEvernote && preview.notebook && !flow.notebookDraft) flow.notebookDraft = preview.notebook;
        flow.token = preview.ready ? reply.preview_token : null;
        flow.phase = preview.ready ? 'ready' : 'idle';
      } catch (err) {
        if (revision !== flow.revision || !current()) return;
        if (err?.name === 'AbortError') return;
        flow.token = null;
        flow.preview = null;
        flow.phase = 'failed';
        flow.error = errorTextFrom(err, strings.preview_failed);
      } finally {
        flow.inFlight = false;
        draw();
      }
    }

    async function act(button) {
      if (!current() || flow.slot.querySelector('[data-saved-start-action]') !== button
          || flow.inFlight || ['previewing', 'starting', 'checking'].includes(flow.phase)) return;
      if (flow.phase === 'unknown') {
        flow.phase = 'checking';
        draw();
        flow.error = '';
        try { await flow.refresh(); }
        catch (_) { flow.error = strings.status_failed; }
        finally { flow.phase = 'unknown'; draw(); }
        return;
      }
      if (!savedEligible(flow.data)) return;
      if (originals?.pending?.(flow.slot.querySelector('[data-saved-start-originals]'))) return;
      if (facetBlocking()) return;
      const starting = flow.phase === 'ready' && Boolean(flow.token) && Boolean(flow.preview?.ready) && flow.preview?.spaceFits !== false;
      if (!starting) {
        await runPreview();
        return;
      }
      const metadata = flow.data.import_json;
      // If Evernote notebook changed, persist it via /meta right before /start.
      if (isEvernote && flow.notebookDraft !== String(metadata.notebook || '')) {
        try {
          await window.apiJson('/app/import/api/meta', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ import_id: id, path: metadata.file_path, notebook: flow.notebookDraft }),
          });
          metadata.notebook = flow.notebookDraft;
          flow.binding = savedBinding(flow.data);
        } catch (err) {
          if (!current()) return;
          flow.error = errorTextFrom(err, strings.start_unconfirmed);
          draw();
          return;
        }
      }
      // If context note changed without blur/change firing yet, save & re-preview first so /start never hits 409 SourceChanged.
      if (!isBodySource && !isEvernote && flow.settingDraft.trim() !== String(metadata.setting || '')) {
        await updateMetaAndRepreview({ setting: flow.settingDraft.trim() });
        return;
      }
      const payload = {
        import_id: id,
        path: metadata.file_path,
        timestamp: metadata.source_timestamp,
        setting: metadata.setting || '',
        preview_token: flow.token,
        first_start_only: true,
      };
      if (flow.preview?.needsBodyConfirm) {
        payload.body_confirmed = flow.token;
      }
      const revision = flow.revision;
      flow.inFlight = true;
      flow.phase = 'starting';
      flow.error = '';
      draw();
      try {
        const reply = await window.apiJson('/app/import/api/start', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(payload),
        });
        if (revision !== flow.revision || !current()) return;
        flow.token = null;
        flow.phase = 'unknown';
        if (reply.import_id !== id) flow.error = strings.start_unconfirmed;
        await flow.refresh();
      } catch (err) {
        if (revision !== flow.revision || !current()) return;
        flow.token = null;
        if (isDefiniteStartRefusal(err)) {
          flow.phase = 'failed';
          flow.error = errorTextFrom(err, strings.start_unconfirmed);
        } else {
          flow.phase = 'unknown';
          flow.error = strings.start_unconfirmed;
        }
      } finally {
        flow.inFlight = false;
        if (flow.phase === 'starting') flow.phase = 'unknown';
        draw();
      }
    }

    draw();
    if (isBodySource && bodyFacetApi && !flow.facetLoaded) {
      loadFacetIfNeeded();
    }
    if (autoPreview && savedEligible(data) && flow.phase === 'idle' && !flow.autoAttempted && !flow.inFlight) {
      flow.autoAttempted = true;
      runPreview();
    }
  }

  function renderRunningProgress(data, liveStage = null) {
    const derived = deriveStatus(data);
    if (derived.code !== 'running') return '';
    const thinking = asObject(data?.thinking);
    let headline = strings.processing;
    let detail = '';
    let pct = null;
    if (thinking && thinking.status === 'body' && thinking.live === true) {
      const body = asObject(thinking.body) || {};
      const phaseKeys = {
        scan: 'journal.window.import.body.phase.words.scan.reading.your.export',
        place: 'journal.window.import.body.phase.words.place.sorting.what.it.read',
        repair: 'journal.window.import.body.phase.words.repair.checking.what.an.earlier.import.left',
        write: 'journal.window.import.body.phase.words.write.saving',
        retry: 'journal.window.import.body.phase.words.retry.trying.again.what.didn.t.save',
      };
      if (body.phase && phaseKeys[body.phase]) {
        headline = window.JournalI18n.render(phaseKeys[body.phase]);
      }
      if (Number.isSafeInteger(body.days_total) && body.days_total > 0) {
        const done = Number.isSafeInteger(body.days_done) ? body.days_done : 0;
        detail = window.JournalI18n.render('journal.window.import.progress.body.days', [
          ['days_done', String(done)], ['days_total', String(body.days_total)],
        ]);
        pct = Math.max(4, Math.min(100, Math.round((done / body.days_total) * 100)));
      }
    } else if (liveStage) {
      if (liveStage.headline) headline = String(liveStage.headline);
      if (liveStage.detail) detail = String(liveStage.detail);
      if (typeof liveStage.pct === 'number' && Number.isFinite(liveStage.pct)) {
        pct = Math.max(4, Math.min(100, Math.round(liveStage.pct)));
      }
    }
    const bar = pct !== null
      ? `<div class="import-running-bar" role="progressbar" aria-valuenow="${pct}" aria-valuemin="0" aria-valuemax="100"><div class="import-running-fill" style="width:${pct}%"></div></div>`
      : '';
    return `<section class="import-running-card" role="status" aria-live="polite" data-running-progress-card>`
      + `<p class="import-running-headline">${escapeHtml(headline)}</p>`
      + (detail ? `<p class="import-running-detail">${escapeHtml(detail)}</p>` : '')
      + bar
      + `</section>`;
  }

  function renderExpiredRecovery() {
    return {
      metaHtml: `<span class="status-badge expired">${escapeHtml(strings.expired_badge)}</span>`,
      bodyHtml: `<section class="import-expired-card">`
        + `<p class="import-expired-lead">${escapeHtml(strings.expired_body)}</p>`
        + `<p class="import-expired-recovery">${escapeHtml(strings.expired_recovery)}</p>`
        + `<div class="import-action-row"><a class="import-primary-btn" href="/app/import/">${escapeHtml(strings.back_to_imports)}</a></div>`
        + `</section>`,
    };
  }

  function renderDetail(data, liveStage = null) {
    if (!window.Drawer || typeof window.Drawer.render !== 'function') {
      throw new Error(strings.drawer_unavailable);
    }
    const derived = deriveStatus(data);
    const drawer = window.Drawer.render({
      id: 'import-detail-bookkeeping',
      label: strings.drawer_label,
      line: composeDrawerLine(data),
      chipText: derived.chipText,
      chipTone: derived.chipTone,
      open: derived.open,
      bodyHtml: renderDrawerBody(data)
    });
    const leads = derived.code === 'success' ? renderLeadsCard(data) : '';
    const running = `<div class="import-running-slot" data-running-progress-slot>${derived.code === 'running' ? renderRunningProgress(data, liveStage) : ''}</div>`;
    const continuation = hasSavedFlow(data) ? '<div data-saved-start-slot></div>' : '';
    return continuation + running + leads + drawer + renderImportDelete(data);
  }

  window.ImportDetail = Object.freeze({
    renderDetail,
    renderRunningProgress,
    renderExpiredRecovery,
    mountSavedStart,
    forgetSavedStarts,
    renderMeta,
    deriveStatus,
    previewSummary,
    formatDuration,
    composeDrawerLine,
    resolveDay,
    createdFileHref,
    hasValue,
    kvRow
  });
})();
