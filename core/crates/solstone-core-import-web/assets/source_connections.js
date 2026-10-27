// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
(() => {
  'use strict';
  window.ImportSourceConnections?.dispose();
  const sources = { plaud: 'Plaud', obsidian: 'Obsidian', audio: 'audio folder' };
  const rows = new Map();
  let root = null;
  let disposed = false;
  const escape = value => window.AppServices.escapeHtml(String(value));
  const sourceUrl = source => `/api/journal/sources/${source}`;
  const scheduleUrl = source => `/api/journal/recurrences/source/${source}`;

  function retained(source, value) {
    try {
      const key = `journal-source-operation:${source}`;
      if (value !== undefined) sessionStorage.setItem(key, JSON.stringify(value));
      return JSON.parse(sessionStorage.getItem(key) || 'null');
    } catch (error) { return null; }
  }

  async function read(url, options) {
    const response = await fetch(url, options);
    if (response.redirected && response.url) {
      const destination = new URL(response.url, window.location.href);
      const current = new URL(window.location.href);
      if (destination.protocol === current.protocol && destination.hostname === current.hostname
          && destination.port === current.port && destination.pathname === '/init') {
        window.location.assign('/init');
        throw new Error('owner setup required');
      }
    }
    if (!response.ok) throw new Error('unavailable');
    return response.json();
  }

  function live(row) {
    return !disposed && root?.isConnected && rows.get(row.source) === row;
  }

  function status(row, message) {
    if (live(row)) row.node.querySelector('[data-source-status]').textContent = message;
  }

  function controls(row) {
    for (const button of row.node.querySelectorAll('button')) button.disabled = row.busy;
    const connected = row.source === 'plaud' ? row.available === true
      : row.connection?.remembered === true && row.connection.access_state === 'ready';
    row.node.querySelector('[data-source-sync]').disabled = row.busy || !row.readable || !connected || row.syncPending;
    for (const button of row.node.querySelectorAll('[data-source-change]')) {
      button.disabled = row.busy || !row.readable || !window.JournalOwnerSources;
    }
    for (const control of row.node.querySelectorAll('[data-source-schedule] input, [data-source-schedule] select, [data-source-schedule] button')) {
      control.disabled = row.busy || !row.schedule;
    }
  }

  function showConnection(row) {
    const details = row.node.querySelector('[data-source-connection]');
    if (row.source === 'plaud') {
      details.textContent = row.available ? 'ready to sync recordings into your journal.' : 'add your Plaud token in settings to sync recordings into your journal.';
      return;
    }
    const connection = row.connection;
    details.textContent = !connection ? 'no folder connected.'
      : connection.access_state === 'ready' ? `${connection.folder_name}: read access remembered for future syncs.`
      : `${connection.folder_name}: choose the folder again to restore read access.`;
    const actions = row.node.querySelector('[data-source-connection-actions]');
    actions.innerHTML = connection
      ? '<button type="button" class="import-secondary-btn" data-source-change="reauthorize">choose folder again</button> <button type="button" class="import-secondary-btn" data-source-change="replace">change folder</button> <button type="button" class="import-secondary-btn" data-source-change="forget">forget folder</button>'
      : '<button type="button" class="import-secondary-btn" data-source-change="begin">connect folder</button>';
  }

  function showSchedule(row) {
    const form = row.node.querySelector('[data-source-schedule]');
    const schedule = row.schedule;
    if (!schedule) return;
    const active = schedule.entries.filter(item => item.active);
    const cadences = [...new Set((active.length ? active : schedule.entries).map(item => item.entry.every))];
    const every = cadences.length === 1 ? cadences[0] : '';
    const options = [['', 'choose a cadence'], ['15m', 'every 15 minutes'], ['1h', 'every hour'], ['1d', 'every day']];
    if (every && !options.some(([value]) => value === every)) options.push([every, every]);
    form.querySelector('select').innerHTML = options.map(([value, label]) => `<option value="${escape(value)}"${value === every ? ' selected' : ''}>${escape(label)}</option>`).join('');
    form.querySelector('input').checked = schedule.enabled;
    form.querySelector('[data-source-schedule-state]').textContent = active.length
      ? `automatic sync is on: ${active.map(item => item.entry.every).join(', ')}.`
      : 'automatic sync is off.';
    form.querySelector('[data-source-conflicts]').textContent = schedule.blocked_names.length
      ? `custom schedules stay unchanged: ${schedule.blocked_names.join(', ')}.` : '';
  }

  async function refresh(row) {
    if (!live(row)) return;
    const revision = ++row.revision;
    row.readable = false;
    row.schedule = null;
    controls(row);
    const results = await Promise.allSettled([read(sourceUrl(row.source)), read(scheduleUrl(row.source))]);
    if (!live(row) || revision !== row.revision) return;
    const connection = results[0];
    if (connection.status === 'fulfilled' && connection.value.source === row.source) {
      const value = connection.value;
      if (row.source === 'plaud' && typeof value.available === 'boolean') {
        row.available = value.available;
        row.readable = true;
      } else if (row.source !== 'plaud' && (value.connection === null || validConnection(value.connection))) {
        row.connection = value.connection;
        row.readable = true;
      }
    }
    if (row.readable) showConnection(row);
    else status(row, 'could not read this source. check again before changing it.');
    const schedule = results[1];
    if (schedule.status === 'fulfilled' && schedule.value.source === row.source
        && typeof schedule.value.enabled === 'boolean' && Array.isArray(schedule.value.entries)
        && Array.isArray(schedule.value.blocked_names)
        && schedule.value.entries.every(item => typeof item.name === 'string' && typeof item.active === 'boolean' && typeof item.entry?.every === 'string')
        && schedule.value.blocked_names.every(name => typeof name === 'string')) {
      row.schedule = schedule.value;
      showSchedule(row);
    } else {
      row.node.querySelector('[data-source-schedule-state]').textContent = 'could not read the schedule. check again before changing it.';
    }
    controls(row);
    await checkSync(row);
    row.node.querySelector('[data-source-change-status]').textContent = retained(row.source)?.change
      ? 'could not confirm the folder change. check the current connection before trying again.' : '';
  }

  function validConnection(value) {
    return value && typeof value.source_identity === 'string' && value.source_identity.length > 0
      && typeof value.folder_name === 'string' && typeof value.remembered === 'boolean'
      && ['ready', 'lost', 'absent'].includes(value.access_state);
  }

  async function change(row, action, button) {
    if (row.busy || !row.readable || !window.JournalOwnerSources) return;
    const reference = crypto.randomUUID();
    row.busy = true;
    controls(row);
    try {
      const result = await window.JournalOwnerSources.change(row.source, action, row.connection?.source_identity || null, reference, () => {
        if (!live(row)) return false;
        const value = retained(row.source, { ...retained(row.source), change: reference });
        if (value?.change !== reference) return false;
        status(row, 'updating folder access…');
        return true;
      });
      if (!live(row)) return;
      if (['updated', 'cancelled', 'refused', 'busy'].includes(result.state) && result.permissions !== 'unknown'
          && retained(row.source)?.change === reference) {
        retained(row.source, { ...retained(row.source), change: null });
      }
      if (result.state === 'cancelled') return;
      await refresh(row);
      status(row, result.state === 'updated' && result.permissions !== 'unknown'
        ? action === 'forget' ? 'folder access forgotten. earlier imports stay in your journal.' : 'folder connection updated.'
        : 'could not confirm the folder change. check the current connection before trying again.');
    } catch (error) {
      if (live(row)) {
        await refresh(row);
        status(row, 'could not confirm the folder change. check the current connection before trying again.');
      }
    } finally {
      row.busy = false;
      if (live(row)) {
        controls(row);
        // The same card remains in place even when its action buttons changed.
        const focus = button.isConnected ? button : row.node.querySelector('[data-source-refresh]');
        focus.focus();
      }
    }
  }

  async function sync(row) {
    if (row.busy || row.syncPending || !row.readable) return;
    // Keep uncertainty before sending. A lost response is never an automatic
    // retry, including after reload; the pre-saved request ID can recover an accepted reference.
    const requestId = crypto.randomUUID();
    const value = retained(row.source, { ...retained(row.source), sync: { request_id: requestId } });
    if (value?.sync?.request_id !== requestId) { status(row, 'this page could not keep the request. reload the page before trying again.'); return; }
    row.busy = true;
    row.syncPending = true;
    controls(row);
    status(row, 'sending sync request…');
    try {
      const result = await read(`${sourceUrl(row.source)}/sync`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ request_id: requestId }) });
      if (result.accepted !== true || result.source !== row.source || result.request_id !== requestId || typeof result.reference !== 'string' || !result.reference) throw new Error('unconfirmed');
      const saved = retained(row.source, { ...retained(row.source), sync: { request_id: requestId, reference: result.reference } });
      if (saved?.sync?.reference !== result.reference) throw new Error('unconfirmed');
      await checkSync(row);
    } catch (error) {
      status(row, 'could not confirm this sync request. check import history before syncing again.');
    } finally {
      row.busy = false;
      if (live(row)) controls(row);
    }
  }

  async function checkSync(row) {
    clearTimeout(row.timer);
    row.timer = null;
    const saved = retained(row.source)?.sync;
    row.syncPending = !!saved;
    if (!saved) return;
    if (!saved.request_id) { status(row, 'could not confirm this sync request. check import history before syncing again.'); return; }
    try {
      const result = await read(`${sourceUrl(row.source)}/sync/${encodeURIComponent(saved.request_id)}`);
      if (!live(row) || result.source !== row.source || result.request_id !== saved.request_id
          || (saved.reference && result.reference !== saved.reference)
          || retained(row.source)?.sync?.request_id !== saved.request_id) return;
      if (result.state === 'finished' && result.terminal === true && typeof result.exit_status === 'string') {
        row.syncPending = false;
        retained(row.source, { ...retained(row.source), sync: null });
        status(row, result.exit_status === 'ok' ? 'source sync finished. media processing may continue.' : 'source sync did not finish. check import history for what was saved.');
      } else {
        if (result.terminal !== false || !['accepted', 'running', 'held', 'unknown'].includes(result.state)) throw new Error('unconfirmed');
        // A retained observation of unknown still proves neither acceptance nor
        // absence. Keep its ID for reads; only an explicit owner action may
        // start another sync, using a fresh request ID and the same source worker.
        if (result.state === 'unknown') row.syncPending = false;
        status(row, result.state === 'running' ? 'source sync is running.' : ['held', 'accepted'].includes(result.state) ? 'waiting to sync.' : 'could not confirm the sync result. check import history.');
        if (['accepted', 'running', 'held'].includes(result.state) && document.visibilityState !== 'hidden' && !root.hidden) {
          row.timer = setTimeout(() => { if (live(row)) checkSync(row); }, 5000);
        }
      }
    } catch (error) { status(row, 'could not read the sync result. check again before syncing again.'); }
    if (live(row)) controls(row);
  }

  async function schedule(row, method) {
    if (row.busy || !row.schedule) return;
    const form = row.node.querySelector('[data-source-schedule]');
    const every = form.querySelector('select').value;
    if (method === 'PUT' && !every) { status(row, 'choose how often to sync before saving the schedule.'); return; }
    row.busy = true;
    controls(row);
    let confirmed = false;
    try {
      await read(scheduleUrl(row.source), { method, ...(method === 'PUT' ? {
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ every, enabled: form.querySelector('input').checked }),
      } : {}) });
      confirmed = true;
    } catch (error) { /* Read actual state before offering another mutation. */ }
    finally {
      if (live(row)) {
        await refresh(row);
        status(row, confirmed ? 'schedule updated.' : 'could not confirm the schedule change. check the current schedule before trying again.');
        row.busy = false;
        controls(row);
      }
    }
  }

  function mount() {
    root = document.getElementById('importSyncSources');
    if (!root || !window.JournalOwnerFolders) return;
    root.hidden = !!window.location.hash && window.location.hash !== '#';
    root.innerHTML = `<h2>sync sources</h2>${Object.entries(sources).map(([source, name]) => `
      <section class="import-sync-source" data-sync-source="${source}" aria-labelledby="source-${source}-title">
        <h3 id="source-${source}-title">${name}</h3>
        <p data-source-connection>checking source…</p>
        ${source === 'plaud' ? '<p><a href="/app/settings/#apikeys">Plaud connection settings</a></p>' : '<p class="import-field-hint">connecting keeps read access to the selected folder for future syncs. forgetting a folder keeps earlier imports in your journal.</p><div class="import-action-row" data-source-connection-actions></div>'}
        <div class="import-action-row"><button type="button" class="import-primary-btn" data-source-sync disabled>sync now</button><button type="button" class="import-secondary-btn" data-source-refresh>check status</button></div>
        <p data-source-change-status role="status" aria-live="polite"></p><p data-source-status role="status" aria-live="polite"></p>
        <details><summary>automatic sync</summary><form data-source-schedule>
          <p data-source-schedule-state>checking schedule…</p><p data-source-conflicts></p>
          <label for="source-${source}-every">how often</label> <select id="source-${source}-every" disabled><option value="">choose a cadence</option></select>
          <label><input type="checkbox" disabled> automatic sync enabled</label>
          <div class="import-action-row"><button type="submit" class="import-secondary-btn" disabled>save schedule</button><button type="button" class="import-secondary-btn" data-source-remove disabled>remove schedule</button></div>
        </form></details>
      </section>`).join('')}`;
    for (const node of root.querySelectorAll('[data-sync-source]')) {
      const row = { source: node.dataset.syncSource, node, busy: false, readable: false, schedule: null, revision: 0, syncPending: !!retained(node.dataset.syncSource)?.sync };
      rows.set(row.source, row);
      node.addEventListener('click', event => {
        const button = event.target.closest('button');
        if (!button || button.disabled) return;
        if (button.hasAttribute('data-source-change')) change(row, button.dataset.sourceChange, button);
        else if (button.hasAttribute('data-source-sync')) sync(row);
        else if (button.hasAttribute('data-source-refresh')) refresh(row);
        else if (button.hasAttribute('data-source-remove')) schedule(row, 'DELETE');
      });
      node.querySelector('form').addEventListener('submit', event => { event.preventDefault(); schedule(row, 'PUT'); });
      refresh(row);
    }
  }

  function returned() {
    for (const row of rows.values()) {
      if (document.visibilityState === 'hidden' || root?.hidden) {
        clearTimeout(row.timer);
        row.timer = null;
      } else if (!row.busy) refresh(row);
    }
  }
  function leaving() {
    window.JournalOwnerSources?.cancel();
    for (const row of rows.values()) clearTimeout(row.timer);
  }
  function dispose() {
    disposed = true;
    leaving();
    window.removeEventListener('pageshow', returned);
    window.removeEventListener('hashchange', returned);
    window.removeEventListener('pagehide', leaving);
    document.removeEventListener('visibilitychange', returned);
    rows.clear();
  }
  window.ImportSourceConnections = { dispose, refresh: returned };
  window.addEventListener('pageshow', returned);
  window.addEventListener('hashchange', returned);
  window.addEventListener('pagehide', leaving);
  document.addEventListener('visibilitychange', returned);
  mount();
})();
