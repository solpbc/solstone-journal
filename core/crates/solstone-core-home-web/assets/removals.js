// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  // The originals page is part of home. Home's mount hands it its context,
  // and the list goes with home.
  let reportHomeReady = function () {};
  let originalsCtx = null;
  const COPY = Object.freeze({
    "card.heading": "originals waiting on you",
    "card.subhead": "original audio and video marked for deletion, plus anything a deletion left unfinished. nothing marked here is deleted until you say so.",
    "card.empty": "nothing is waiting on you.",
    "card.unavailable": "the list couldn't be loaded.",
    "card.loading": "loading originals…",
    "card.too_large": "this item's deletion details are too large to review here.",
    "card.support": "contact support for help",
    "page.range": "items {first}–{last} of {total}",
    "page.selection": "selection applies to this page and clears when you change pages.",
    "operation.submitting": "sending your deletion request.",
    "operation.running": "deletion is in progress.",
    "operation.stop_pending": "requesting a stop.",
    "operation.stopping": "stop requested. waiting for the work to finish.",
    "operation.stop": "stop deleting",
    "operation.held": "deleting hasn't fully finished.",
    "operation.completed": "deletion finished.",
    "operation.refused": "deletion didn't start.",
    "operation.unknown": "it's unclear what was deleted and what wasn't.",
    "operation.unavailable": "the current result couldn't be checked.",
    "operation.record_unavailable": "the deletion record isn't available.",
    "card.total_one": "on this page: 1 original · {size}",
    "card.total_many": "on this page: {n} originals · {size}",
    "row.identity": "{date} · {stream}",
    "row.origin_policy_one": "1 original marked by your retention settings. this part of the list rebuilds every day.",
    "row.origin_policy_many": "{n} originals marked by your retention settings. this part of the list rebuilds every day.",
    "row.origin_offload_one": "1 original copied off your device by your backup. there's no way to check it's still there.",
    "row.origin_offload_many": "{n} originals copied off your device by your backup. there's no way to check they're still there.",
    "row.what_one": "1 original · {size}",
    "row.what_many": "{n} originals · {size}",
    "row.kept_one": "nothing else goes with it.",
    "row.kept_many": "nothing else goes with them.",
    "row.delete": "delete originals",
    "row.keep": "keep for now",
    "bulk.select_all": "select this page",
    "bulk.clear": "clear selection",
    "bulk.selected_one": "1 selected",
    "bulk.selected_many": "{n} selected",
    "bulk.selected_over_cap": "{n} selected. one action covers up to {max}.",
    "bulk.delete": "delete selected",
    "bulk.keep": "keep selected",
    "confirm.heading_one": "delete this original?",
    "confirm.heading_many": "delete these originals?",
    "confirm.days_one": "1 day",
    "confirm.days_many": "{n} days",
    "confirm.body_policy_one": "this moves 1 original from {date} · {stream} to the trash. nothing else goes with it. the trash erases it after {days}, and its space comes back then. you can recover it until then.",
    "confirm.body_policy_many": "this moves {n} originals from {date} · {stream} to the trash. nothing else goes with them. the trash erases them after {days}, and their space comes back then. you can recover them until then.",
    "confirm.body_policy_one_nodays": "this moves 1 original from {date} · {stream} to the trash. nothing else goes with it. it waits in the trash, then the trash erases it and its space comes back. you can recover it until then.",
    "confirm.body_policy_many_nodays": "this moves {n} originals from {date} · {stream} to the trash. nothing else goes with them. they wait in the trash, then the trash erases them and their space comes back. you can recover them until then.",
    "confirm.body_offload_one": "this deletes 1 original from {date} · {stream}. your backup has a copy that can't be checked from here. nothing else goes with it. it can't be undone.",
    "confirm.body_offload_many": "this deletes {n} originals from {date} · {stream}. your backup has a copy that can't be checked from here. nothing else goes with them. it can't be undone.",
    "confirm.body_policy_selected": "this moves {n} originals to the trash. nothing else goes with them. the trash erases them after {days}, and their space comes back then. you can recover them until then.",
    "confirm.body_policy_selected_nodays": "this moves {n} originals to the trash. nothing else goes with them. they wait in the trash, then the trash erases them and their space comes back. you can recover them until then.",
    "confirm.body_offload_selected": "this deletes {n} originals. your backup has a copy that can't be checked from here. nothing else goes with them. it can't be undone.",
    "confirm.mixed_lead": "this deletes {o} of these originals and moves {p} to the trash. nothing else goes with them.",
    "confirm.mixed_offload_one": "the one it deletes was copied off your device by your backup. your backup has a copy that can't be checked from here. deleting it can't be undone.",
    "confirm.mixed_offload_many": "the ones it deletes were copied off your device by your backup. your backup has a copy that can't be checked from here. deleting them can't be undone.",
    "confirm.mixed_policy_one": "the one it moves to the trash is erased after {days}, and its space comes back then. you can recover it until then.",
    "confirm.mixed_policy_many": "the ones it moves to the trash are erased after {days}, and their space comes back then. you can recover them until then.",
    "confirm.mixed_policy_one_nodays": "the one it moves to the trash waits there, then the trash erases it and its space comes back. you can recover it until then.",
    "confirm.mixed_policy_many_nodays": "the ones it moves to the trash wait there, then the trash erases them and their space comes back. you can recover them until then.",
    "confirm.go_trash": "move to trash",
    "confirm.go_one": "delete it",
    "confirm.go_many": "delete them",
    "confirm.cancel": "cancel",
    "confirm.recover.heading": "finish the deletions that stopped?",
    "confirm.recover.body": "this finishes every deletion that stopped partway. any original it hadn't deleted yet goes back in its segment. the rest stay deleted, and those your retention settings marked are in the trash.",
    "confirm.recover.go": "finish them",
    "done.clause_deleted_one": "deleted 1 original.",
    "done.clause_deleted_many": "deleted {n} originals.",
    "done.clause_trashed_one": "moved 1 original to the trash.",
    "done.clause_trashed_many": "moved {n} originals to the trash.",
    "done.space_one": "it's erased {erase}, and its space comes back then. you can erase it sooner from the trash.",
    "done.space_many": "they're erased {erase}, and their space comes back then. you can erase them sooner from the trash.",
    "done.space_one_nodate": "its space comes back when it's erased from the trash.",
    "done.space_many_nodate": "their space comes back when they're erased from the trash.",
    "done.open_trash": "open trash",
    "done.clause_not_removed_one": "1 more couldn't be deleted.",
    "done.clause_not_removed_many": "{m} more couldn't be deleted.",
    "done.clause_halted": "it stopped there.",
    "done.refused_none_one": "it couldn't be deleted. nothing was deleted.",
    "done.refused_none_many": "they couldn't be deleted. nothing was deleted.",
    "done.refused_item": "{name}: {reason}",
    "done.refused_item_unnamed": "{reason}",
    "done.unknown": "deleting stopped before what happened could be confirmed. it's unclear what was deleted and what wasn't.",
    "done.kept_policy": "kept for now. it'll be back the next time this part of the list rebuilds.",
    "done.kept_offload": "kept for now. it'll be back after your next backup.",
    "done.too_many": "choose up to {n} items at a time. nothing was deleted.",
    "done.declined_failed": "it couldn't be taken off the list, so it stays. nothing was deleted.",
    "done.clause_kept_one": "kept 1 for now.",
    "done.clause_kept_many": "kept {n} for now.",
    "done.clause_not_kept_one": "1 more couldn't be taken off the list, so it stays.",
    "done.clause_not_kept_many": "{m} more couldn't be taken off the list, so they stay.",
    "done.clause_nothing_deleted": "nothing was deleted.",
    "done.declined_unknown": "nothing was deleted. it's unclear whether it's still on the list.",
    "done.recovered": "the deletions that had stopped are now finished.",
    "done.recovered_none": "there was nothing left to finish.",
    "done.recovered_leftover": "some finished. at least one deletion that stopped is still waiting.",
    "done.recover_failed": "they couldn't be finished. what's left is still there.",
    "done.recover_unknown": "finishing stopped before what happened could be confirmed. it's unclear what was completed.",
    "failed.badge": "didn't finish",
    "failed.body": "a deletion you started stopped partway. finishing it puts back any original it hadn't deleted yet, and the rest stay deleted.",
    "failed.finish": "finish the deletion"
  });

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

  function interpolate(template, values) {
    return template.replace(/\{([^}]+)\}/g, function (_, name) {
      return escapeHtml(values?.[name] ?? '');
    });
  }

  function copy(key, values) {
    return interpolate(COPY[key], values);
  }

  function cardinality(count) {
    return Number(count) === 1 ? 'one' : 'many';
  }

  function copyForCount(prefix, count, values) {
    return copy(prefix + '_' + cardinality(count), values);
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

  function streamLabel(stream) {
    if (stream === '_default') return null;
    return typeof stream === 'string' ? window.JournalFormat.stream(stream) : null;
  }

  // The list is a deletion decision, so every row carries an anchored date
  // rather than the relative label the rest of the app uses, and the time of
  // the material it covers so two rows from one day and one stream are told
  // apart before anything is deleted. G1-30. The clock is the page's own:
  // seconds are noise on a decision made by the day, and a second convention
  // made the list read like a log file. G1-202.
  function identityLabel(row) {
    const date = window.JournalFormat.dayFull(row.day);
    const time = window.JournalFormat.segmentTimeOfDay(row.dir);
    return time === 'time unavailable' ? date : date + ' ' + time;
  }

  function identityText(row) {
    const stream = streamLabel(row.stream);
    if (stream === null) return escapeHtml(identityLabel(row));
    return copy("row.identity", { date: identityLabel(row), stream: stream });
  }

  // The label now names the source the way the owner would say it, so the
  // exact stream key rides along in the title where a deletion decision can
  // still be checked against it. G1-203.
  function identity(text, row) {
    const key = row && typeof row.stream === 'string' && row.stream !== '_default' ? row.stream : null;
    const title = key ? ' title="' + escapeHtml(key) + '"' : '';
    return '<p class="removals-card-identity" data-removal-identity' + title + '>' + text + '</p>';
  }

  function copyWithoutDefaultStream(key, values) {
    return interpolate(COPY[key].replace(' · {stream}', ''), values);
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
  function markedRow(row) {
    const count = rowCount(row);
    const checked = selected.has(row.id) ? ' checked' : '';
    return '<article class="removals-card-row" data-removal-row data-mark-id="' + escapeHtml(row.id) + '">'
      + identity(identityText(row), row)
      + '<label class="removals-select"><input type="checkbox" data-removal-select data-mark-id="' + escapeHtml(row.id) + '"' + checked + '> select this item</label>'
      + '<p class="removals-card-what">' + copyForCount('row.what', count, { n: count, size: row.size }) + '</p>'
      + '<button type="button" data-removal-action="approve" data-mark-id="' + escapeHtml(row.id) + '">'
      + copy("row.delete")
      + '</button>'
      + '<button type="button" data-removal-action="decline" data-mark-id="' + escapeHtml(row.id) + '">'
      + copy("row.keep")
      + '</button>'
      + '</article>';
  }

  function failedRow(row) {
    return '<article class="removals-card-row" data-removal-row data-mark-id="' + escapeHtml(row.id) + '">'
      + identity(identityText(row), row)
      + '<p class="removals-card-failed-badge">' + copy("failed.badge") + '</p>'
      + '<p class="removals-card-failed-body">'
      + copy("failed.body")
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
    if (present.policy) notes.push(copyForCount('row.origin_policy', originals.policy, { n: originals.policy }));
    if (present.offload) notes.push(copyForCount('row.origin_offload', originals.offload, { n: originals.offload }));
    notes.push(copyForCount('row.kept', originals.policy + originals.offload));
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
        ? copy('bulk.selected_over_cap', { n: n, max: MAX_SELECTED_MARKS })
        : copyForCount('bulk.selected', n, { n: n })) + '</span>';
    return '<div class="removals-card-toolbar">'
      + '<button type="button" data-removal-action="select-all">' + copy("bulk.select_all") + '</button>'
      + '<button type="button" data-removal-action="clear-selection">' + copy("bulk.clear") + '</button>'
      + count
      + '<button type="button" data-removal-action="delete-selected"' + disabled + '>' + copy("bulk.delete") + '</button>'
      + '<button type="button" data-removal-action="keep-selected"' + disabled + '>' + copy("bulk.keep") + '</button>'
      + '</div>';
  }

  function finishHtml() {
    if (!rows.some(function (row) { return row.state === 'failed'; })) return '';
    return '<p class="removals-card-finish"><button type="button" data-removal-action="finish">'
      + copy("failed.finish")
      + '</button></p>';
  }

  function confirmationHtml() {
    if (!confirmation) return '';
    if (confirmation.kind === 'recover') {
      return '<section class="removals-card-confirm" role="dialog">'
        + '<h3>' + copy("confirm.recover.heading") + '</h3>'
        + '<p>' + copy("confirm.recover.body") + '</p>'
        + '<button type="button" data-removal-action="confirm-finish">' + copy("confirm.recover.go") + '</button>'
        + '<button type="button" data-removal-action="cancel">' + copy("confirm.cancel") + '</button>'
        + '</section>';
    }
    const confirmRows = confirmation.rows;
    const days = trashDays === null ? null : copyForCount('confirm.days', trashDays, { n: trashDays });
    // A policy key with its day count, or its form without a number.
    const policyKey = function (key) { return days === null ? key + '_nodays' : key; };
    if (confirmRows.length === 1) {
      const row = confirmRows[0];
      const count = rowCount(row);
      const stream = streamLabel(row.stream);
      const policy = origin(row) === 'policy';
      const bodyKey = 'confirm.body_' + origin(row) + '_' + cardinality(count);
      const values = { n: count, date: identityLabel(row), stream: stream, days: days };
      const key = policy ? policyKey(bodyKey) : bodyKey;
      const body = stream === null ? copyWithoutDefaultStream(key, values) : copy(key, values);
      return confirmSection(copyForCount('confirm.heading', count), [body],
        policy ? copy('confirm.go_trash') : copyForCount('confirm.go', count), policy);
    }
    // Counted by origin over the selection, as the note above the list counts them.
    const byOrigin = { policy: 0, offload: 0 };
    confirmRows.forEach(function (row) { byOrigin[origin(row)] += rowCount(row); });
    const n = byOrigin.policy + byOrigin.offload;
    const heading = copy("confirm.heading_many");
    if (byOrigin.offload === 0) {
      return confirmSection(heading, [copy(policyKey('confirm.body_policy_selected'), { n: n, days: days })],
        copy('confirm.go_trash'), true);
    }
    if (byOrigin.policy === 0) {
      return confirmSection(heading, [copy('confirm.body_offload_selected', { n: n })], copy("confirm.go_many"), false);
    }
    // Both kinds: part of it can't be undone, so it keeps the warning look.
    return confirmSection(heading, [
      copy('confirm.mixed_lead', { o: byOrigin.offload, p: byOrigin.policy }),
      copyForCount('confirm.mixed_offload', byOrigin.offload),
      copy(policyKey('confirm.mixed_policy_' + cardinality(byOrigin.policy)), { days: days })
    ], copy("confirm.go_many"), false);
  }

  // A move to the trash is reversible, so its dialog isn't the warning box and
  // its button isn't danger.
  function confirmSection(heading, paragraphs, go, toTrash) {
    return '<section class="removals-card-confirm' + (toTrash ? ' removals-card-confirm--trash' : '') + '" role="dialog">'
      + '<h3>' + heading + '</h3>'
      + paragraphs.map(function (text) { return '<p>' + text + '</p>'; }).join('')
      + '<button type="button" data-removal-action="confirm"' + (toTrash ? ' class="removals-go-trash"' : '') + '>' + go + '</button>'
      + '<button type="button" data-removal-action="cancel">' + copy("confirm.cancel") + '</button>'
      + '</section>';
  }

  function pagesHtml() {
    return '<nav class="removals-card-pages" aria-label="originals pages"><button type="button" data-removal-action="previous"'
      + (previousOffsets.length === 0 ? ' disabled' : '') + '>previous</button><span>'
      + copy('page.range', { first: pageOffset + 1, last: pageOffset + rows.length, total: totalRows })
      + '</span><button type="button" data-removal-action="next"'
      + (nextOffset === null ? ' disabled' : '') + '>next</button></nav>';
  }

  function render() {
    if (!card || retired || mountedRoot !== document.querySelector('[data-home-root]')) return;
    renderGeneration += 1;
    const heading = '<h2 class="pulse-section-header">' + copy("card.heading") + '</h2><p>' + copy("card.subhead") + '</p>';
    if (listState === 'list.empty') {
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p>' + copy("card.empty") + '</p>' + outcomeHtml + operationHtml() + '</section>';
    } else if (listState === 'list.ready') {
      const totals = total();
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p class="removals-card-total">'
        + copyForCount('card.total', totals.count, { n: totals.count, size: formatBytes(totals.bytes) })
        + '</p><details class="removals-review"' + (expanded ? ' open' : '') + '><summary>review originals' + (rows.some(row => row.state === 'failed') ? ' · unfinished deletions need review' : '') + '</summary>'
        + listNoteHtml() + toolbarHtml() + '<p class="removals-card-scope">' + copy('page.selection') + '</p>' + cardRows()
        + pagesHtml()
        + finishHtml() + '</details>' + confirmationHtml() + outcomeHtml + operationHtml() + '</section>';
    } else if (listState === 'list.review_too_large') {
      card.innerHTML = '<section class="removals-card">' + heading
        + rows.map(row => identity(identityText(row), row)).join('')
        + '<p data-removals-list-unavailable>' + copy('card.too_large') + '</p>'
        + '<p><a href="/app/support/">' + copy('card.support') + '</a></p>'
        + pagesHtml() + outcomeHtml + operationHtml() + '</section>';
    } else if (listState === 'list.loading') {
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p role="status">' + copy('card.loading') + '</p>' + operationHtml() + '</section>';
    } else {
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p data-removals-list-unavailable>' + copy("card.unavailable") + '</p>' + outcomeHtml + operationHtml() + '</section>';
    }
    wire();
    const focus = card.querySelector('.removals-card-confirm');
    if (focus) { focus.tabIndex = -1; focus.focus(); }
  }

  function refusalItems(response) {
    const items = Array.isArray(response.refusals) ? response.refusals : [];
    return items.reduce(function (rendered, item) {
      if (item.state === 'refusal.item_named') {
        rendered.push('<li>' + copy("done.refused_item", { name: item.name, reason: item.reason }) + '</li>');
      } else if (item.state === 'refusal.item_unnamed') {
        rendered.push('<li>' + copy("done.refused_item_unnamed", { reason: item.reason }) + '</li>');
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
        ? copyForCount('done.clause_deleted', removed, { n: removed })
        : copyForCount('done.refused_none', selectionOriginals(rows)),
      notRemoved > 0
        ? copyForCount('done.clause_not_removed', notRemoved, { m: notRemoved })
        : '',
      response.halted ? copy("done.clause_halted") : ''
    ].filter(Boolean);
    setOutcome('<p>' + clauses.join(' ') + '</p>' + refusalList(items));
  }

  // Some kept, some couldn't be taken off the list: say both, not a total failure.
  function declinedPartialOutcome(response) {
    const kept = Number(response.declined_count) || 0;
    const notKept = (Number(response.refused_count) || 0) + (Number(response.unavailable_count) || 0);
    const clauses = [
      copyForCount('done.clause_kept', kept, { n: kept }),
      notKept > 0 ? copyForCount('done.clause_not_kept', notKept, { m: notKept }) : '',
      copy("done.clause_nothing_deleted")
    ].filter(Boolean);
    setOutcome('<p>' + clauses.join(' ') + '</p>');
  }

  function showOutcome(response, context) {
    const items = refusalItems(response);
    const rows = context.rows;
    const decline = context.action === 'decline';
    const originals = selectionOriginals(rows);
    const nothingRan = decline
      ? copy("done.declined_failed")
      : copyForCount('done.refused_none', originals);
    const anyOffload = rows.some(function (row) { return origin(row) === 'offload'; });
    switch (response.state) {
      case 'outcome.unknown':
        setOutcome('<p>' + copy(decline ? "done.declined_unknown" : "done.unknown") + '</p>');
        break;
      case 'approve.refused_before_start':
        setOutcome('<p>' + copyForCount('done.refused_none', originals) + '</p>');
        break;
      case 'approve.refused_after_start':
        setOutcome('<p>' + copyForCount('done.refused_none', originals) + '</p>' + refusalList(items));
        break;
      case 'approve.deleted':
      case 'approve.partial':
      case 'approve.halted':
        approveOutcome(response, rows, items);
        break;
      case 'declined.done':
        setOutcome('<p>' + copy(anyOffload ? "done.kept_offload" : "done.kept_policy") + '</p>');
        break;
      case 'tool.unavailable':
      case 'approve.policy_keeps':
        setOutcome('<p>' + nothingRan + '</p>');
        break;
      case 'request.too_large':
        setOutcome('<p>' + copy("done.too_many", { n: MAX_SELECTED_MARKS }) + '</p>');
        break;
      case 'declined.partial':
        declinedPartialOutcome(response);
        break;
      case 'declined.refused':
        setOutcome('<p>' + copy("done.declined_failed") + '</p>');
        break;
      case 'declined.unknown':
        setOutcome('<p>' + copy("done.declined_unknown") + '</p>');
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
      setOutcome('<p>' + copy("done.recover_unknown") + '</p>');
    } else if (response.state === 'tool.unavailable') {
      setOutcome('<p>' + copy("done.recover_failed") + '</p>');
    } else if (!trustworthy) {
      setOutcome('<p>' + copy("done.recover_unknown") + '</p>');
    } else {
      const leftover = response.state === 'recover.failed' || rows.some(function (row) {
        return row.state === 'failed';
      });
      const finished = Number(response.finished_count) || 0;
      if (leftover) {
        setOutcome('<p>' + copy(finished > 0 ? "done.recovered_leftover" : "done.recover_failed") + '</p>');
      } else if (response.state === 'recover.done') {
        setOutcome('<p>' + copy("done.recovered") + '</p>');
      } else if (response.state === 'recover.none') {
        setOutcome('<p>' + copy("done.recovered_none") + '</p>');
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
    let key = 'operation.' + action.phase;
    if (!action.settled && action.cancellationRequested) key = 'operation.stopping';
    else if (!action.settled && action.stopInFlight) key = 'operation.stop_pending';
    let html = '<p>' + copy(key) + '</p>';
    if (action.observationUnavailable) html += '<p>' + copy('operation.unavailable') + '</p>';
    const targets = action.report?.outcome?.targets;
    let toTrashOnly = false;
    if (Array.isArray(targets) && targets.every(target => Array.isArray(target.removed) && Array.isArray(target.not_removed))) {
      // Deleted outright (copied off by your backup), then moved to the trash
      // (marked by your retention settings), then what wasn't deleted.
      const removed = targets.flatMap(target => target.removed);
      const trashed = targets.flatMap(target => Array.isArray(target.trashed) ? target.trashed : []);
      if (removed.length) {
        html += '<p>' + copyForCount('done.clause_deleted', removed.length, { n: removed.length }) + '</p>';
        html += '<ul>' + removed.map(name => '<li>' + escapeHtml(name) + '</li>').join('') + '</ul>';
      }
      if (trashed.length) {
        html += '<p>' + copyForCount('done.clause_trashed', trashed.length, { n: trashed.length }) + '</p>';
        html += '<ul>' + trashed.map(name => '<li>' + escapeHtml(name) + '</li>').join('') + '</ul>';
        const erase = eraseDate(action.report?.trash?.erase_at);
        html += '<p data-removal-space>' + (erase
          ? copyForCount('done.space', trashed.length, { erase: erase })
          : copy('done.space_' + cardinality(trashed.length) + '_nodate')) + '</p>';
      }
      const refusals = targets.flatMap(target => target.not_removed);
      if (refusals.length) {
        html += '<p>' + copyForCount('done.clause_not_removed', refusals.length, { m: refusals.length }) + '</p>';
        html += '<ul>' + refusals.map(item => '<li>' + copy('done.refused_item', { name: item.entry, reason: item.reason }) + '</li>').join('') + '</ul>';
      }
      if (trashed.length) {
        html += '<p><a href="' + TRASH_URL + '" data-removal-open-trash>' + copy('done.open_trash') + '</a></p>';
      }
      toTrashOnly = trashed.length > 0 && removed.length === 0 && refusals.length === 0
        && !action.observationUnavailable && (key === 'operation.running' || key === 'operation.completed');
    } else {
      html += '<p>' + copy('operation.record_unavailable') + '</p>';
    }
    if (action.generation && !action.settled) {
      html += '<button type="button" data-removal-action="stop"'
        + (action.stopInFlight || action.cancellationRequested ? ' disabled' : '') + '>' + copy('operation.stop') + '</button>';
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
    setOutcome('<p>' + copy("done.too_many", { n: MAX_SELECTED_MARKS }) + '</p>');
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
