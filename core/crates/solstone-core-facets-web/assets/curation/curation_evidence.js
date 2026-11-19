// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function (global) {
  'use strict';

  // --- owner-facing strings ---
  const DRAWER_LABEL = window.JournalI18n.render('journal.window.facets.curation.evidence.drawer.title');
  const PIECE_SINGULAR = window.JournalI18n.render('journal.window.facets.curation.evidence.piece.one');
  const PIECE_PLURAL = window.JournalI18n.render('journal.window.facets.curation.evidence.piece.many');
  const COUNT_SHOWING = window.JournalI18n.render('journal.window.facets.curation.evidence.part.showing');
  const COUNT_OF = window.JournalI18n.render('journal.window.facets.curation.evidence.part.of');
  const META_SEPARATOR = ' · ';
  // --- end owner-facing strings ---

  const escapeHtml = global.AppServices.escapeHtml;

  function samplesFor(item) {
    const samples = item?.evidence?.samples;
    return Array.isArray(samples) ? samples : [];
  }

  function totalCountFor(item, sampleCount) {
    const count = Math.trunc(Number(item?.evidence?.count));
    return Number.isFinite(count) && count > sampleCount ? count : sampleCount;
  }

  function formatEvidenceLine(displayedCount, totalCount) {
    // G2-B08: a bare "3 of 29" beside the word "evidence" reads as an id or a
    // rank. Say what the two numbers are.
    if (displayedCount < totalCount) {
      return `${COUNT_SHOWING} ${displayedCount} ${COUNT_OF} ${totalCount}`;
    }
    if (totalCount === 1) {
      return `1 ${PIECE_SINGULAR}`;
    }
    return `${totalCount} ${PIECE_PLURAL}`;
  }

  function sampleFields(sample) {
    return {
      day: String(sample.day ?? '').trim(),
      stream: String(sample.stream ?? '').trim(),
      segment: String(sample.segment ?? '').trim(),
    };
  }

  function plainRow(fields) {
    const day = fields.day ? escapeHtml(global.JournalFormat.day(fields.day)) : '';
    const stream = fields.stream
      ? `<span title="${escapeHtml(fields.stream)}">${escapeHtml(global.JournalFormat.stream(fields.stream))}</span>`
      : '';
    const segment = fields.segment ? escapeHtml(global.JournalFormat.segmentTime(fields.segment)) : '';
    const inner = [day, stream, segment].filter(Boolean).join(META_SEPARATOR);
    return '<li class="drawer-evidence-row">' +
      `<span class="drawer-evidence-title">${inner}</span>` +
      '</li>';
  }

  function linkedRow(fields) {
    const dayHref = `/app/transcripts/${fields.day}`;
    const segmentHref = `/app/transcripts/${fields.day}#${fields.segment}`;
    const streamHtml = fields.stream
      ? `<span class="ev-meta" title="${escapeHtml(fields.stream)}">${escapeHtml(global.JournalFormat.stream(fields.stream))}</span>`
      : '';
    return '<li class="drawer-evidence-row">' +
      `<a class="drawer-evidence-title" href="${escapeHtml(dayHref)}">${escapeHtml(global.JournalFormat.day(fields.day))}</a>` +
      streamHtml +
      `<a class="ev-meta" href="${escapeHtml(segmentHref)}">${escapeHtml(global.JournalFormat.segmentTime(fields.segment))}</a>` +
      '</li>';
  }

  function renderSample(sample) {
    const fields = sampleFields(sample);
    if (!fields.day || !fields.segment) return plainRow(fields);
    return linkedRow(fields);
  }

  function buildEvidenceDrawerProps(item) {
    const samples = samplesFor(item);
    const sampleCount = samples.length;
    if (sampleCount === 0) return null;
    const totalCount = totalCountFor(item, sampleCount);
    const bodyHtml = `<ul class="drawer-evidence">${samples.map(renderSample).join('')}</ul>`;
    return {
      id: `curation-evidence:${String(item?.kind ?? '')}:${String(item?.key ?? '')}`,
      open: false,
      label: DRAWER_LABEL,
      line: formatEvidenceLine(sampleCount, totalCount),
      bodyHtml,
    };
  }

  const CurationEvidence = {
    buildEvidenceDrawerProps,
    formatEvidenceLine,
  };
  global.CurationEvidence = CurationEvidence;
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = CurationEvidence;
  }
})(typeof window !== 'undefined' ? window : globalThis);
