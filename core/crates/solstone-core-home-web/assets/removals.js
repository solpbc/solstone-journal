// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const COPY = Object.freeze({
    "card.heading": "originals waiting on you",
    "card.subhead": "original audio and video marked for deletion, plus anything a deletion left unfinished. nothing marked here is deleted until you say so.",
    "card.empty": "nothing is waiting on you.",
    "card.unavailable": "i couldn't load the list. nothing has been deleted.",
    "card.total_one": "in all: 1 original · {size}",
    "card.total_many": "in all: {n} originals · {size}",
    "row.identity": "{date} · {stream}",
    "row.origin_policy_one": "your retention settings marked this. i rebuild that part of the list every day.",
    "row.origin_policy_many": "your retention settings marked these. i rebuild that part of the list every day.",
    "row.origin_offload_one": "your backup copied this off your device. i can't check it's still there.",
    "row.origin_offload_many": "your backup copied these off your device. i can't check they're still there.",
    "row.what_one": "1 original · {size}",
    "row.what_many": "{n} originals · {size}",
    "row.kept_one": "nothing else goes with it.",
    "row.kept_many": "nothing else goes with them.",
    "row.delete": "delete originals",
    "row.keep": "keep for now",
    "confirm.heading_one": "delete this original?",
    "confirm.heading_many": "delete these originals?",
    "confirm.body_policy_one": "this deletes 1 original from {date} · {stream}. nothing else goes with it. it can't be undone.",
    "confirm.body_policy_many": "this deletes {n} originals from {date} · {stream}. nothing else goes with them. it can't be undone.",
    "confirm.body_offload_one": "this deletes 1 original from {date} · {stream}. your backup has a copy i can't check. nothing else goes with it. it can't be undone.",
    "confirm.body_offload_many": "this deletes {n} originals from {date} · {stream}. your backup has a copy i can't check. nothing else goes with them. it can't be undone.",
    "confirm.go_one": "delete it",
    "confirm.go_many": "delete them",
    "confirm.cancel": "cancel",
    "done.clause_deleted_one": "deleted 1 original.",
    "done.clause_deleted_many": "deleted {n} originals.",
    "done.clause_not_removed_one": "i couldn't delete 1 more.",
    "done.clause_not_removed_many": "i couldn't delete {m} more.",
    "done.clause_halted": "i didn't get any further.",
    "done.refused_none_one": "i couldn't delete it. nothing was deleted.",
    "done.refused_none_many": "i couldn't delete them. nothing was deleted.",
    "done.refused_item": "{name}: {reason}",
    "done.refused_item_unnamed": "{reason}",
    "done.unknown": "deleting stopped before i could tell what happened. i don't know what was deleted and what wasn't.",
    "done.kept_policy": "kept for now. it'll be back the next time i rebuild that part of the list.",
    "done.kept_offload": "kept for now. it'll be back after your next backup.",
    "done.too_many": "something went wrong. nothing was deleted.",
    "done.declined_failed": "i couldn't take it off the list, so it stays. nothing was deleted.",
    "done.declined_unknown": "nothing was deleted. i don't know whether it's still on the list.",
    "failed.badge": "didn't finish",
    "failed.body": "a deletion you started elsewhere stopped partway. i can't finish that deletion or undo it. what's left is still there, at {staged}, and you can delete it yourself."
  });

  const LIST_URL = '/app/home/api/removals';
  const APPROVE_URL = '/app/home/api/approve';
  const DECLINE_URL = '/app/home/api/decline';

  let card = null;
  let mountedRoot = null;
  let rows = [];
  let listState = '';
  let confirmation = null;
  let outcomeHtml = '';

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
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let amount = value;
    let unit = 'KB';
    for (let index = 1; index < units.length; index += 1) {
      if (amount < 1024) break;
      amount /= 1024;
      unit = units[index];
    }
    return amount >= 1024 && unit === 'TB'
      ? (amount / 1024).toFixed(1) + ' PB'
      : amount.toFixed(1) + ' ' + unit;
  }

  function streamLabel(stream) {
    if (stream === '_default') return null;
    return typeof stream === 'string' ? stream : null;
  }

  function identityText(row) {
    const stream = streamLabel(row.stream);
    if (stream === null) return escapeHtml(row.day);
    return copy("row.identity", { date: row.day, stream: stream });
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

  function origin(row) {
    return row.origin === 'offload' ? 'offload' : 'policy';
  }

  function markedRow(row) {
    const count = rowCount(row);
    const rowOrigin = origin(row);
    return '<article class="removals-card-row" data-removal-row data-mark-id="' + escapeHtml(row.id) + '">'
      + identity(identityText(row))
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
      + copy("failed.body", { staged: row.staged })
      + '</p>'
      + '</article>';
  }

  function markedRows() {
    return rows.filter(function (row) {
      return row.state === 'marked';
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

  function confirmationHtml() {
    if (!confirmation) return '';
    const row = confirmation.row;
    const count = rowCount(row);
    const stream = streamLabel(row.stream);
    const bodyKey = 'confirm.body_' + origin(row) + '_' + cardinality(count);
    const values = { n: count, date: row.day, stream: stream };
    const body = stream === null ? copyWithoutDefaultStream(bodyKey, values) : copy(bodyKey, values);
    return '<section class="removals-card-confirm" role="dialog">'
      + '<h3>' + copyForCount('confirm.heading', count) + '</h3>'
      + '<p>' + body + '</p>'
      + '<button type="button" data-removal-action="confirm">' + copyForCount('confirm.go', count) + '</button>'
      + '<button type="button" data-removal-action="cancel">' + copy("confirm.cancel") + '</button>'
      + '</section>';
  }

  function render() {
    if (!card) return;
    const heading = '<h2>' + copy("card.heading") + '</h2><p>' + copy("card.subhead") + '</p>';
    if (listState === 'list.empty') {
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p>' + copy("card.empty") + '</p>' + outcomeHtml + '</section>';
    } else if (listState === 'list.ready') {
      const totals = total();
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p class="removals-card-total">'
        + copyForCount('card.total', totals.count, { n: totals.count, size: formatBytes(totals.bytes) })
        + '</p>' + cardRows() + confirmationHtml() + outcomeHtml + '</section>';
    } else if (listState === 'outcome.unknown') {
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p class="removals-card-outcome">' + copy("done.unknown") + '</p>'
        + outcomeHtml + '</section>';
    } else {
      card.innerHTML = '<section class="removals-card">' + heading
        + '<p>' + copy("card.unavailable") + '</p>' + outcomeHtml + '</section>';
    }
    wire();
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

  function approveOutcome(response, row, items) {
    const removed = Number(response.removed_count) || 0;
    const notRemoved = Number(response.not_removed_count) || 0;
    const clauses = [
      removed > 0
        ? copyForCount('done.clause_deleted', removed, { n: removed })
        : copyForCount('done.refused_none', rowCount(row)),
      notRemoved > 0
        ? copyForCount('done.clause_not_removed', notRemoved, { m: notRemoved })
        : '',
      response.halted ? copy("done.clause_halted") : ''
    ].filter(Boolean);
    setOutcome('<p>' + clauses.join(' ') + '</p>' + refusalList(items));
  }

  function showOutcome(response, context) {
    const items = refusalItems(response);
    const row = context.row;
    switch (response.state) {
      case 'outcome.unknown':
        setOutcome('<p>' + copy("done.unknown") + '</p>');
        break;
      case 'approve.refused_before_start':
        setOutcome('<p>' + copyForCount('done.refused_none', rowCount(row)) + '</p>');
        break;
      case 'approve.refused_after_start':
        setOutcome('<p>' + copyForCount('done.refused_none', rowCount(row)) + '</p>' + refusalList(items));
        break;
      case 'approve.deleted':
      case 'approve.partial':
      case 'approve.halted':
        approveOutcome(response, row, items);
        break;
      case 'declined.done':
        setOutcome('<p>' + copy(origin(row) === 'offload' ? "done.kept_offload" : "done.kept_policy") + '</p>');
        break;
      case 'tool.unavailable':
      case 'approve.policy_keeps':
        setOutcome('<p>' + copyForCount('done.refused_none', rowCount(row)) + '</p>');
        break;
      case 'request.too_large':
        setOutcome('<p>' + copy("done.too_many") + '</p>');
        break;
      case 'declined.partial':
      case 'declined.refused':
        setOutcome('<p>' + copy("done.declined_failed") + '</p>');
        break;
      case 'declined.unknown':
        setOutcome('<p>' + copy("done.declined_unknown") + '</p>');
        break;
      case 'request.invalid':
        return response.state;
      default:
        setOutcome('');
    }
    render();
    return response.state;
  }

  async function refresh() {
    try {
      const response = await request(LIST_URL);
      listState = response.state;
      rows = Array.isArray(response.removals) ? response.removals : [];
    } catch (_) {
      listState = 'list.register_unavailable';
      rows = [];
    }
    render();
  }

  async function submit(action, row) {
    const endpoint = action === 'approve' ? APPROVE_URL : DECLINE_URL;
    confirmation = null;
    try {
      const response = await request(endpoint, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ mark_ids: [row.id] })
      });
      await refresh();
      showOutcome(response, { row: row });
    } catch (_) {
      await refresh();
      showOutcome({ state: action === 'decline' ? 'declined.unknown' : 'outcome.unknown' }, { row: row });
    }
  }

  function openConfirmation(row) {
    confirmation = { row: row };
    render();
  }

  function wire() {
    if (!card) return;
    card.querySelectorAll('[data-removal-action]').forEach(function (control) {
      control.addEventListener('click', function () {
        const action = control.dataset.removalAction;
        const row = rows.find(function (candidate) {
          return candidate.id === control.dataset.markId;
        });
        if (action === 'approve' && row) openConfirmation(row);
        if (action === 'decline' && row) submit('decline', row);
        if (action === 'cancel') {
          confirmation = null;
          render();
        }
        if (action === 'confirm' && confirmation) submit('approve', confirmation.row);
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
