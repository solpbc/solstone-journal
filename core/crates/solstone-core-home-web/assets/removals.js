// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  // The originals page is part of home. Home's mount hands it its context,
  // and the list goes with home.
  let reportHomeReady = function () {};
  let originalsCtx = null;

  const LIST_URL = '/app/home/api/removals';
  const APPROVE_URL = '/app/home/api/approve';
  const DECLINE_URL = '/app/home/api/decline';
  const RECOVER_URL = '/app/home/api/recover';
  // Originals your retention settings marked wait here, and are recovered or
  // erased sooner here.
  const TRASH_URL = '/app/settings#trash';
  const MAX_SELECTED_MARKS = 32;
  const POLL_INTERVAL_MS = 2000;
  let retired = false;
  let mountEpoch = 0;
  let activeAction = null;

  let card = null;
  let mountedRoot = null;
  let rows = [];
  let listState = '';
  let reviewContext = null;
  let refreshGeneration = 0;
  let renderGeneration = 0;
  let confirmation = null;
  let outcomeHtml = '';
  let selected = new Set();
  let pageOffset = 0;
  let nextOffset = null;
  let totalRows = 0;
  let previousOffsets = [];
  let expanded = false;
  // The days a moved original waits in the trash, from the list; null when it
  // didn't say, and the dialog uses its forms without a number.
  let trashDays = null;

  function escapeHtml(value) {
    return String(value ?? '').replace(/[&<>"']/g, (char) => ({
      '&': '&amp;',
      '<': '&lt;',
      '>': '&gt;',
      '"': '&quot;',
      "'": '&#39;'
    })[char]);
  }

  const { renderHtml, cardinalQuantity } = window.JournalI18n;

  function quantity(count) {
    return ['quantity', cardinalQuantity(count)];
  }

  function originalCount(count) {
    return ['original_count', String(count)];
  }

  function request(url, options) {
    // Every read of the list asks for the list as it is now.
    if (originalsCtx) return originalsCtx.read(url, Object.assign({ fresh: true }, options));
    if (typeof window.apiJson !== 'function') return Promise.reject(new Error());
    return window.apiJson(url, options);
  }

  function formatBytes(bytes) {
    const value = Number(bytes) || 0;
    if (value < 1024) return value + ' B';
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
    let amount = value;
    let unit = 'KiB';
    for (let index = 1; index < units.length; index += 1) {
      if (amount < 1024) break;
      amount /= 1024;
      unit = units[index];
    }
    return amount >= 1024 && unit === 'TiB'
      ? (amount / 1024).toFixed(1) + ' PiB'
      : amount.toFixed(1) + ' ' + unit;
  }

  // The day the trash erases what moved there, as the trash page's receipts
  // write it (`tomorrow`, `tue nov 17`): the journal's day. Empty when it can't
  // be read, and the sentence leaves the date out.
  function eraseDate(eraseAt) {
    const instant = new Date(eraseAt);
    if (!eraseAt || Number.isNaN(instant.getTime()) || !window.JournalClock) return '';
    return window.JournalFormat.day(window.JournalClock.dayKey(instant)).toLowerCase();
  }

  function streamLabel(stream, { sourceEnglish = false } = {}) {
    if (stream === '_default') return null;
    return typeof stream === 'string' ? window.JournalFormat.stream(stream, { sourceEnglish }) : null;
  }

  // The list is a deletion decision, so every row carries an anchored date
  // rather than the relative label the rest of the app uses, and the time of
  // the material it covers so two rows from one day and one stream are told
  // apart before anything is deleted. G1-30. The clock is the page's own:
  // seconds are noise on a decision made by the day, and a second convention
  // made the list read like a log file. G1-202.
  function identityLabel(row, { sourceEnglish = false } = {}) {
    const date = window.JournalFormat.dayFull(row.day, { sourceEnglish });
    const time = window.JournalFormat.segmentTimeOfDay(row.dir, { omitMissing: true, sourceEnglish });
    return time === null ? date : date + ' ' + time;
  }

  function identityText(row) {
    const stream = streamLabel(row.stream);
    if (stream === null) return escapeHtml(identityLabel(row));
    return renderHtml('journal.window.home.originals.row.identity.stream', [['date_and_time', identityLabel(row)], ['stream_name', stream]]);
  }

  // The label now names the source the way the owner would say it, so the
  // exact stream key rides along in the title where a deletion decision can
  // still be checked against it. G1-203.
  function identity(text, row) {
    const key = row && typeof row.stream === 'string' && row.stream !== '_default' ? row.stream : null;
    const title = key ? ' title="' + escapeHtml(key) + '"' : '';
    return '<p class="removals-card-identity" data-removal-identity' + title + '>' + text + '</p>';
  }

  function rowCount(row) {
    return Number(row.count) || 0;
  }

  // selection-wide originals count; for a one-row selection it equals today's rowCount(row); for a bulk selection it is never one selected row's count
  function selectionOriginals(rows) {
    return rows.reduce(function (total, row) {
      return total + rowCount(row);
    }, 0);
  }

  function origin(row) {
    return row.origin === 'offload' ? 'offload' : 'policy';
  }

  // Every row used to restate what marked it and the "nothing else goes with
  // it" clause, so four rows said the same two sentences four times. Those two
  // clauses are shared, not per-row: listNoteHtml() states them once above the
  // list. The row keeps only what actually differs between rows, which is the
  // date and stream, the count and size, and its own two actions.
  function selectionLabel(id, checked) {
    const checkbox = document.createElement('input');
    checkbox.type = 'checkbox';
    checkbox.setAttribute('data-removal-select', '');
    checkbox.setAttribute('data-mark-id', id);
    if (checked) checkbox.setAttribute('checked', '');
    const label = document.createElement('span');
    label.append(...window.JournalI18n.renderMarkup('journal.window.home.originals.selection.label', [], {
      fixed: { checkbox }
    }));
    return label.innerHTML;
  }

  function markedRow(row) {
    const count = rowCount(row);
    const checked = selected.has(row.id);
    return '<article class="removals-card-row" data-removal-row data-mark-id="' + escapeHtml(row.id) + '">'
      + identity(identityText(row), row)
      + '<label class="removals-select">' + selectionLabel(row.id, checked) + '</label>'
      + '<p class="removals-card-what">' + renderHtml('journal.window.home.originals.row.contents.count', [quantity(count), originalCount(count), ['size', String(row.size ?? '')]]) + '</p>'
      + '<button type="button" data-removal-action="approve" data-mark-id="' + escapeHtml(row.id) + '">'
      + renderHtml('journal.window.home.originals.copy.row.delete')
      + '</button>'
      + '<button type="button" data-removal-action="decline" data-mark-id="' + escapeHtml(row.id) + '">'
      + renderHtml('journal.window.home.originals.copy.row.keep')
      + '</button>'
      + '</article>';
  }

  function failedRow(row) {
    return '<article class="removals-card-row" data-removal-row data-mark-id="' + escapeHtml(row.id) + '">'
      + identity(identityText(row), row)
      + '<p class="removals-card-failed-badge">' + renderHtml('journal.window.home.originals.copy.failed.badge') + '</p>'
      + '<p class="removals-card-failed-body">'
      + renderHtml('journal.window.home.originals.copy.failed.body')
      + '</p>'
      + '</article>';
  }

  function markedRows() {
    return rows.filter(function (row) {
      return row.state === 'marked';
    });
  }

  function selectedRows() {
    return markedRows().filter(function (row) {
      return selected.has(row.id);
    });
  }

  function cardRows() {
    return rows.map(function (row) {
      return row.state === 'failed' ? failedRow(row) : markedRow(row);
    }).join('');
  }

  function total() {
    return markedRows().reduce(function (value, row) {
      value.count += rowCount(row);
      value.bytes += Number(row.bytes) || 0;
      return value;
    }, { count: 0, bytes: 0 });
  }

  // The clauses every marked row shares, stated once above the list: one
  // sentence per origin actually present, then the promise that deleting an
  // original takes nothing else with it. Each origin sentence carries its own
  // count, because on a mixed list a bare "this" or "these" leaves the reader
  // no way to tell which rows the sentence covers. Counted over every marked
  // row on this page, agreeing with its totals and selection scope.
  function listNoteHtml() {
    const marked = markedRows();
    if (marked.length === 0) return '';
    const present = { policy: false, offload: false };
    const originals = { policy: 0, offload: 0 };
    marked.forEach(function (row) {
      const key = origin(row);
      present[key] = true;
      originals[key] += rowCount(row);
    });
    const notes = [];
    if (present.policy) notes.push(renderHtml('journal.window.home.originals.row.marked.by.retention', [quantity(originals.policy), originalCount(originals.policy)]) + ' ' + renderHtml('journal.window.home.originals.row.marked.by.retention.detail'));
    if (present.offload) notes.push(renderHtml('journal.window.home.originals.row.copied.by.backup', [quantity(originals.offload), originalCount(originals.offload)]) + ' ' + renderHtml('journal.window.home.originals.row.copied.by.backup.detail', [quantity(originals.offload)]));
    notes.push(renderHtml('journal.window.home.originals.row.nothing.else.goes', [quantity(originals.policy + originals.offload)]));
    return '<p class="removals-card-origin" data-removals-note>' + notes.join(' ') + '</p>';
  }

  function toolbarHtml() {
    if (markedRows().length === 0) return '';
    const n = selected.size;
    const disabled = n === 0 ? ' disabled' : '';
    // The standing sentence used to promise a 32-item cap beside a 20-row
    // page, leaving the owner to reconcile two numbers on a screen whose
    // actions are irreversible. The cap is stated where it can act: on the
    // live count, at the moment a selection passes it. G1-204.
    const count = n === 0
      ? ''
      : '<span>' + (n > MAX_SELECTED_MARKS
        ? renderHtml('journal.window.home.originals.bulk.selection.over.cap', [['selected_count', String(n)], ['selection_limit', String(MAX_SELECTED_MARKS)]])
        : renderHtml('journal.window.home.originals.bulk.selection.count', [quantity(n), ['selected_count', String(n)]])) + '</span>';
    return '<div class="removals-card-toolbar">'
      + '<button type="button" data-removal-action="select-all">' + renderHtml('journal.window.home.originals.copy.bulk.select.all') + '</button>'
      + '<button type="button" data-removal-action="clear-selection">' + renderHtml('journal.window.home.originals.copy.bulk.clear') + '</button>'
      + count
      + '<button type="button" data-removal-action="delete-selected"' + disabled + '>' + renderHtml('journal.window.home.originals.copy.bulk.delete') + '</button>'
      + '<button type="button" data-removal-action="keep-selected"' + disabled + '>' + renderHtml('journal.window.home.originals.copy.bulk.keep') + '</button>'
      + '</div>';
  }

  function finishHtml() {
    if (!rows.some(function (row) { return row.state === 'failed'; })) return '';
    return '<p class="removals-card-finish"><button type="button" data-removal-action="finish">'
      + renderHtml('journal.window.home.originals.copy.failed.finish')
      + '</button></p>';
  }

  function confirmationHtml() {
    if (!confirmation) return '';
    if (confirmation.kind === 'recover') {
      return '<section class="removals-card-confirm" role="dialog">'
        + '<h3>' + renderHtml('journal.window.home.originals.copy.confirm.recover.heading') + '</h3>'
        + '<p>' + renderHtml('journal.window.home.originals.copy.confirm.recover.body') + '</p>'
        + '<button type="button" data-removal-action="confirm-finish">' + renderHtml('journal.window.home.originals.copy.confirm.recover.go') + '</button>'
        + '<button type="button" data-removal-action="cancel">' + renderHtml('journal.window.home.originals.copy.confirm.cancel') + '</button>'
        + '</section>';
    }
    const confirmRows = confirmation.rows;
    if (confirmRows.length === 1) return confirmSingleRow(confirmRows[0]);
    return confirmSelectedRows(confirmRows);
  }

  function retentionDaysArgs() {
    return [
      ['days_quantity', cardinalQuantity(trashDays)],
      ['retention_days', String(trashDays)]
    ];
  }

  function confirmSingleRow(row) {
    const count = rowCount(row);
    const numbered = [quantity(count)];
    const source = {};
    const scope = [quantity(count), originalCount(count), ['date_and_time', identityLabel(row, source)]];
    const stream = streamLabel(row.stream, source);
    if (stream !== null) scope.push(['stream_name', stream]);
    const policy = origin(row) === 'policy';
    const heading = renderHtml('journal.window.home.originals.confirm.delete.heading', numbered);
    const nothingElse = renderHtml('journal.window.home.originals.confirm.delete.named.stream.third', numbered);
    if (policy) {
      const move = stream === null
        ? 'journal.window.home.originals.confirm.trash.until.erase.default.stream.first'
        : 'journal.window.home.originals.confirm.trash.until.erase.named.stream.first';
      const erase = trashDays === null
        ? renderHtml('journal.window.home.originals.confirm.trash.named.stream.third', numbered)
        : renderHtml('journal.window.home.originals.confirm.trash.until.erase.named.stream.third',
          [...numbered, ...retentionDaysArgs()]);
      const sentences = [
        renderHtml(move, scope), nothingElse, erase,
        renderHtml('journal.window.home.originals.confirm.trash.until.erase.named.stream.fourth', numbered)
      ];
      return confirmSection(heading, [sentences.join(' ')],
        renderHtml('journal.window.home.originals.copy.confirm.go.trash'), true);
    }
    const remove = stream === null
      ? 'journal.window.home.originals.confirm.delete.default.stream.first'
      : 'journal.window.home.originals.confirm.delete.named.stream.first';
    const sentences = [
      renderHtml(remove, scope),
      renderHtml('journal.window.home.originals.confirm.delete.named.stream.second'),
      nothingElse,
      renderHtml('journal.window.home.originals.confirm.delete.named.stream.fourth')
    ];
    return confirmSection(heading, [sentences.join(' ')],
      renderHtml('journal.window.home.originals.confirm.delete.action', numbered), false);
  }

  function confirmSelectedRows(confirmRows) {
    const byOrigin = { policy: 0, offload: 0 };
    confirmRows.forEach(row => { byOrigin[origin(row)] += rowCount(row); });
    const n = byOrigin.policy + byOrigin.offload;
    const selectedRows = [quantity(confirmRows.length)];
    const heading = renderHtml('journal.window.home.originals.confirm.delete.heading', selectedRows);
    if (byOrigin.offload === 0) {
      const erase = trashDays === null
        ? renderHtml('journal.window.home.originals.confirm.bulk.trash.third')
        : renderHtml('journal.window.home.originals.confirm.bulk.trash.until.erase.third', retentionDaysArgs());
      const sentences = [
        renderHtml('journal.window.home.originals.confirm.bulk.trash.until.erase.first', [originalCount(n)]),
        renderHtml('journal.window.home.originals.confirm.bulk.delete.third'),
        erase,
        renderHtml('journal.window.home.originals.confirm.bulk.trash.until.erase.fourth')
      ];
      return confirmSection(heading, [sentences.join(' ')],
        renderHtml('journal.window.home.originals.copy.confirm.go.trash'), true);
    }
    const unavailable = renderHtml('journal.window.home.originals.confirm.delete.named.stream.second');
    const go = renderHtml('journal.window.home.originals.confirm.delete.action', selectedRows);
    if (byOrigin.policy === 0) {
      const sentences = [
        renderHtml('journal.window.home.originals.confirm.bulk.delete.first', [originalCount(n)]),
        unavailable,
        renderHtml('journal.window.home.originals.confirm.bulk.delete.third'),
        renderHtml('journal.window.home.originals.confirm.delete.named.stream.fourth')
      ];
      return confirmSection(heading, [sentences.join(' ')], go, false);
    }
    const offload = [
      renderHtml('journal.window.home.originals.confirm.mixed.delete.first', [quantity(byOrigin.offload)]),
      unavailable,
      renderHtml('journal.window.home.originals.confirm.mixed.delete.third', [quantity(byOrigin.offload)])
    ];
    const erase = trashDays === null
      ? renderHtml('journal.window.home.originals.confirm.mixed.trash.first', [quantity(byOrigin.policy)])
      : renderHtml('journal.window.home.originals.confirm.mixed.trash.until.erase.first',
        [quantity(byOrigin.policy), ...retentionDaysArgs()]);
    const policy = [
      erase,
      renderHtml('journal.window.home.originals.confirm.trash.until.erase.named.stream.fourth', [quantity(byOrigin.policy)])
    ];
    return confirmSection(heading, [
      renderHtml('journal.window.home.originals.confirm.mixed.summary', [
        ['deleted_count', String(byOrigin.offload)], ['trashed_count', String(byOrigin.policy)]
      ]),
      offload.join(' '), policy.join(' ')
    ], go, false);
  }

  // A move to the trash is reversible, so its dialog isn't the warning box and
  // its button isn't danger.
  function confirmSection(heading, paragraphs, go, toTrash) {
    return '<section class="removals-card-confirm' + (toTrash ? ' removals-card-confirm--trash' : '') + '" role="dialog">'
      + '<h3>' + heading + '</h3>'
      + paragraphs.map(function (text) { return '<p>' + text + '</p>'; }).join('')
      + '<button type="button" data-removal-action="confirm"' + (toTrash ? ' class="removals-go-trash"' : '') + '>' + go + '</button>'
      + '<button type="button" data-removal-action="cancel">' + renderHtml('journal.window.home.originals.copy.confirm.cancel') + '</button>'
      + '</section>';
  }

  function pagesHtml() {
    return `<nav class="removals-card-pages" aria-label="${window.JournalI18n.renderHtml('journal.window.home.accessible.name.originals.pages')}"><button type="button" data-removal-action="previous"`
      + (previousOffsets.length === 0 ? ' disabled' : '') + `>${window.JournalI18n.renderHtml('journal.window.home.visible.text.previous')}</button><span>`
      + renderHtml('journal.window.home.originals.page.item.range', [['first_item', String(pageOffset + 1)], ['last_item', String(pageOffset + rows.length)], ['total_items', String(totalRows)]])
      + '</span><button type="button" data-removal-action="next"'
      + (nextOffset === null ? ' disabled' : '') + `>${window.JournalI18n.renderHtml('journal.window.home.visible.text.next')}</button></nav>`;
  }

  function render() {
    if (!card || retired || mountedRoot !== document.querySelector('[data-home-root]')) return;
    renderGeneration += 1;
    const heading = '<h2 class="pulse-section-header">' + renderHtml('journal.window.home.originals.copy.card.heading') + '</h2><p>' + renderHtml('journal.window.home.originals.copy.card.subhead') + '</p>';
    if (listState === 'list.empty') {
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p>' + renderHtml('journal.window.home.originals.copy.card.empty') + '</p>' + outcomeHtml + operationHtml() + '</section>';
    } else if (listState === 'list.ready') {
      const totals = total();
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p class="removals-card-total">'
        + renderHtml('journal.window.home.originals.card.page.total', [quantity(totals.count), originalCount(totals.count), ['size', formatBytes(totals.bytes)]])
        + '</p><details class="removals-review"' + (expanded ? ' open' : '') + `><summary>${window.JournalI18n.renderHtml(rows.some(row => row.state === 'failed') ? 'journal.window.home.originals.review.unfinished' : 'journal.window.home.disclosure.label.review.originals')}</summary>`
        + listNoteHtml() + toolbarHtml() + '<p class="removals-card-scope">' + renderHtml('journal.window.home.originals.copy.page.selection') + '</p>' + cardRows()
        + pagesHtml()
        + finishHtml() + '</details>' + confirmationHtml() + outcomeHtml + operationHtml() + '</section>';
    } else if (listState === 'list.review_too_large') {
      card.innerHTML = '<section class="removals-card">' + heading
        + rows.map(row => identity(identityText(row), row)).join('')
        + '<p data-removals-list-unavailable>' + renderHtml('journal.window.home.originals.copy.card.too.large') + '</p>'
        + '<p><a href="/app/support/">' + renderHtml('journal.window.home.originals.copy.card.support') + '</a></p>'
        + pagesHtml() + outcomeHtml + operationHtml() + '</section>';
    } else if (listState === 'list.loading') {
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p role="status">' + renderHtml('journal.window.home.originals.copy.card.loading') + '</p>' + operationHtml() + '</section>';
    } else {
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p data-removals-list-unavailable>' + renderHtml('journal.window.home.originals.copy.card.unavailable') + '</p>' + outcomeHtml + operationHtml() + '</section>';
    }
    wire();
    const focus = card.querySelector('.removals-card-confirm');
    if (focus) { focus.tabIndex = -1; focus.focus(); }
  }

  function refusalItems(response) {
    const items = Array.isArray(response.refusals) ? response.refusals : [];
    return items.reduce(function (rendered, item) {
      if (item.state === 'refusal.item_named') {
        rendered.push('<li>' + renderHtml('journal.window.home.originals.receipt.refused.named', [['original_name', String(item.name ?? '')], ['reason', String(item.reason ?? '')]]) + '</li>');
      } else if (item.state === 'refusal.item_unnamed') {
        rendered.push('<li>' + renderHtml('journal.window.home.originals.receipt.refused.reason', [['reason', String(item.reason ?? '')]]) + '</li>');
      }
      return rendered;
    }, []);
  }

  function refusalList(items) {
    return items.length === 0 ? '' : '<ul>' + items.join('') + '</ul>';
  }

  function setOutcome(html) {
    outcomeHtml = html ? '<section class="removals-card-outcome">' + html + '</section>' : '';
  }

  function approveOutcome(response, rows, items) {
    const removed = Number(response.removed_count) || 0;
    const notRemoved = Number(response.not_removed_count) || 0;
    const clauses = [
      removed > 0
        ? renderHtml('journal.window.home.originals.receipt.deleted', [quantity(removed), originalCount(removed)])
        : renderHtml('journal.window.home.originals.receipt.none.deleted', [quantity(selectionOriginals(rows))]),
      notRemoved > 0
        ? renderHtml('journal.window.home.originals.receipt.not.removed', [quantity(notRemoved), ['remaining_count', String(notRemoved)]])
        : '',
      response.halted ? renderHtml('journal.window.home.originals.copy.done.clause.halted') : ''
    ].filter(Boolean);
    setOutcome('<p>' + clauses.join(' ') + '</p>' + refusalList(items));
  }

  // Some kept, some couldn't be taken off the list: say both, not a total failure.
  function declinedPartialOutcome(response) {
    const kept = Number(response.declined_count) || 0;
    const notKept = (Number(response.refused_count) || 0) + (Number(response.unavailable_count) || 0);
    const clauses = [
      renderHtml('journal.window.home.originals.receipt.kept', [quantity(kept), originalCount(kept)]),
      notKept > 0 ? renderHtml('journal.window.home.originals.receipt.not.kept', [quantity(notKept), ['remaining_count', String(notKept)]]) : '',
      renderHtml('journal.window.home.originals.copy.done.clause.nothing.deleted')
    ].filter(Boolean);
    setOutcome('<p>' + clauses.join(' ') + '</p>');
  }

  function showOutcome(response, context) {
    const items = refusalItems(response);
    const rows = context.rows;
    const decline = context.action === 'decline';
    const originals = selectionOriginals(rows);
    const nothingRan = decline
      ? renderHtml('journal.window.home.originals.copy.done.declined.failed')
      : renderHtml('journal.window.home.originals.receipt.none.deleted', [quantity(originals)]);
    const anyOffload = rows.some(function (row) { return origin(row) === 'offload'; });
    switch (response.state) {
      case 'outcome.unknown':
        setOutcome('<p>' + renderHtml(decline ? 'journal.window.home.originals.copy.done.declined.unknown' : 'journal.window.home.originals.copy.done.unknown') + '</p>');
        break;
      case 'approve.refused_before_start':
        setOutcome('<p>' + renderHtml('journal.window.home.originals.receipt.none.deleted', [quantity(originals)]) + '</p>');
        break;
      case 'approve.refused_after_start':
        setOutcome('<p>' + renderHtml('journal.window.home.originals.receipt.none.deleted', [quantity(originals)]) + '</p>' + refusalList(items));
        break;
      case 'approve.deleted':
      case 'approve.partial':
      case 'approve.halted':
        approveOutcome(response, rows, items);
        break;
      case 'declined.done':
        setOutcome('<p>' + renderHtml(anyOffload ? 'journal.window.home.originals.copy.done.kept.offload' : 'journal.window.home.originals.copy.done.kept.policy') + '</p>');
        break;
      case 'tool.unavailable':
      case 'approve.policy_keeps':
        setOutcome('<p>' + nothingRan + '</p>');
        break;
      case 'request.too_large':
        setOutcome('<p>' + renderHtml('journal.window.home.originals.bulk.limit', [['selection_limit', String(MAX_SELECTED_MARKS)]]) + '</p>');
        break;
      case 'declined.partial':
        declinedPartialOutcome(response);
        break;
      case 'declined.refused':
        setOutcome('<p>' + renderHtml('journal.window.home.originals.copy.done.declined.failed') + '</p>');
        break;
      case 'declined.unknown':
        setOutcome('<p>' + renderHtml('journal.window.home.originals.copy.done.declined.unknown') + '</p>');
        break;
      case 'request.invalid':
        setOutcome('');
        break;
      default:
        setOutcome('');
    }
    render();
    return response.state;
  }

  function showRecoverOutcome(response, trustworthy) {
    if (response.state === 'outcome.unknown') {
      setOutcome('<p>' + renderHtml('journal.window.home.originals.copy.done.recover.unknown') + '</p>');
    } else if (response.state === 'tool.unavailable') {
      setOutcome('<p>' + renderHtml('journal.window.home.originals.copy.done.recover.failed') + '</p>');
    } else if (!trustworthy) {
      setOutcome('<p>' + renderHtml('journal.window.home.originals.copy.done.recover.unknown') + '</p>');
    } else {
      const leftover = response.state === 'recover.failed' || rows.some(function (row) {
        return row.state === 'failed';
      });
      const finished = Number(response.finished_count) || 0;
      if (leftover) {
        setOutcome('<p>' + renderHtml(finished > 0 ? 'journal.window.home.originals.copy.done.recovered.leftover' : 'journal.window.home.originals.copy.done.recover.failed') + '</p>');
      } else if (response.state === 'recover.done') {
        setOutcome('<p>' + renderHtml('journal.window.home.originals.copy.done.recovered') + '</p>');
      } else if (response.state === 'recover.none') {
        setOutcome('<p>' + renderHtml('journal.window.home.originals.copy.done.recovered.none') + '</p>');
      } else {
        setOutcome('');
      }
    }
    render();
  }

  async function refresh() {
    const generation = ++refreshGeneration;
    const epoch = mountEpoch;
    const root = mountedRoot;
    let response;
    try {
      response = await request(LIST_URL + '?offset=' + pageOffset);
    } catch (_) {
      response = { state: 'list.register_unavailable' };
    }
    if (retired || epoch !== mountEpoch || root !== document.querySelector('[data-home-root]')
        || generation !== refreshGeneration) return false;
    const validPage = Number.isSafeInteger(response?.offset) && response.offset >= 0
      && Number.isSafeInteger(response.total_rows) && response.total_rows >= 0
      && (response.next_offset === null || (Number.isSafeInteger(response.next_offset) && response.next_offset > response.offset));
    const oversized = validPage && response?.state === 'list.review_too_large'
      && Array.isArray(response.removals) && response.removals.length === 1 && response.review_context === null;
    const trustworthy = validPage && (response?.state === 'list.ready' || response?.state === 'list.empty')
      && Array.isArray(response.removals)
      && typeof response.review_context === 'string' && response.review_context.length > 0;
    const nextContext = trustworthy ? response.review_context : null;
    if (nextContext !== reviewContext || !trustworthy) {
      selected = new Set();
      confirmation = null;
    }
    reviewContext = nextContext;
    trashDays = trustworthy && Number.isInteger(response.trash_days)
      && response.trash_days >= 1 && response.trash_days <= 30 ? response.trash_days : null;
    listState = trustworthy || oversized ? response.state : 'list.register_unavailable';
    rows = trustworthy || oversized ? response.removals : [];
    if (trustworthy || oversized) {
      if (pageOffset !== response.offset) previousOffsets = [];
      pageOffset = response.offset;
      nextOffset = response.next_offset;
      totalRows = response.total_rows;
    }
    const markedIds = new Set(markedRows().map(function (row) { return row.id; }));
    selected = new Set(Array.from(selected).filter(function (id) { return markedIds.has(id); }));
    render();
    reportHomeReady(trustworthy || oversized ? (rows.length ? 'content' : 'empty') : 'error');
    return trustworthy;
  }

  function currentAction(action) {
    return !!action && !retired && activeAction === action && action.mountEpoch === mountEpoch
      && mountedRoot === document.querySelector('[data-home-root]');
  }

  function canonicalGeneration(value) {
    // This is transport, including values beyond JavaScript's exact integers.
    return typeof value === 'string' && /^(0|[1-9][0-9]*)$/.test(value)
      && value.length <= 20
      && (value.length < 20 || value <= '18446744073709551615');
  }

  function operationHtml() {
    const action = activeAction;
    if (!action || action.kind !== 'approve') return '';
    let key = action.phase;
    if (!action.settled && action.cancellationRequested) key = 'stopping';
    else if (!action.settled && action.stopInFlight) key = 'stop.pending';
    let html = '<p>' + renderHtml('journal.window.home.originals.copy.operation.' + key) + '</p>';
    if (action.observationUnavailable) html += '<p>' + renderHtml('journal.window.home.originals.copy.operation.unavailable') + '</p>';
    const targets = action.report?.outcome?.targets;
    let toTrashOnly = false;
    if (Array.isArray(targets) && targets.every(target => Array.isArray(target.removed) && Array.isArray(target.not_removed))) {
      // Deleted outright (copied off by your backup), then moved to the trash
      // (marked by your retention settings), then what wasn't deleted.
      const removed = targets.flatMap(target => target.removed);
      const trashed = targets.flatMap(target => Array.isArray(target.trashed) ? target.trashed : []);
      if (removed.length) {
        html += '<p>' + renderHtml('journal.window.home.originals.receipt.deleted', [quantity(removed.length), originalCount(removed.length)]) + '</p>';
        html += '<ul>' + removed.map(name => '<li>' + escapeHtml(name) + '</li>').join('') + '</ul>';
      }
      if (trashed.length) {
        html += '<p>' + renderHtml('journal.window.home.originals.receipt.moved.to.trash', [quantity(trashed.length), originalCount(trashed.length)]) + '</p>';
        html += '<ul>' + trashed.map(name => '<li>' + escapeHtml(name) + '</li>').join('') + '</ul>';
        const erase = eraseDate(action.report?.trash?.erase_at);
        html += '<p data-removal-space>' + (erase
          ? renderHtml('journal.window.home.originals.receipt.space.after.erase', [quantity(trashed.length), ['erase_day', erase]])
          : renderHtml('journal.window.home.originals.receipt.space.without.erase.day', [quantity(trashed.length)])) + '</p>';
      }
      const refusals = targets.flatMap(target => target.not_removed);
      if (refusals.length) {
        html += '<p>' + renderHtml('journal.window.home.originals.receipt.not.removed', [quantity(refusals.length), ['remaining_count', String(refusals.length)]]) + '</p>';
        html += '<ul>' + refusals.map(item => '<li>' + renderHtml('journal.window.home.originals.receipt.refused.named', [['original_name', String(item.entry ?? '')], ['reason', String(item.reason ?? '')]]) + '</li>').join('') + '</ul>';
      }
      if (trashed.length) {
        html += '<p><a href="' + TRASH_URL + '" data-removal-open-trash>' + renderHtml('journal.window.home.originals.copy.done.open.trash') + '</a></p>';
      }
      toTrashOnly = trashed.length > 0 && removed.length === 0 && refusals.length === 0
        && !action.observationUnavailable && (key === 'running' || key === 'completed');
    } else {
      html += '<p>' + renderHtml('journal.window.home.originals.copy.operation.record.unavailable') + '</p>';
    }
    if (action.generation && !action.settled) {
      html += '<button type="button" data-removal-action="stop"'
        + (action.stopInFlight || action.cancellationRequested ? ' disabled' : '') + '>' + renderHtml('journal.window.home.originals.copy.operation.stop') + '</button>';
    }
    // A result that only moved originals to the trash takes the neutral box.
    return '<section class="removals-card-outcome' + (toTrashOnly ? ' removals-card-outcome--trash' : '')
      + '" data-removal-operation>' + html + '</section>';
  }

  function schedulePoll(action) {
    if (!currentAction(action) || action.settled || action.timer !== null) return;
    action.timer = window.setTimeout(function () {
      action.timer = null;
      poll(action);
    }, POLL_INTERVAL_MS);
  }

  async function poll(action) {
    if (!currentAction(action) || action.settled || action.pollInFlight) return;
    action.pollInFlight = true;
    try {
      const response = await request(APPROVE_URL + '/' + action.generation, { cache: 'no-store' });
      if (!currentAction(action)) return;
      if (response?.generation !== action.generation
          || !['running', 'refused', 'held', 'unknown', 'completed'].includes(response.state)
          || typeof response.worker_settled !== 'boolean'
          || typeof response.cancellation_requested !== 'boolean') {
        action.observationUnavailable = true;
      } else {
        action.observationUnavailable = false;
        action.cancellationRequested ||= response.cancellation_requested;
        // A missing later report cannot erase earlier recorded effects.
        if (Array.isArray(response.report?.outcome?.targets)
            && response.report.outcome.targets.every(target => Array.isArray(target.removed)
              && target.removed.every(name => typeof name === 'string')
              && (target.trashed === undefined
                || (Array.isArray(target.trashed) && target.trashed.every(name => typeof name === 'string')))
              && Array.isArray(target.not_removed)
              && target.not_removed.every(item => typeof item?.entry === 'string' && typeof item.reason === 'string'))) {
          action.report = response.report;
        }
        action.phase = response.state === 'completed' && !response.worker_settled ? 'running' : response.state;
        action.settled = response.worker_settled && response.state !== 'running';
        action.locked = !action.settled || !['completed', 'refused'].includes(response.state);
        if (action.settled) {
          render();
          await refresh();
          if (!currentAction(action)) return;
        }
      }
    } catch (_) {
      if (!currentAction(action)) return;
      action.observationUnavailable = true;
    } finally {
      if (currentAction(action)) {
        action.pollInFlight = false;
        render();
        schedulePoll(action);
      }
    }
  }

  async function stop(action) {
    if (!currentAction(action) || !action.generation || action.settled
        || action.stopInFlight || action.cancellationRequested) return;
    action.stopInFlight = true;
    render();
    try {
      const response = await request(APPROVE_URL + '/' + action.generation + '/cancel', {
        method: 'POST', cache: 'no-store',
        headers: { 'Content-Type': 'application/json' }, body: '{}'
      });
      if (!currentAction(action) || action.settled) return;
      // Stop replies only acknowledge the original sticky negative flag.
      // They never replace a newer poll's settlement or effect report.
      if (response?.generation === action.generation && response.cancellation_requested === true) {
        action.cancellationRequested = true;
      }
    } catch (_) {
      // A failed request does not acknowledge cancellation or erase effects.
    } finally {
      if (currentAction(action)) {
        action.stopInFlight = false;
        render();
      }
    }
  }

  async function submit(action, selectionRows, context) {
    if (activeAction?.locked || typeof context !== 'string' || !context || context !== reviewContext) return;
    const endpoint = action === 'approve' ? APPROVE_URL : DECLINE_URL;
    const tracking = {
      kind: action, rows: selectionRows.slice(), reviewContext: context, mountEpoch: mountEpoch,
      generation: null, phase: 'submitting', settled: false, locked: true,
      pollInFlight: false, stopInFlight: false, cancellationRequested: false,
      observationUnavailable: false, report: null, timer: null
    };
    activeAction = tracking;
    confirmation = null;
    selected = new Set();
    setOutcome('');
    render();
    try {
      const response = await request(endpoint, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ review_context: context, mark_ids: tracking.rows.map(function (row) { return row.id; }) })
      });
      if (!currentAction(tracking)) return;
      if (action === 'approve' && response?.state === 'approve.started') {
        if (!canonicalGeneration(response.generation)) {
          tracking.phase = 'unknown'; render(); return;
        }
        tracking.generation = response.generation;
        tracking.phase = 'running';
        render();
        await poll(tracking);
        return;
      }
      await refresh();
      if (!currentAction(tracking)) return;
      if (action === 'approve' && ![
        'approve.refused_before_start', 'approve.refused_after_start', 'approve.deleted',
        'approve.partial', 'approve.halted', 'tool.unavailable', 'approve.policy_keeps',
        'request.too_large', 'request.invalid'
      ].includes(response?.state)) {
        tracking.phase = 'unknown'; render(); return;
      }
      activeAction = null;
      showOutcome(response, { action: action, rows: tracking.rows });
    } catch (_) {
      if (!currentAction(tracking)) return;
      if (action === 'approve') {
        tracking.phase = 'unknown';
        render();
      } else {
        await refresh();
        if (!currentAction(tracking)) return;
        activeAction = null;
        showOutcome({ state: 'declined.unknown' }, { action: action, rows: tracking.rows });
      }
    }
  }

  async function submitRecover() {
    if (activeAction?.locked) return;
    confirmation = null;
    selected = new Set();
    let response;
    try {
      response = await request(RECOVER_URL, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}'
      });
    } catch (_) {
      response = { state: 'outcome.unknown' };
    }
    const trustworthy = await refresh();
    showRecoverOutcome(response, trustworthy);
  }

  function openDeleteConfirmation(rows) {
    expanded = true;
    if (activeAction?.locked || !reviewContext) return;
    confirmation = { kind: 'delete', rows: rows, reviewContext: reviewContext };
    render();
  }

  function tooManySelected() {
    setOutcome('<p>' + renderHtml('journal.window.home.originals.bulk.limit', [['selection_limit', String(MAX_SELECTED_MARKS)]]) + '</p>');
    render();
  }

  function wire() {
    if (!card) return;
    const generation = renderGeneration;
    const context = reviewContext;
    const renderedRows = rows;
    card.querySelector('.removals-review')?.addEventListener('toggle', event => { expanded = event.currentTarget.open; });
    card.querySelectorAll('[data-removal-action]').forEach(function (control) {
      if (activeAction?.locked && !['stop', 'previous', 'next'].includes(control.dataset.removalAction)) control.disabled = true;
      control.addEventListener('click', function () {
        if (generation !== renderGeneration || context !== reviewContext) return;
        const action = control.dataset.removalAction;
        if (action === 'stop') { stop(activeAction); return; }
        if (activeAction?.locked && action !== 'previous' && action !== 'next') return;
        const row = renderedRows.find(function (candidate) {
          return candidate.id === control.dataset.markId;
        });
        if (action === 'previous' || action === 'next') {
          if (action === 'next') {
            if (nextOffset === null) return;
            previousOffsets.push(pageOffset);
            pageOffset = nextOffset;
          } else {
            if (!previousOffsets.length) return;
            pageOffset = previousOffsets.pop();
          }
          selected = new Set();
          confirmation = null;
          reviewContext = null;
          listState = 'list.loading';
          expanded = true;
          render();
          const pageCard = card;
          const pageEpoch = mountEpoch;
          refresh().then(() => {
            if (card === pageCard && mountEpoch === pageEpoch) {
              card?.querySelector('.removals-card-pages button:not(:disabled)')?.focus();
            }
          });
          return;
        }
        if (action === 'approve' && row) openDeleteConfirmation([row]);
        if (action === 'decline' && row) submit('decline', [row], context);
        if (action === 'cancel') {
          confirmation = null;
          render();
        }
        if (action === 'confirm' && confirmation && confirmation.kind === 'delete') {
          submit('approve', confirmation.rows, confirmation.reviewContext);
        }
        if (action === 'select-all') {
          selected = new Set(markedRows().map(function (item) { return item.id; }));
          render();
        }
        if (action === 'clear-selection') {
          selected = new Set();
          render();
        }
        if (action === 'delete-selected') {
          if (selected.size === 0) return;
          if (selected.size > MAX_SELECTED_MARKS) {
            tooManySelected();
            return;
          }
          openDeleteConfirmation(selectedRows());
        }
        if (action === 'keep-selected') {
          if (selected.size === 0) return;
          if (selected.size > MAX_SELECTED_MARKS) {
            tooManySelected();
            return;
          }
          submit('decline', selectedRows(), context);
        }
        if (action === 'finish') {
          confirmation = { kind: 'recover' };
          render();
        }
        if (action === 'confirm-finish') submitRecover();
      });
    });
    card.querySelectorAll('[data-removal-select]').forEach(function (control) {
      control.disabled = !!activeAction?.locked;
      control.addEventListener('click', function () {
        if (activeAction?.locked || generation !== renderGeneration || context !== reviewContext || !context) return;
        const id = control.dataset.markId;
        if (!id) return;
        if (selected.has(id)) selected.delete(id);
        else selected.add(id);
        render();
      });
    });
  }

  function detach() {
    mountEpoch += 1;
    refreshGeneration += 1;
    renderGeneration += 1;
    if (activeAction?.timer !== null && activeAction?.timer !== undefined) window.clearTimeout(activeAction.timer);
    activeAction = null;
    card = null;
    mountedRoot = null;
  }

  function mount() {
    // The originals list has its own page; the first page shows only a row
    // that links to it.
    const pathname = window.location.pathname || '';
    const root = document.querySelector('[data-home-root]');
    if (!pathname.startsWith('/app/home/originals/') || !root) { detach(); return; }
    if (root === mountedRoot && card) { refresh(); return; }
    detach();
    reviewContext = null;
    pageOffset = 0;
    nextOffset = null;
    totalRows = 0;
    previousOffsets = [];
    selected = new Set();
    confirmation = null;
    outcomeHtml = '';
    mountedRoot = root;
    card = document.createElement('div');
    card.setAttribute('data-removals-card', '');
    root.appendChild(card);
    refresh();
  }

  window.HomeOriginals = {
    mount(ctx) {
      originalsCtx = ctx;
      reportHomeReady = ctx.ready;
      retired = false;
      ctx.onDispose(function () {
        detach();
        if (originalsCtx === ctx) {
          originalsCtx = null;
          reportHomeReady = function () {};
        }
      });
      mount();
    },
    // The list read again, on the page it is already on.
    refresh() {
      mount();
    }
  };
})();
