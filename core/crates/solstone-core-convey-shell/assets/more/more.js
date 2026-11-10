// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

const MORE_STATE_FIELDS = [
  {"view":"activities","url":"/app/activities/api/index","path":["coverage"]},
  {"view":"transcripts","url":"/app/transcripts/api/index","path":["coverage"]},
  {"view":"map","url":"/app/map/api/index","path":["coverage"]},
  {"view":"body","url":"/app/body/api/index","path":["coverage"]},
  {"view":"speakers","url":"/app/speakers/api/index","path":["coverage"]},
  {"view":"entities","url":"/app/entities/api/journal/summary","path":["folder_counts","all"]},
  {"view":"import","url":"/app/import/api/list","path":["total"]},
  {"view":"network","url":"/app/network/api/clients","path":["clients"]},
  {"view":"thinking","url":"/app/thinking/api/state","path":["providers","active_lane","lane"]},
  {"view":"curation","url":"/app/curation/api/summary","path":["to_review"]},
  {"view":"agents","url":"/app/agents/api/state","path":["connections"]},
  {"view":"backup","url":"/app/backup/status","path":["success"]}
];

// More on the view contract: this script only registers. Nothing of the view
// runs when the script loads. The backup helper is the fragment's, loaded
// once for the page, and dispose does not remove it.
(function () {
  // A pin write outlives the view. This holds its outcome, not the page, so
  // the owner can leave while it is in flight. The next mount reads the shell
  // and paints that when this view did not.
  let pinWrite = null;

  const GROUPS = [
    ['keeps', 'what your journal keeps', 'your days, as your journal holds them'],
    ['shape', 'what you shape', 'fix and tidy what it keeps'],
    ['runs', 'how it runs', 'set it up and keep it running']
  ];
  const COVERAGE_VIEWS = ['activities', 'transcripts', 'map', 'body', 'speakers'];
  // lucide `pin`, inlined so the page doesn't fetch the whole icon catalogue for it.
  const PIN_ICON = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 17v5"/><path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1v-.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V7a1 1 0 0 1 1-1 2 2 0 0 0 0-4H8a2 2 0 0 0 0 4 1 1 0 0 1 1 1z"/></svg>';

  function escapeHtml(value) {
    return window.AppServices && window.AppServices.escapeHtml
      ? window.AppServices.escapeHtml(String(value))
      : String(value)
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
  }

  function leaf(body, path) {
    let current = body;
    for (const key of path) {
      if (current === null || current === undefined || typeof current !== 'object') return undefined;
      current = current[key];
    }
    return current;
  }

  function stateFor(view, body) {
    if (body === null || typeof body !== 'object') return null;
    if (COVERAGE_VIEWS.indexOf(view) !== -1) {
      if (!Object.prototype.hasOwnProperty.call(body, 'coverage')) return null;
      if (body.coverage === null) return { text: 'nothing yet', action: false };
      if (typeof body.coverage !== 'object' || typeof body.coverage.start !== 'string') return null;
      const since = window.JournalFormat && typeof window.JournalFormat.sinceDay === 'function'
        ? window.JournalFormat.sinceDay(body.coverage.start)
        : '';
      if (!since) return null;
      return { text: 'since ' + since, action: false };
    }
    if (view === 'entities') {
      const count = leaf(body, ['folder_counts', 'all']);
      if (typeof count !== 'number') return null;
      if (count > 0) return { text: count + ' kept', action: false };
      if (count === 0) return { text: 'nothing yet', action: false };
      return null;
    }
    if (view === 'import') {
      if (typeof body.total !== 'number') return null;
      if (body.total === 0) return { text: 'none yet', action: false };
      if (body.total === 1) return { text: '1 import', action: false };
      if (body.total > 1) return { text: body.total + ' imports', action: false };
      return null;
    }
    if (view === 'network') {
      if (!Array.isArray(body.clients)) return null;
      if (body.clients.length === 0) return { text: 'no devices yet', action: true };
      return null;
    }
    if (view === 'thinking') {
      const lane = leaf(body, ['providers', 'active_lane', 'lane']);
      if (lane === undefined || lane === 'none') return { text: 'not set up yet', action: true };
      return null;
    }
    if (view === 'curation') {
      const count = body.to_review;
      if (typeof count !== 'number' || !Number.isFinite(count) || count < 0) return null;
      if (count > 0) return { text: count + ' to review', action: true };
      if (count === 0) return { text: 'nothing to review', action: false };
      return null;
    }
    if (view === 'agents') {
      if (!Array.isArray(body.connections)) return null;
      const count = body.connections.length;
      if (count > 0) return { text: count + ' connected', action: false };
      if (count === 0) return { text: 'none yet', action: false };
      return null;
    }
    if (view === 'backup') {
      if (body.success !== true) return null;
      if (!window.BackupStatus || typeof window.BackupStatus.initialPanel !== 'function') return null;
      const panel = window.BackupStatus.initialPanel(body);
      if (panel === 'intro') return { text: 'not set up yet', action: true };
      return null;
    }
    return null;
  }

  function appsIn(shell, group) {
    return (shell.apps || [])
      .filter((app) => app.name !== 'more' && app.focus && app.focus.group === group)
      .sort((left, right) => left.focus.rank - right.focus.rank);
  }

  function pinnedSet(shell) {
    return new Set(Array.isArray(shell.pinned) ? shell.pinned : []);
  }

  function cardMarkup(app, pinned, pinIcon) {
    const icon = app.icon_svg || '';
    const source = app.source
      ? `<span class="more-card-source">${escapeHtml(app.source)}</span>`
      : '';
    const control = app.pinnable
      ? `<button type="button" class="more-pin more-pin-button" data-pin-view="${escapeHtml(app.name)}" aria-pressed="${pinned ? 'true' : 'false'}" aria-label="${escapeHtml(pinLabel(app, pinned))}" title="${escapeHtml(pinLabel(app, pinned))}">${pinIcon}</button>`
      : `<span class="more-pin more-pin-static" title="always in the side rail" aria-hidden="true">${pinIcon}</span>`;
    return (
      `<article class="more-card" data-app-name="${escapeHtml(app.name)}">` +
      `<a class="more-card-link" href="/app/${encodeURIComponent(app.name)}/">` +
      `<span class="more-card-top"><span class="more-card-icon app-chrome-icon" aria-hidden="true">${icon}</span>` +
      `<span class="more-card-label">${escapeHtml(app.label)}</span></span>` +
      `<span class="more-state" hidden></span>` +
      `<span class="more-card-desc">${escapeHtml(app.description || '')}</span>` +
      source +
      `</a>` +
      control +
      `</article>`
    );
  }

  function pinLabel(app, pinned) {
    return pinned
      ? `unpin ${app.label} from the side rail`
      : `pin ${app.label} to the side rail`;
  }

  function paint(root, shell, pinIcon) {
    const pins = pinnedSet(shell);
    const groups = GROUPS.map(([group, title, subtitle]) => {
      const cards = appsIn(shell, group).map((app) => cardMarkup(app, pins.has(app.name), pinIcon)).join('');
      return (
        `<section class="more-group" data-focus="${group}">` +
        `<h2>${title}</h2>` +
        `<p class="more-sub">${subtitle}</p>` +
        `<div class="more-grid">${cards}</div>` +
        `</section>`
      );
    }).join('');
    root.innerHTML =
      '<h1>more</h1>' +
      '<p class="more-lede">every view your journal has: what each one shows, and where its contents come from.</p>' +
      '<div class="more-live" role="status" aria-live="polite"></div>' +
      '<label class="visually-hidden" for="more-filter">filter views</label>' +
      '<input id="more-filter" class="more-filter" type="search" placeholder="type to filter views" autocomplete="off">' +
      groups +
      '<p class="more-empty" hidden>no views match that.</p>';
  }

  function syncPins(root, shell) {
    const pins = pinnedSet(shell);
    root.querySelectorAll('[data-pin-view]').forEach((button) => {
      const app = (shell.apps || []).find((item) => item.name === button.getAttribute('data-pin-view'));
      if (!app) return;
      const pinned = pins.has(app.name);
      button.setAttribute('aria-pressed', pinned ? 'true' : 'false');
      const label = pinLabel(app, pinned);
      button.setAttribute('aria-label', label);
      button.setAttribute('title', label);
    });
  }

  function applyPinned(root, shell, pinned) {
    if (window.solShellChrome && typeof window.solShellChrome.updatePinnedViews === 'function') {
      window.solShellChrome.updatePinnedViews(shell, pinned);
    } else {
      shell.pinned = Array.isArray(pinned) ? pinned.slice() : [];
    }
    if (root) syncPins(root, shell);
  }

  function showSaveError(root) {
    const live = root.querySelector('.more-live');
    if (live) live.textContent = "couldn't save that. try again.";
  }

  function clearSaveError(root) {
    const live = root.querySelector('.more-live');
    if (live) live.textContent = '';
  }

  function applyFilter(root) {
    const input = root.querySelector('#more-filter');
    const empty = root.querySelector('.more-empty');
    if (!input) return;
    const query = input.value.trim().toLowerCase();
    let visible = 0;
    root.querySelectorAll('.more-card').forEach((card) => {
      const name = card.getAttribute('data-app-name');
      const app = card._app;
      const keywords = app && app.keywords ? app.keywords.join(' ') : '';
      const source = app && app.source ? app.source : '';
      const description = app && app.description ? app.description : '';
      const label = app && app.label ? app.label : '';
      const blob = (label + ' ' + name + ' ' + description + ' ' + source + ' ' + keywords).toLowerCase();
      const matches = blob.includes(query);
      card.hidden = !matches;
      if (matches) visible += 1;
    });
    root.querySelectorAll('.more-group').forEach((group) => {
      group.hidden = !group.querySelector('.more-card:not([hidden])');
    });
    if (empty) empty.hidden = visible !== 0;
  }

  function rememberApps(root, shell) {
    root.querySelectorAll('.more-card').forEach((card) => {
      const name = card.getAttribute('data-app-name');
      card._app = (shell.apps || []).find((app) => app.name === name) || null;
    });
  }

  function here(ctx, root) {
    return !ctx.disposed && root && root.isConnected;
  }

  function installPins(root, shell, ctx) {
    const busy = new Set();
    root.querySelectorAll('[data-pin-view]').forEach((button) => {
      ctx.on(button, 'click', async () => {
        const name = button.getAttribute('data-pin-view');
        if (button.disabled || busy.has(name)) return;
        busy.add(name);
        button.disabled = true;
        const want = button.getAttribute('aria-pressed') !== 'true';
        const previous = Array.isArray(shell.pinned) ? shell.pinned.slice() : [];
        // Not the mount's signal: a write has no deadline, and leaving the
        // view must not abandon it.
        const request = window.apiJson('/api/shell/pins', {
          method: 'PUT',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ view: name, pinned: want }),
          noAuthRedirect: true
        });
        const settled = request.then(
          (response) => ({ ok: true, pinned: Array.isArray(response && response.pinned) ? response.pinned : null }),
          () => ({ ok: false })
        );
        pinWrite = settled;
        try {
          const response = await request;
          if (!response || !Array.isArray(response.pinned)) throw new Error('pins response');
          if (!here(ctx, root)) {
            if (window.solShellChrome && typeof window.solShellChrome.updatePinnedViews === 'function') {
              window.solShellChrome.updatePinnedViews(shell, response.pinned);
            } else {
              shell.pinned = response.pinned.slice();
            }
            if (pinWrite === settled) pinWrite = null;
            return;
          }
          applyPinned(root, shell, response.pinned);
          clearSaveError(root);
          if (pinWrite === settled) pinWrite = null;
        } catch (error) {
          if (!here(ctx, root)) return;
          try {
            // Shell scope: a view read would never settle once the owner has left.
            const fresh = await window.apiJson('/api/shell', { scope: 'shell', noAuthRedirect: true });
            if (!here(ctx, root)) return;
            if (!fresh || !Array.isArray(fresh.pinned)) throw error;
            applyPinned(root, shell, fresh.pinned);
          } catch (refetchError) {
            if (!here(ctx, root)) return;
            applyPinned(root, shell, previous);
          }
          if (!here(ctx, root)) return;
          showSaveError(root);
          if (pinWrite === settled) pinWrite = null;
        } finally {
          busy.delete(name);
          if (here(ctx, root)) button.disabled = false;
        }
      });
    });
  }

  function loadStates(root, ctx, timeout) {
    const jobs = MORE_STATE_FIELDS.map(async (field) => {
      const card = root.querySelector('.more-card[data-app-name="' + field.view + '"]');
      const slot = card && card.querySelector('.more-state');
      if (!slot) return;
      const controller = new AbortController();
      if (ctx.signal.aborted) controller.abort();
      else ctx.on(ctx.signal, 'abort', () => controller.abort());
      const timer = ctx.setTimeout(() => controller.abort(), timeout);
      try {
        const body = await window.apiJson(field.url, { noAuthRedirect: true, signal: controller.signal });
        if (!here(ctx, root)) return;
        const state = stateFor(field.view, body);
        if (!state) return;
        slot.textContent = state.text;
        slot.className = state.action ? 'more-state more-state-action' : 'more-state';
        slot.hidden = false;
      } catch (error) {
        if (here(ctx, root)) slot.hidden = true;
      } finally {
        ctx.clearTimeout(timer);
      }
    });
    return Promise.all(jobs);
  }

  function shellFromPage() {
    return new Promise((resolve) => {
      if (typeof window.whenShellReady !== 'function') resolve(null);
      else window.whenShellReady(resolve);
    });
  }

  async function drawMore(ctx, root, timeout) {
    const shell = await shellFromPage();
    if (!shell || ctx.disposed) return;

    if (pinWrite) {
      const pending = pinWrite;
      pinWrite = null;
      await pending;
      if (ctx.disposed) return;
      try {
        const fresh = await window.apiJson('/api/shell', { scope: 'shell', noAuthRedirect: true, signal: ctx.signal });
        if (ctx.disposed) return;
        if (fresh && Array.isArray(fresh.pinned)) {
          if (window.solShellChrome && typeof window.solShellChrome.updatePinnedViews === 'function') {
            window.solShellChrome.updatePinnedViews(shell, fresh.pinned);
          } else {
            shell.pinned = fresh.pinned.slice();
          }
        }
      } catch (_) {
        // The shell already in hand still paints. The write was not shown as failed.
      }
    }
    if (ctx.disposed) return;

    paint(root, shell, PIN_ICON);
    rememberApps(root, shell);
    const input = root.querySelector('#more-filter');
    if (input) ctx.on(input, 'input', () => applyFilter(root));
    installPins(root, shell, ctx);
    ctx.ready('content');
    await loadStates(root, ctx, timeout);
  }

  function mountMore(ctx) {
    // A test may set a shorter bound before mount. Production stays at 4s.
    const timeout = Number.isFinite(window.__moreStateTimeout) ? window.__moreStateTimeout : 4000;
    delete window.__moreStateTimeout;

    const root = document.getElementById('more-root');
    ctx.on(document, 'keydown', (event) => {
      if (!here(ctx, root)) return;
      const input = root.querySelector('#more-filter');
      if (!input || event.key !== 'Escape' || document.activeElement !== input) return;
      if (!input.value) return;
      input.value = '';
      applyFilter(root);
    });
    if (!root) throw new Error('more has no root');

    // The swap waits for mount to return, and the click paints that frame.
    // The cards are drawn on the next task, after that paint.
    let frame = 0;
    const painted = new Promise((resolve, reject) => {
      const start = () => {
        if (ctx.disposed) { resolve(); return; }
        Promise.resolve(drawMore(ctx, root, timeout)).then(resolve, reject);
      };
      if (typeof window.requestAnimationFrame !== 'function') {
        ctx.setTimeout(start, 0);
        return;
      }
      frame = window.requestAnimationFrame(() => {
        if (ctx.disposed) { resolve(); return; }
        ctx.setTimeout(start, 0);
      });
      ctx.on(ctx.signal, 'abort', () => {
        if (typeof window.cancelAnimationFrame === 'function') window.cancelAnimationFrame(frame);
        resolve();
      });
    });
    window.__morePainted = painted;
    painted.catch((error) => {
      if (!ctx.disposed) window.logError?.(error, { context: 'more' });
    });
  }

  window.SolViews.register('more', { mount: mountMore });
})();
