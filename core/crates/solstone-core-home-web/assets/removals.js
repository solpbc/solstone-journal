// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const COPY = Object.freeze({
    "card.heading": "the removal list",
    "card.subhead": "original audio and video marked for deletion, plus anything a removal left unfinished. nothing here is deleted until you say so.",
    "card.empty": "nothing is waiting on you.",
    "card.unavailable": "i couldn't load the list. i haven't deleted anything.",
    "card.total": "marked: {n} originals · {size}",
    "row.identity": "{date} · {stream}",
    "row.origin_policy": "your retention settings marked this. i rebuild that part of the list every day.",
    "row.origin_offload": "your backup copied these off your device. i can't check it's still there.",
    "row.what": "{n} originals · {size}",
    "row.kept": "nothing else goes with them.",
    "row.delete": "delete originals",
    "row.keep": "keep for now",
    "confirm.heading": "delete these originals?",
    "confirm.body_one": "this deletes {n} originals from {date} · {stream}. nothing else goes with them. it can't be undone.",
    "confirm.body_many": "this deletes {n} originals, {size}, across {rows} rows. nothing else goes with them. it can't be undone.",
    "confirm.go": "delete them",
    "confirm.cancel": "cancel",
    "done.deleted": "deleted {n} originals.",
    "done.partial": "deleted {n} originals. i couldn't delete {m} more.",
    "done.halted": "deleted {n} originals. i didn't get to {m} of the rows you picked.",
    "done.refused_none": "i couldn't delete them. nothing was deleted.",
    "done.refused_item": "{name}: {reason}",
    "done.unknown": "deleting stopped before i could tell what happened. i don't know what was deleted and what wasn't.",
    "done.kept_policy": "kept for now. it'll be back the next time i rebuild that part of the list.",
    "done.kept_offload": "kept for now. it'll be back after your next backup.",
    "failed.badge": "didn't finish",
    "failed.body": "a removal you started elsewhere stopped partway. the segment is still there, at {staged}. i can't finish it or undo it. it's yours to remove."
  });

  // PENDING: decline deletes nothing, ever, so done.refused_none is not truthful here.
  // These states and the unnamed refusal noun await authored owner copy and render nothing.
  const PENDING = Object.freeze({
    "declined.partial": null,
    "declined.refused": null,
    "refusal.item_unnamed": null
  });
  const MAX_SELECTION = 32;
  const LIST_URL = '/app/home/api/removals';
  const APPROVE_URL = '/app/home/api/approve';
  const DECLINE_URL = '/app/home/api/decline';

  let card = null;
  let mountedRoot = null;
  let rows = [];
  let listState = '';
  let selectedIds = new Set();
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
    if (stream === null) {
      return escapeHtml(row.day);
    }
    return copy("row.identity", { date: row.day, stream: stream });
  }

  function identity(text) {
    return '<p class="removals-card-identity" data-removal-identity>' + text + '</p>';
  }

  function copyWithoutDefaultStream(key, values) {
    return interpolate(COPY[key].replace(' · {stream}', ''), values);
  }

  function markedRow(row) {
    const selected = selectedIds.has(row.id) ? ' checked' : '';
    const origin = row.origin === 'offload' ? "row.origin_offload" : "row.origin_policy";
    const label = identityText(row);
    return '<article class="removals-card-row" data-removal-row data-mark-id="' + escapeHtml(row.id) + '">'
      + identity(label)
      + '<p class="removals-card-origin">' + copy(origin) + '</p>'
      + '<p class="removals-card-what">' + copy("row.what", { n: row.count, size: row.size }) + '</p>'
      + '<p class="removals-card-kept">' + copy("row.kept") + '</p>'
      + '<label><input type="checkbox" data-removal-select data-mark-id="' + escapeHtml(row.id) + '" aria-label="' + label + '"' + selected + '></label>'
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
      value.count += Number(row.count) || 0;
      value.bytes += Number(row.bytes) || 0;
      return value;
    }, { count: 0, bytes: 0 });
  }

  function confirmationHtml() {
    if (!confirmation) return '';
    const selected = confirmation.rows;
    const totals = selected.reduce(function (value, row) {
      value.count += Number(row.count) || 0;
      value.bytes += Number(row.bytes) || 0;
      return value;
    }, { count: 0, bytes: 0 });
    const only = selected[0];
    const stream = only && streamLabel(only.stream);
    const body = selected.length === 1
      ? (stream === null ? copyWithoutDefaultStream : copy)("confirm.body_one", {
        n: only.count,
        date: only.day,
        stream: stream
      })
      : copy("confirm.body_many", {
        n: totals.count,
        size: formatBytes(totals.bytes),
        rows: selected.length
      });
    return '<section class="removals-card-confirm" role="dialog">'
      + '<h3>' + copy("confirm.heading") + '</h3>'
      + '<p>' + body + '</p>'
      + '<button type="button" data-removal-action="confirm">' + copy("confirm.go") + '</button>'
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
        + copy("card.total", { n: totals.count, size: formatBytes(totals.bytes) })
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
        return rendered;
      }
      if (pending(item.state)) {
        // The unnamed fallback remains pending; never render a raw state code.
        return rendered;
      }
      return rendered;
    }, []);
  }

  function refusalList(items) {
    return items.length === 0 ? '' : '<ul>' + items.join('') + '</ul>';
  }

  function pending(state) {
    return Object.prototype.hasOwnProperty.call(PENDING, state);
  }

  function setOutcome(html) {
    outcomeHtml = html ? '<section class="removals-card-outcome">' + html + '</section>' : '';
  }

  function showOutcome(response, context) {
    const removed = Number(response.removed_count) || 0;
    const notRemoved = Number(response.not_removed_count) || 0;
    const items = refusalItems(response);
    switch (response.state) {
      case 'outcome.unknown':
        setOutcome('<p>' + copy("done.unknown") + '</p>');
        break;
      case 'approve.refused_before_start':
        setOutcome('<p>' + copy("done.refused_none") + '</p>');
        break;
      case 'approve.refused_after_start':
        setOutcome('<p>' + copy("done.refused_none") + '</p>' + refusalList(items));
        break;
      case 'approve.partial':
        setOutcome('<p>' + copy("done.partial", { n: removed, m: notRemoved }) + '</p>' + refusalList(items));
        break;
      case 'approve.deleted':
        setOutcome('<p>' + copy("done.deleted", { n: removed }) + '</p>');
        break;
      case 'approve.halted':
        setOutcome('<p>' + copy("done.halted", { n: removed, m: context.rows.length }) + '</p>');
        break;
      case 'declined.done': {
        const row = context.rows[0];
        setOutcome('<p>' + copy(row?.origin === 'offload' ? "done.kept_offload" : "done.kept_policy") + '</p>');
        break;
      }
      case 'tool.unavailable':
        setOutcome('<p>' + copy("done.refused_none") + '</p>');
        break;
      case 'request.too_large':
        setOutcome('<p>' + copy("done.refused_none") + '</p>');
        break;
      case 'approve.policy_keeps':
        setOutcome('<p>' + copy("done.refused_none") + '</p>');
        break;
      case 'declined.partial':
      case 'declined.refused':
        if (pending(response.state)) setOutcome('');
        break;
      case 'request.invalid':
        return response.state;
      default:
        setOutcome('');
    }
    render();
    return response.state;
  }

  function selectedRows() {
    return markedRows().filter(function (row) {
      return selectedIds.has(row.id);
    });
  }

  async function refresh() {
    try {
      const response = await request(LIST_URL);
      listState = response.state;
      rows = Array.isArray(response.removals) ? response.removals : [];
      selectedIds = new Set(selectedRows().map(function (row) { return row.id; }));
    } catch (_) {
      listState = 'list.register_unavailable';
      rows = [];
      selectedIds.clear();
    }
    render();
  }

  async function submit(action, selected) {
    const endpoint = action === 'approve' ? APPROVE_URL : DECLINE_URL;
    confirmation = null;
    try {
      const response = await request(endpoint, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ mark_ids: selected.map(function (row) { return row.id; }) })
      });
      await refresh();
      showOutcome(response, { rows: selected });
    } catch (_) {
      await refresh();
      showOutcome({ state: 'outcome.unknown' }, { rows: selected });
    }
  }

  function openConfirmation(row) {
    if (!selectedIds.has(row.id)) {
      if (selectedIds.size >= MAX_SELECTION) return;
      selectedIds.add(row.id);
    }
    const selected = selectedRows();
    if (selected.length === 0) return;
    confirmation = { rows: selected };
    render();
  }

  function wire() {
    if (!card) return;
    card.querySelectorAll('[data-removal-select]').forEach(function (control) {
      control.addEventListener('change', function () {
        const id = control.dataset.markId;
        if (control.checked) {
          if (selectedIds.size >= MAX_SELECTION) {
            control.checked = false;
            return;
          }
          selectedIds.add(id);
        } else {
          selectedIds.delete(id);
        }
      });
    });
    card.querySelectorAll('[data-removal-action]').forEach(function (control) {
      control.addEventListener('click', function () {
        const action = control.dataset.removalAction;
        const row = rows.find(function (candidate) {
          return candidate.id === control.dataset.markId;
        });
        if (action === 'approve' && row) openConfirmation(row);
        if (action === 'decline' && row) submit('decline', [row]);
        if (action === 'cancel') {
          confirmation = null;
          render();
        }
        if (action === 'confirm' && confirmation) submit('approve', confirmation.rows);
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
