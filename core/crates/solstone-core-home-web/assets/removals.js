// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const COPY = Object.freeze({
    "card.heading": "originals waiting on you",
    "card.subhead": "original audio and video marked for deletion, plus anything a deletion left unfinished. nothing marked here is deleted until you say so.",
    "card.empty": "nothing is waiting on you.",
    "card.unavailable": "the list couldn't be loaded. nothing has been deleted.",
    "card.total_one": "in all: 1 original · {size}",
    "card.total_many": "in all: {n} originals · {size}",
    "row.identity": "{date} · {stream}",
    "row.origin_policy_one": "your retention settings marked this. this part of the list rebuilds every day.",
    "row.origin_policy_many": "your retention settings marked these. this part of the list rebuilds every day.",
    "row.origin_offload_one": "your backup copied this off your device. whether it's still there can't be checked.",
    "row.origin_offload_many": "your backup copied these off your device. whether they're still there can't be checked.",
    "row.what_one": "1 original · {size}",
    "row.what_many": "{n} originals · {size}",
    "row.kept_one": "nothing else goes with it.",
    "row.kept_many": "nothing else goes with them.",
    "row.delete": "delete originals",
    "row.keep": "keep for now",
    "bulk.select_all": "select all",
    "bulk.clear": "clear selection",
    "bulk.selected_one": "1 selected",
    "bulk.selected_many": "{n} selected",
    "bulk.delete": "delete selected",
    "bulk.keep": "keep selected",
    "confirm.heading_one": "delete this original?",
    "confirm.heading_many": "delete these originals?",
    "confirm.body_policy_one": "this deletes 1 original from {date} · {stream}. nothing else goes with it. it can't be undone.",
    "confirm.body_policy_many": "this deletes {n} originals from {date} · {stream}. nothing else goes with them. it can't be undone.",
    "confirm.body_offload_one": "this deletes 1 original from {date} · {stream}. your backup has a copy that can't be checked. nothing else goes with it. it can't be undone.",
    "confirm.body_offload_many": "this deletes {n} originals from {date} · {stream}. your backup has a copy that can't be checked. nothing else goes with them. it can't be undone.",
    "confirm.body_policy_selected": "this deletes {n} originals. nothing else goes with them. it can't be undone.",
    "confirm.body_offload_selected": "this deletes {n} originals. your backup has a copy that can't be checked. nothing else goes with them. it can't be undone.",
    "confirm.go_one": "delete it",
    "confirm.go_many": "delete them",
    "confirm.cancel": "cancel",
    "confirm.recover.heading": "finish the deletions that stopped?",
    "confirm.recover.body": "this completes every deletion that stopped partway. it writes the record that those originals are gone. it does not put anything back.",
    "confirm.recover.go": "finish them",
    "done.clause_deleted_one": "deleted 1 original.",
    "done.clause_deleted_many": "deleted {n} originals.",
    "done.clause_not_removed_one": "1 more couldn't be deleted.",
    "done.clause_not_removed_many": "{m} more couldn't be deleted.",
    "done.clause_halted": "it stopped there.",
    "done.refused_none_one": "it couldn't be deleted. nothing was deleted.",
    "done.refused_none_many": "they couldn't be deleted. nothing was deleted.",
    "done.refused_item": "{name}: {reason}",
    "done.refused_item_unnamed": "{reason}",
    "done.unknown": "deleting stopped before what happened could be confirmed. what was deleted and what wasn't isn't known.",
    "done.kept_policy": "kept for now. it'll be back the next time this part of the list rebuilds.",
    "done.kept_offload": "kept for now. it'll be back after your next backup.",
    "done.too_many": "choose up to {n} items at a time. nothing was deleted.",
    "done.declined_failed": "it couldn't be taken off the list, so it stays. nothing was deleted.",
    "done.declined_unknown": "nothing was deleted. whether it's still on the list isn't known.",
    "done.recovered": "the deletions that had stopped are now finished.",
    "done.recovered_none": "there was nothing left to finish.",
    "done.recovered_leftover": "some finished. at least one deletion that stopped is still waiting.",
    "done.recover_failed": "they couldn't be finished. what's left is still there.",
    "done.recover_unknown": "finishing stopped before what happened could be confirmed. what was completed isn't known.",
    "failed.badge": "didn't finish",
    "failed.body": "a deletion you started stopped partway. finishing it completes that deletion. it does not put anything back.",
    "failed.finish": "finish the deletion"
  });

  const LIST_URL = '/app/home/api/removals';
  const APPROVE_URL = '/app/home/api/approve';
  const DECLINE_URL = '/app/home/api/decline';
  const RECOVER_URL = '/app/home/api/recover';
  const MAX_SELECTED_MARKS = 32;

  let card = null;
  let mountedRoot = null;
  let rows = [];
  let listState = '';
  let confirmation = null;
  let outcomeHtml = '';
  let selected = new Set();
  let pageIndex = 0;
  let expanded = false;
  const PAGE_SIZE = 20;

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

  function streamLabel(stream) {
    if (stream === '_default') return null;
    return typeof stream === 'string' ? window.JournalFormat.stream(stream) : null;
  }

  // The list is a deletion decision, so every row carries an anchored date
  // rather than the relative label the rest of the app uses.
  function identityText(row) {
    const stream = streamLabel(row.stream);
    if (stream === null) return escapeHtml(window.JournalFormat.dayFull(row.day));
    return copy("row.identity", { date: window.JournalFormat.dayFull(row.day), stream: stream });
  }

  function identity(text) {
    return '<p class="removals-card-identity" data-removal-identity>' + text + '</p>';
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

  function markedRow(row) {
    const count = rowCount(row);
    const rowOrigin = origin(row);
    const checked = selected.has(row.id) ? ' checked' : '';
    return '<article class="removals-card-row" data-removal-row data-mark-id="' + escapeHtml(row.id) + '">'
      + identity(identityText(row))
      + '<label class="removals-select"><input type="checkbox" data-removal-select data-mark-id="' + escapeHtml(row.id) + '"' + checked + '> select this item</label>'
      + '<p class="removals-card-origin">' + copyForCount('row.origin_' + rowOrigin, count) + '</p>'
      + '<p class="removals-card-what">' + copyForCount('row.what', count, { n: count, size: row.size }) + '</p>'
      + '<p class="removals-card-kept">' + copyForCount('row.kept', count) + '</p>'
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
      + identity(identityText(row))
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
    return rows.slice(pageIndex * PAGE_SIZE, (pageIndex + 1) * PAGE_SIZE).map(function (row) {
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

  function toolbarHtml() {
    if (markedRows().length === 0) return '';
    const n = selected.size;
    const disabled = n === 0 ? ' disabled' : '';
    const count = n === 0 ? '' : '<span>' + copyForCount('bulk.selected', n, { n: n }) + '</span>';
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
    if (confirmRows.length === 1) {
      const row = confirmRows[0];
      const count = rowCount(row);
      const stream = streamLabel(row.stream);
      const bodyKey = 'confirm.body_' + origin(row) + '_' + cardinality(count);
      const values = { n: count, date: window.JournalFormat.dayFull(row.day), stream: stream };
      const body = stream === null ? copyWithoutDefaultStream(bodyKey, values) : copy(bodyKey, values);
      return '<section class="removals-card-confirm" role="dialog">'
        + '<h3>' + copyForCount('confirm.heading', count) + '</h3>'
        + '<p>' + body + '</p>'
        + '<button type="button" data-removal-action="confirm">' + copyForCount('confirm.go', count) + '</button>'
        + '<button type="button" data-removal-action="cancel">' + copy("confirm.cancel") + '</button>'
        + '</section>';
    }
    const n = selectionOriginals(confirmRows);
    const offload = confirmRows.some(function (row) { return origin(row) === 'offload'; });
    const bodyKey = offload ? 'confirm.body_offload_selected' : 'confirm.body_policy_selected';
    return '<section class="removals-card-confirm" role="dialog">'
      + '<h3>' + copy("confirm.heading_many") + '</h3>'
      + '<p>' + copy(bodyKey, { n: n }) + '</p>'
      + '<button type="button" data-removal-action="confirm">' + copy("confirm.go_many") + '</button>'
      + '<button type="button" data-removal-action="cancel">' + copy("confirm.cancel") + '</button>'
      + '</section>';
  }

  function render() {
    if (!card) return;
    pageIndex = Math.min(pageIndex, Math.max(0, Math.ceil(rows.length / PAGE_SIZE) - 1));
    const heading = '<h2>' + copy("card.heading") + '</h2><p>' + copy("card.subhead") + '</p>';
    if (listState === 'list.empty') {
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p>' + copy("card.empty") + '</p>' + outcomeHtml + '</section>';
    } else if (listState === 'list.ready') {
      const totals = total();
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p class="removals-card-total">'
        + copyForCount('card.total', totals.count, { n: totals.count, size: formatBytes(totals.bytes) })
        + '</p><details class="removals-review"' + (expanded ? ' open' : '') + '><summary>review originals' + (rows.some(row => row.state === 'failed') ? ' · unfinished deletions need review' : '') + '</summary>'
        + toolbarHtml() + '<p class="removals-card-scope">selection applies across every page. choose up to ' + MAX_SELECTED_MARKS + ' items per action.</p>' + cardRows()
        + '<nav class="removals-card-pages" aria-label="originals pages"><button type="button" data-removal-action="previous"' + (pageIndex === 0 ? ' disabled' : '') + '>previous</button><span>page ' + (pageIndex + 1) + ' of ' + Math.max(1, Math.ceil(rows.length / PAGE_SIZE)) + '</span><button type="button" data-removal-action="next"' + ((pageIndex + 1) * PAGE_SIZE >= rows.length ? ' disabled' : '') + '>next</button></nav>'
        + finishHtml() + '</details>' + confirmationHtml() + outcomeHtml + '</section>';
    } else {
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p>' + copy("card.unavailable") + '</p>' + outcomeHtml + '</section>';
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
    let trustworthy = false;
    try {
      const response = await request(LIST_URL);
      listState = response.state;
      rows = Array.isArray(response.removals) ? response.removals : [];
      trustworthy = (response.state === 'list.ready' || response.state === 'list.empty')
        && Array.isArray(response.removals);
    } catch (_) {
      listState = 'list.register_unavailable';
      rows = [];
    }
    const markedIds = new Set(markedRows().map(function (row) { return row.id; }));
    selected = new Set(Array.from(selected).filter(function (id) { return markedIds.has(id); }));
    render();
    return trustworthy;
  }

  async function submit(action, selectionRows) {
    const endpoint = action === 'approve' ? APPROVE_URL : DECLINE_URL;
    confirmation = null;
    selected = new Set();
    try {
      const response = await request(endpoint, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ mark_ids: selectionRows.map(function (row) { return row.id; }) })
      });
      await refresh();
      showOutcome(response, { action: action, rows: selectionRows });
    } catch (_) {
      await refresh();
      showOutcome(
        { state: action === 'decline' ? 'declined.unknown' : 'outcome.unknown' },
        { action: action, rows: selectionRows }
      );
    }
  }

  async function submitRecover() {
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
    confirmation = { kind: 'delete', rows: rows };
    render();
  }

  function tooManySelected() {
    setOutcome('<p>' + copy("done.too_many", { n: MAX_SELECTED_MARKS }) + '</p>');
    render();
  }

  function wire() {
    if (!card) return;
    card.querySelector('.removals-review')?.addEventListener('toggle', event => { expanded = event.currentTarget.open; });
    card.querySelectorAll('[data-removal-action]').forEach(function (control) {
      control.addEventListener('click', function () {
        const action = control.dataset.removalAction;
        const row = rows.find(function (candidate) {
          return candidate.id === control.dataset.markId;
        });
        if (action === 'previous' || action === 'next') {
          pageIndex += action === 'next' ? 1 : -1; expanded = true; render();
          card.querySelector('.removals-card-pages button:not(:disabled)')?.focus();
          return;
        }
        if (action === 'approve' && row) openDeleteConfirmation([row]);
        if (action === 'decline' && row) submit('decline', [row]);
        if (action === 'cancel') {
          confirmation = null;
          render();
        }
        if (action === 'confirm' && confirmation && confirmation.kind === 'delete') {
          submit('approve', confirmation.rows);
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
          submit('decline', selectedRows());
        }
        if (action === 'finish') {
          confirmation = { kind: 'recover' };
          render();
        }
        if (action === 'confirm-finish') submitRecover();
      });
    });
    card.querySelectorAll('[data-removal-select]').forEach(function (control) {
      control.addEventListener('click', function () {
        const id = control.dataset.markId;
        if (!id) return;
        if (selected.has(id)) selected.delete(id);
        else selected.add(id);
        render();
      });
    });
  }

  function mount() {
    const root = document.querySelector('[data-home-root]');
    if (!root) return;
    if (root === mountedRoot && card) return;
    mountedRoot = root;
    card = document.createElement('div');
    card.setAttribute('data-removals-card', '');
    root.appendChild(card);
    refresh();
  }

  document.addEventListener('workspace:mounted', function (event) {
    const appName = event?.detail?.app || event?.detail?.name || '';
    if (appName && appName !== 'home') return;
    mount();
  });
  if (document.readyState === 'complete') mount();
})();
