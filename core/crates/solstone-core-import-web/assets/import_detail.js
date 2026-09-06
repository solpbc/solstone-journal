// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  'use strict';

  // --- owner-facing strings ---
  const strings = Object.freeze({
    unknown: 'unknown',
    drawer_label: 'bookkeeping',
    leads_title: 'where this landed',
    activities: 'activities',
    view_day: 'view that day',
    upload_facts: 'upload facts',
    processing_facts: 'processing facts',
    merge_summary: 'merge summary',
    merge_highlights: 'merge highlights',
    artifact_paths: 'artifact paths',
    created_files: 'created files',
    original_file: 'original file',
    file_size: 'file size',
    mime_type: 'mime type',
    uploaded_at: 'uploaded at',
    detected_time: 'detected time',
    import_time: 'import time',
    facets: 'facets',
    setting: 'setting',
    status: 'status',
    source: 'source',
    target_day: 'target day',
    date_range: 'date range',
    entry: 'entry',
    entries: 'entries',
    entity: 'entity',
    entities: 'entities',
    file: 'file',
    files: 'files',
    completed_at: 'completed at',
    failed_at: 'failed at',
    failed_stage: 'failed stage',
    error: 'error',
    segments: 'segments',
    imports: 'imports',
    decisions: 'decisions',
    staging: 'staging',
    staged_entities: 'staged entities',
    errored_segments: 'errored segments',
    summary_errors: 'summary errors',
    raw_payload: 'raw payload',
    drawer_unavailable: 'drawer renderer unavailable',
    completed: 'completed',
    failed: 'failed',
    pending: 'pending',
    running: 'running',
    processing: 'processing…',
    failed_line: 'failed while processing',
    completed_in: 'completed in',
    files_created: 'files created',
    owner_identity_differs: 'owner identity differs',
    identity_differs: 'identity differs',
    under_a_minute: 'under a minute',
    minute: 'minute',
    minutes: 'minutes',
    hour: 'hour',
    hours: 'hours',
    day: 'day',
    days: 'days',
    copied: 'copied',
    skipped: 'skipped',
    errored: 'errored',
    created: 'created',
    merged: 'merged',
    staged: 'staged',
    importer: 'importer',
    processed: 'processed',
    nothing_left: 'nothing left this machine',
    profile_link: 'review profile settings',
    collision_title: 'owner identity differs between journals',
    collision_body_before_target: 'this journal belongs to ',
    collision_body_between_names: ', and the imported journal marks ',
    collision_body_after_source: ' as its owner. this ',
    collision_body_journal_entity: 'journal&#39;s',
    collision_body_after_entity: ' owner is unchanged; the other person came in as a regular entity.',
    file_size_units: Object.freeze(['b', 'kb', 'mb', 'gb'])
  });
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

  function plural(count, singular, pluralValue) {
    return Number(count) === 1 ? singular : pluralValue;
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

  function formatCount(value, singular, pluralValue) {
    const count = numberValue(value);
    if (count === null) return null;
    return `${count} ${plural(count, singular, pluralValue)}`;
  }

  function formatDuration(startIso, endIso) {
    if (!hasValue(startIso) || !hasValue(endIso)) return null;
    const start = new Date(String(startIso));
    const end = new Date(String(endIso));
    const startMs = start.getTime();
    const endMs = end.getTime();
    if (!Number.isFinite(startMs) || !Number.isFinite(endMs) || endMs < startMs) {
      return null;
    }

    const seconds = (endMs - startMs) / 1000;
    if (seconds < 60) return strings.under_a_minute;
    if (seconds < 90 * 60) {
      const minutes = Math.max(1, Math.round(seconds / 60));
      return `${minutes} ${plural(minutes, strings.minute, strings.minutes)}`;
    }
    if (seconds < 36 * 60 * 60) {
      const hours = Math.max(1, Math.round(seconds / 3600));
      return `${hours} ${plural(hours, strings.hour, strings.hours)}`;
    }
    const days = Math.max(1, Math.round(seconds / 86400));
    return `${days} ${plural(days, strings.day, strings.days)}`;
  }

  function statusClass(status) {
    if (status === strings.completed) return 'success';
    if (status === strings.failed) return 'failed';
    if (status === strings.running) return 'running';
    return 'pending';
  }

  function deriveStatus(data) {
    const importedJson = asObject(data?.imported_json);
    const principalCollision = asObject(importedJson?.principal_collision);
    const canonicalStatus = data?.status;

    if (canonicalStatus === 'failed') {
      return {
        status: strings.failed,
        chipText: strings.failed,
        chipTone: 'danger',
        open: true
      };
    }
    if (canonicalStatus === 'success') {
      if (principalCollision) {
        return {
          status: strings.completed,
          chipText: strings.identity_differs,
          chipTone: 'warn',
          open: true
        };
      }
      return { status: strings.completed, chipText: '', chipTone: '', open: false };
    }
    if (canonicalStatus === 'running') {
      return { status: strings.running, chipText: '', chipTone: '', open: false };
    }
    return { status: strings.pending, chipText: '', chipTone: '', open: false };
  }

  function composeDrawerLine(data) {
    const importJson = asObject(data?.import_json) || {};
    const importedJson = asObject(data?.imported_json);
    const derived = deriveStatus(data);

    if (derived.status === strings.failed) return strings.failed_line;
    if (derived.status === strings.running || derived.status === strings.pending) {
      return strings.processing;
    }

    const clauses = [];
    const files = numberValue(importedJson?.total_files_created);
    if (files !== null) {
      clauses.push(`${files} ${strings.files_created}`);
    }
    const entries = numberValue(importedJson?.entries_written);
    if (entries !== null) {
      clauses.push(`${entries} ${plural(entries, strings.entry, strings.entries)}`);
    }
    const duration = formatDuration(
      importJson.upload_datetime,
      importedJson?.processing_completed
    );
    if (duration) {
      clauses.push(`${strings.completed_in} ${duration}`);
    }
    if (!clauses.length) {
      clauses.push(strings.completed);
    }
    if (asObject(importedJson?.principal_collision)) {
      clauses.push(strings.owner_identity_differs);
    }
    return clauses.join(' · ');
  }

  function resolveDay(data) {
    const importedJson = asObject(data?.imported_json) || {};
    const range = importedJson.date_range || null;
    const day = importedJson.target_day || range?.[0] || null;
    return hasValue(day) ? String(day) : null;
  }

  function createdFileHref(path) {
    const match = String(path ?? '').match(/(?:^|\/)chronicle\/(\d{8})\//);
    return match ? `/app/transcripts/${encodeURIComponent(match[1])}` : null;
  }

  function kvRow(label, value) {
    if (!hasValue(value)) return '';
    return `<dt>${escapeHtml(label)}</dt><dd>${escapeHtml(value)}</dd>`;
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
      : importedJson.source_type === 'generic'
        ? strings.file
        : strings.unknown;
    const fileSize = formatFileSize(importJson.file_size);
    const uploadTime = formatDateTime(importJson.upload_datetime);
    const fileText = fileSize ? `${fileName} (${fileSize})` : fileName;
    const uploadHtml = uploadTime
      ? ` · <span>${escapeHtml(strings.uploaded_at)}: ${escapeHtml(uploadTime)}</span>`
      : '';

    return `<span>${escapeHtml(fileText)}</span>${uploadHtml} <span class="status-badge ${statusClass(derived.status)}">${escapeHtml(derived.status)}</span>`;
  }

  function renderLeadsCard(data) {
    const importedJson = asObject(data?.imported_json) || {};
    const derived = deriveStatus(data);
    const day = resolveDay(data);
    const source = importedJson.source_display || importedJson.source_type;
    const facts = [
      derived.status,
      source,
      day ? window.JournalFormat.day(day) : '',
      formatCount(importedJson.entries_written, strings.entry, strings.entries),
      formatCount(importedJson.entities_seeded, strings.entity, strings.entities),
      formatCount(importedJson.total_files_created, strings.file, strings.files)
    ].filter(hasValue);
    const encodedDay = day ? encodeURIComponent(day) : '';
    const links = day
      ? `<div class="import-leads-links"><a href="#content">view imported content</a><a href="/app/transcripts/${escapeHtml(encodedDay)}">${escapeHtml(strings.view_day)}</a></div>`
      : '';

    return `<section class="import-leads-card"><h2>${escapeHtml(strings.leads_title)}</h2><div class="import-leads-facts">${facts.map((fact) => `<span>${escapeHtml(fact)}</span>`).join('')}</div>${links}</section>`;
  }

  function uploadFacts(importJson) {
    return sectionHtml(strings.upload_facts, [
      kvRow(strings.original_file, importJson.original_filename),
      kvRow(strings.file_size, formatFileSize(importJson.file_size)),
      kvRow(strings.mime_type, importJson.mime_type),
      kvRow(strings.uploaded_at, formatDateTime(importJson.upload_datetime)),
      kvRow(strings.detected_time, formatTimestamp(importJson.detected_timestamp)),
      kvRow(strings.import_time, formatTimestamp(importJson.user_timestamp)),
      kvRow(strings.setting, importJson.setting)
    ]);
  }

  function processingFacts(data) {
    const importedJson = asObject(data?.imported_json) || {};
    const derived = deriveStatus(data);
    const source = importedJson.source_display || importedJson.source_type;
    return sectionHtml(strings.processing_facts, [
      kvRow(strings.status, derived.status),
      kvRow(strings.source, source),
      kvRow(strings.target_day, importedJson.target_day),
      kvRow(strings.date_range, formatDateRange(importedJson.date_range)),
      kvRow(strings.entries, formatCount(importedJson.entries_written, strings.entry, strings.entries)),
      kvRow(strings.entities, formatCount(importedJson.entities_seeded, strings.entity, strings.entities)),
      kvRow(strings.files, formatCount(importedJson.total_files_created, strings.file, strings.files)),
      kvRow(strings.completed_at, formatDateTime(importedJson.processing_completed)),
      kvRow(strings.failed_at, formatDateTime(importedJson.processing_failed)),
      kvRow(strings.failed_stage, data?.error_stage),
      kvRow(strings.error, data?.error)
    ]);
  }

  function renderMergeSummary(importedJson) {
    const summary = asObject(importedJson?.merge_summary);
    if (!summary) return '';
    const counter = (key, label) => {
      const value = numberValue(summary[key]);
      return value === null ? '' : `${value} ${label}`;
    };
    const row = (label, clauses) => {
      const kept = clauses.filter(Boolean);
      return kept.length ? kvRow(label, kept.join(' · ')) : '';
    };
    return sectionHtml(strings.merge_summary, [
      row(
        strings.segments,
        [
          counter('segments_copied', strings.copied),
          counter('segments_skipped', strings.skipped),
          counter('segments_errored', strings.errored)
        ]
      ),
      row(
        strings.entities,
        [
          counter('entities_created', strings.created),
          counter('entities_merged', strings.merged),
          counter('entities_staged', strings.staged)
        ]
      ),
      row(
        strings.facets,
        [
          counter('facets_created', strings.created),
          counter('facets_merged', strings.merged)
        ]
      ),
      row(
        strings.imports,
        [
          counter('imports_copied', strings.copied),
          counter('imports_skipped', strings.skipped)
        ]
      )
    ]);
  }

  function renderCollisionCallout(importedJson) {
    if (!asObject(importedJson?.principal_collision)) return '';
    const principalCollision = importedJson.principal_collision;
    return `
      <div class="import-collision-callout">
        <h3>${escapeHtml(strings.collision_title)}</h3>
        <p>${escapeHtml(strings.collision_body_before_target)}${escapeHtml(principalCollision.target_name || '')}${escapeHtml(strings.collision_body_between_names)}${escapeHtml(principalCollision.source_name || '')}${escapeHtml(strings.collision_body_after_source)}${strings.collision_body_journal_entity}${escapeHtml(strings.collision_body_after_entity)}</p>
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
    const highlights = asObject(data?.decision_highlights) || {};
    const sections = [];
    const staged = asArray(highlights.staged_entities).map((item) => {
      const source = item?.source_name || '';
      const target = item?.target_name || '';
      const path = item?.staging_path || '';
      return `<li class="drawer-evidence-row"><span class="drawer-evidence-title">${escapeHtml(source)} ${escapeHtml('->')} ${escapeHtml(target)}</span><span class="ev-meta"><code>${escapeHtml(path)}</code></span></li>`;
    });
    const errored = asArray(highlights.errored_segments).map((item) => (
      `<li class="drawer-evidence-row"><span class="drawer-evidence-title">${escapeHtml(item?.item_id || '')}</span><span class="ev-meta">${escapeHtml(item?.reason || '')}</span></li>`
    ));
    const summaryErrors = asArray(data?.summary_errors).map((item) => (
      `<li class="drawer-evidence-row"><span class="drawer-evidence-title">${escapeHtml(item || '')}</span></li>`
    ));

    sections.push(evidenceSection(strings.staged_entities, staged));
    sections.push(evidenceSection(strings.errored_segments, errored));
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

  function renderProvenance(importedJson) {
    const processedAt = formatDateTime(
      importedJson?.processing_completed || importedJson?.processing_failed
    );
    const clauses = [];
    if (processedAt) clauses.push(`${strings.processed} ${processedAt}`);
    if (hasValue(importedJson?.source_type)) {
      clauses.push(`${importedJson.source_type} ${strings.importer}`);
    }
    clauses.push(strings.nothing_left);
    return `<p class="drawer-provenance">${clauses.map(escapeHtml).join(' · ')}</p>`;
  }

  function rawBlock(data) {
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
      renderCollisionCallout(importedJson),
      uploadFacts(importJson),
      processingFacts(data),
      renderMergeSummary(importedJson),
      renderMergeHighlights(data),
      renderArtifactPaths(data),
      renderCreatedFiles(importedJson),
      renderProvenance(importedJson),
      rawBlock(data)
    ].join('');
  }

  function renderDetail(data) {
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
    return renderLeadsCard(data) + drawer;
  }

  window.ImportDetail = Object.freeze({
    renderDetail,
    renderMeta,
    deriveStatus,
    formatDuration,
    composeDrawerLine,
    resolveDay,
    createdFileHref,
    hasValue,
    kvRow
  });
})();
