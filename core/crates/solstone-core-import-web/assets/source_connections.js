// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
// The import page's sync sources, drawn by import's mount:
// `ImportSourceConnections.mount(ctx)` draws them into the page's
// #importSyncSources, and everything it adds outside that goes through `ctx`
// and goes with the view.
(() => {
  'use strict';
  const sources = { plaud: 'Plaud', audio: window.JournalI18n.render('journal.window.import.import.source.control.audio.folder') };
  window.ImportSourceConnections = Object.freeze({ mount: mountSourceConnections });

  function mountSourceConnections(ctx) {
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

  // A read is the view's (cut when it goes). A write the owner started
  // finishes, and its answer is handled only while the view is up; none
  // starts once it has gone.
  async function read(url, options) {
    const write = String(options?.method || 'GET').toUpperCase() !== 'GET';
    if (write && disposed) return window.SolStore.never();
    const response = await (write ? window.fetch(url, options) : ctx.fetch(url, options));
    if (disposed) return window.SolStore.never();
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

  // A draw writes only what differs from what the page already shows, so the
  // sync check that finds nothing new leaves the page as it is. The same
  // setters are in import's own script and import_detail.js.
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
  function writeDisabled(el, disabled) {
    if (el && el.disabled !== !!disabled) el.disabled = !!disabled;
  }

  // The control whose write is out keeps focus: busy, never turned off. The
  // row's other controls are turned off while it's out, as before.
  function busy(el, on) {
    writeAttr(el, 'aria-disabled', on ? 'true' : null);
    writeAttr(el, 'aria-busy', on ? 'true' : null);
  }

  // After a press, a control that's now turned off or gone hands focus to the
  // row's "check status".
  function settle(row, held) {
    if (!held || !live(row) || document.activeElement !== held && document.activeElement !== document.body) return;
    if (held.isConnected && !held.disabled) return;
    if (document.activeElement === held) held.blur?.();
    row.node.querySelector('[data-source-refresh]')?.focus();
  }

  function status(row, message) {
    if (live(row)) writeText(row.node.querySelector('[data-source-status]'), message);
  }

  // Each control's state is worked out first and written once.
  function controls(row) {
    const off = new Map();
    for (const button of row.node.querySelectorAll('button')) off.set(button, row.busy);
    const connected = row.source === 'plaud' ? row.available === true
      : row.connection?.remembered === true && row.connection.access_state === 'ready';
    off.set(row.node.querySelector('[data-source-sync]'), row.busy || !row.readable || !connected || row.syncPending);
    for (const button of row.node.querySelectorAll('[data-source-change]')) {
      off.set(button, row.busy || !row.readable || !window.JournalOwnerSources);
    }
    for (const control of row.node.querySelectorAll('[data-source-schedule] input, [data-source-schedule] select, [data-source-schedule] button')) {
      off.set(control, row.busy || !row.schedule);
    }
    off.forEach((disabled, control) => {
      const pressed = row.busy && row.pressed === control;
      busy(control, pressed);
      writeDisabled(control, pressed ? false : disabled);
    });
  }

  function showConnection(row) {
    const details = row.node.querySelector('[data-source-connection]');
    if (row.source === 'plaud') {
      details.textContent = row.available ? window.JournalI18n.render('journal.window.import.visible.text.ready.to.sync.recordings.into.your') : window.JournalI18n.render('journal.window.import.visible.text.add.your.plaud.token.in.settings');
      return;
    }
    const connection = row.connection;
    details.textContent = !connection ? window.JournalI18n.render('journal.window.import.visible.text.no.folder.connected')
      : connection.access_state === 'ready' ? window.JournalI18n.renderEnglish('journal.window.import.folder.access.remembered', [['folder', String(connection.folder_name)]])
      : window.JournalI18n.render('journal.window.import.folder.access.repeat', [['folder', String(connection.folder_name)]]);
    const actions = row.node.querySelector('[data-source-connection-actions]');
    actions.innerHTML = connection
      ? `<button type="button" class="import-secondary-btn" data-source-change="reauthorize">${window.JournalI18n.renderHtml('journal.window.import.button.label.choose.folder.again')}</button> <button type="button" class="import-secondary-btn" data-source-change="replace">${window.JournalI18n.renderHtml('journal.window.import.button.label.change.folder')}</button> <button type="button" class="import-secondary-btn" data-source-change="forget">${window.JournalI18n.renderHtml('journal.window.import.button.label.forget.folder')}</button>`
      : `<button type="button" class="import-secondary-btn" data-source-change="begin">${window.JournalI18n.renderHtml('journal.window.import.button.label.connect.folder')}</button>`;
  }

  function showSchedule(row) {
    const form = row.node.querySelector('[data-source-schedule]');
    const schedule = row.schedule;
    if (!schedule) return;
    const active = schedule.entries.filter(item => item.active);
    const cadences = [...new Set((active.length ? active : schedule.entries).map(item => item.entry.every))];
    const every = cadences.length === 1 ? cadences[0] : '';
    const options = [['', window.JournalI18n.render('journal.window.import.import.source.control.choose.a.cadence')], ['15m', window.JournalI18n.render('journal.window.import.import.source.control.every.minutes')], ['1h', window.JournalI18n.render('journal.window.import.import.source.control.every.hour')], ['1d', window.JournalI18n.render('journal.window.import.import.source.control.every.day')]];
    if (every && !options.some(([value]) => value === every)) options.push([every, every]);
    form.querySelector('select').innerHTML = options.map(([value, label]) => `<option value="${escape(value)}"${value === every ? ' selected' : ''}>${escape(label)}</option>`).join('');
    form.querySelector('input').checked = schedule.enabled;
    form.querySelector('[data-source-schedule-state]').textContent = active.length
      ? window.JournalI18n.render('journal.window.import.folder.schedule.active', [['cadences', String(active.map(item => item.entry.every).join(', '))]])
      : window.JournalI18n.render('journal.window.import.visible.text.automatic.sync.is.off');
    form.querySelector('[data-source-conflicts]').textContent = schedule.blocked_names.length
      ? window.JournalI18n.render('journal.window.import.folder.schedule.conflicts', [['schedule_names', String(schedule.blocked_names.join(', '))]]) : '';
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
    row.node.querySelector('[data-source-read-status]').textContent = row.readable
      ? '' : window.JournalI18n.render('journal.window.import.visible.text.could.not.read.this.source.check');
    const schedule = results[1];
    if (schedule.status === 'fulfilled' && schedule.value.source === row.source
        && typeof schedule.value.enabled === 'boolean' && Array.isArray(schedule.value.entries)
        && Array.isArray(schedule.value.blocked_names)
        && schedule.value.entries.every(item => typeof item.name === 'string' && typeof item.active === 'boolean' && typeof item.entry?.every === 'string')
        && schedule.value.blocked_names.every(name => typeof name === 'string')) {
      row.schedule = schedule.value;
      showSchedule(row);
    } else {
      row.node.querySelector('[data-source-schedule-state]').textContent = window.JournalI18n.render('journal.window.import.visible.text.could.not.read.the.schedule.check');
    }
    controls(row);
    await checkSync(row);
    row.node.querySelector('[data-source-change-status]').textContent = retained(row.source)?.change
      ? window.JournalI18n.render('journal.window.import.visible.text.could.not.confirm.the.folder.change') : '';
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
    row.pressed = button || null;
    controls(row);
    try {
      const result = await window.JournalOwnerSources.change(row.source, action, row.connection?.source_identity || null, reference, () => {
        if (!live(row)) return false;
        const value = retained(row.source, { ...retained(row.source), change: reference });
        if (value?.change !== reference) return false;
        status(row, window.JournalI18n.render('journal.window.import.import.status.or.action.updating.folder.access'));
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
        ? action === 'forget' ? window.JournalI18n.render('journal.window.import.import.status.or.action.folder.access.forgotten.earlier.imports.stay') : window.JournalI18n.render('journal.window.import.import.status.or.action.folder.connection.updated')
        : window.JournalI18n.render('journal.window.import.import.status.or.action.could.not.confirm.the.folder.change'));
    } catch (error) {
      if (live(row)) {
        await refresh(row);
        status(row, window.JournalI18n.render('journal.window.import.import.status.or.action.could.not.confirm.the.folder.change'));
      }
    } finally {
      row.busy = false;
      row.pressed = null;
      if (live(row)) {
        controls(row);
        // The same card remains in place even when its action buttons changed.
        const focus = button.isConnected ? button : row.node.querySelector('[data-source-refresh]');
        focus.focus();
      }
    }
  }

  async function sync(row, button = null) {
    if (row.busy || row.syncPending || !row.readable) return;
    // Keep uncertainty before sending. A lost response is never an automatic
    // retry, including after reload; the pre-saved request ID can recover an accepted reference.
    const requestId = crypto.randomUUID();
    const value = retained(row.source, { ...retained(row.source), sync: { request_id: requestId } });
    if (value?.sync?.request_id !== requestId) { status(row, window.JournalI18n.render('journal.window.import.import.status.or.action.this.page.could.not.keep.the')); return; }
    row.busy = true;
    row.pressed = button;
    row.syncPending = true;
    controls(row);
    status(row, window.JournalI18n.render('journal.window.import.import.status.or.action.sending.sync.request'));
    try {
      const result = await read(`${sourceUrl(row.source)}/sync`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ request_id: requestId }) });
      if (result.accepted !== true || result.source !== row.source || result.request_id !== requestId || typeof result.reference !== 'string' || !result.reference) throw new Error('unconfirmed');
      const saved = retained(row.source, { ...retained(row.source), sync: { request_id: requestId, reference: result.reference } });
      if (saved?.sync?.reference !== result.reference) throw new Error('unconfirmed');
      await checkSync(row);
    } catch (error) {
      status(row, window.JournalI18n.render('journal.window.import.import.status.or.action.could.not.confirm.this.sync.request'));
    } finally {
      row.busy = false;
      row.pressed = null;
      if (live(row)) {
        controls(row);
        // While the sync runs, "sync now" is unavailable.
        settle(row, button);
      }
    }
  }

  async function checkSync(row) {
    ctx.clearTimeout(row.timer);
    row.timer = null;
    const saved = retained(row.source)?.sync;
    row.syncPending = !!saved;
    if (!saved) return;
    if (!saved.request_id) { status(row, window.JournalI18n.render('journal.window.import.import.status.or.action.could.not.confirm.this.sync.request')); return; }
    try {
      const result = await read(`${sourceUrl(row.source)}/sync/${encodeURIComponent(saved.request_id)}`);
      if (!live(row) || result.source !== row.source || result.request_id !== saved.request_id
          || (saved.reference && result.reference !== saved.reference)
          || retained(row.source)?.sync?.request_id !== saved.request_id) return;
      if (result.state === 'finished' && result.terminal === true && typeof result.exit_status === 'string') {
        row.syncPending = false;
        retained(row.source, { ...retained(row.source), sync: null });
        status(row, result.exit_status === 'ok' ? window.JournalI18n.render('journal.window.import.import.status.or.action.source.sync.finished.media.processing.may') : window.JournalI18n.render('journal.window.import.import.status.or.action.source.sync.did.not.finish.check'));
      } else {
        if (result.terminal !== false || !['accepted', 'running', 'held', 'unknown'].includes(result.state)) throw new Error('unconfirmed');
        // A retained observation of unknown still proves neither acceptance nor
        // absence. Keep its ID for reads; only an explicit owner action may
        // start another sync, using a fresh request ID and the same source worker.
        if (result.state === 'unknown') row.syncPending = false;
        status(row, result.state === 'running' ? window.JournalI18n.render('journal.window.import.import.status.or.action.source.sync.is.running') : ['held', 'accepted'].includes(result.state) ? window.JournalI18n.render('journal.window.import.import.status.or.action.waiting.to.sync') : window.JournalI18n.render('journal.window.import.import.status.or.action.could.not.confirm.the.sync.result'));
        if (['accepted', 'running', 'held'].includes(result.state) && document.visibilityState !== 'hidden' && !root.hidden) {
          row.timer = ctx.setTimeout(() => { if (live(row)) checkSync(row); }, 5000);
        }
      }
    } catch (error) { status(row, window.JournalI18n.render('journal.window.import.import.status.or.action.could.not.read.the.sync.result')); }
    if (live(row)) controls(row);
  }

  async function schedule(row, method, button = null) {
    if (row.busy || !row.schedule) return;
    const form = row.node.querySelector('[data-source-schedule]');
    const every = form.querySelector('select').value;
    if (method === 'PUT' && !every) { status(row, window.JournalI18n.render('journal.window.import.import.status.or.action.choose.how.often.to.sync.before')); return; }
    row.busy = true;
    row.pressed = button;
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
        status(row, confirmed ? window.JournalI18n.render('journal.window.import.import.status.or.action.schedule.updated') : window.JournalI18n.render('journal.window.import.import.status.or.action.could.not.confirm.the.schedule.change'));
        row.busy = false;
        row.pressed = null;
        controls(row);
        settle(row, button);
      }
    }
  }

  function mount() {
    root = document.getElementById('importSyncSources');
    if (!root || !window.JournalOwnerFolders) return;
    root.hidden = !!window.location.hash && window.location.hash !== '#';
    root.innerHTML = `<h2>${window.JournalI18n.renderHtml('journal.window.import.section.heading.sync.sources')}</h2>${Object.entries(sources).map(([source, name]) => `
      <section class="import-sync-source" data-sync-source="${source}" aria-labelledby="source-${source}-title">
        <h3 id="source-${source}-title">${escape(name)}</h3>
        <p data-source-connection>${window.JournalI18n.renderHtml('journal.window.import.visible.text.checking.source')}</p>
        ${source === 'plaud' ? `<p><a href="/app/settings/#apikeys">${window.JournalI18n.renderHtml('journal.window.import.link.label.plaud.connection.settings')}</a></p>` : `<p class="import-field-hint">${window.JournalI18n.renderEnglishHtml('journal.window.import.visible.text.connecting.keeps.read.access.to.the')}</p><div class="import-action-row" data-source-connection-actions></div>`}
        <div class="import-action-row"><button type="button" class="import-primary-btn" data-source-sync disabled>${window.JournalI18n.renderHtml('journal.window.import.button.label.sync.now')}</button><button type="button" class="import-secondary-btn" data-source-refresh>${window.JournalI18n.renderHtml('journal.window.import.button.label.check.status')}</button></div>
        <p data-source-read-status role="status" aria-live="polite"></p><p data-source-change-status role="status" aria-live="polite"></p><p data-source-status role="status" aria-live="polite"></p>
        <details><summary>${window.JournalI18n.renderHtml('journal.window.import.disclosure.label.automatic.sync')}</summary><form data-source-schedule>
          <p data-source-schedule-state>${window.JournalI18n.renderHtml('journal.window.import.visible.text.checking.schedule')}</p><p data-source-conflicts></p>
          <label for="source-${source}-every">${window.JournalI18n.renderHtml('journal.window.import.field.label.how.often')}</label> <select id="source-${source}-every" disabled><option value="">${window.JournalI18n.renderHtml('journal.window.import.option.label.choose.a.cadence')}</option></select>
          <label><input type="checkbox" disabled> ${window.JournalI18n.renderHtml('journal.window.import.field.label.automatic.sync.enabled')}</label>
          <div class="import-action-row"><button type="submit" class="import-secondary-btn" disabled>${window.JournalI18n.renderHtml('journal.window.import.button.label.save.schedule')}</button><button type="button" class="import-secondary-btn" data-source-remove disabled>${window.JournalI18n.renderHtml('journal.window.import.button.label.remove.schedule')}</button></div>
        </form></details>
      </section>`).join('')}`;
    for (const node of root.querySelectorAll('[data-sync-source]')) {
      const row = { source: node.dataset.syncSource, node, busy: false, readable: false, schedule: null, revision: 0, syncPending: !!retained(node.dataset.syncSource)?.sync };
      rows.set(row.source, row);
      node.addEventListener('click', event => {
        const button = event.target.closest('button');
        if (!button || button.disabled) return;
        if (button.hasAttribute('data-source-change')) change(row, button.dataset.sourceChange, button);
        else if (button.hasAttribute('data-source-sync')) sync(row, button);
        else if (button.hasAttribute('data-source-refresh')) refresh(row);
        else if (button.hasAttribute('data-source-remove')) schedule(row, 'DELETE', button);
      });
      node.querySelector('form').addEventListener('submit', event => {
        event.preventDefault();
        schedule(row, 'PUT', event.submitter || node.querySelector('[data-source-schedule] button[type="submit"]'));
      });
      refresh(row);
    }
  }

  function returned() {
    for (const row of rows.values()) {
      if (document.visibilityState === 'hidden' || root?.hidden) {
        ctx.clearTimeout(row.timer);
        row.timer = null;
      } else if (!row.busy) refresh(row);
    }
  }
  function leaving() {
    window.JournalOwnerSources?.cancel();
    for (const row of rows.values()) ctx.clearTimeout(row.timer);
  }
  function dispose() {
    disposed = true;
    leaving();
    rows.clear();
    root = null;
  }
  ctx.onDispose(dispose);
  ctx.on(window, 'pageshow', returned);
  ctx.on(window, 'hashchange', returned);
  ctx.on(window, 'pagehide', leaving);
  ctx.on(document, 'visibilitychange', returned);
  mount();
  }
})();
