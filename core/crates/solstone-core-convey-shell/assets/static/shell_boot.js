// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const DAY_RE = /^\d{8}$/;
  const badgesByView = new Map();

  function pathContext() {
    const parts = window.location.pathname.split('/');
    const isAppPath = parts[1] === 'app' && parts[2];
    const segment = isAppPath && parts[3] ? decodeURIComponent(parts[3]) : null;
    return {
      appName: isAppPath ? decodeURIComponent(parts[2]) : null,
      segment,
      day: segment && DAY_RE.test(segment) ? segment : null
    };
  }

  window.solPathContext = pathContext;

  function findApp(shell, appName) {
    return (shell?.apps || []).find((app) => app.name === appName) || null;
  }

  const escapeHtml = window.AppServices.escapeHtml;

  function applyChromeCopy() {
    const statusLink = document.getElementById('status-pane-console-link');
    const consoleLabel = window.CONVEY_COPY.CONSOLE_LINK_LABEL;
    if (statusLink) {
      statusLink.textContent = consoleLabel;
      statusLink.setAttribute('aria-label', consoleLabel);
    }

    const consoleHeading = window.CONVEY_COPY.CONSOLE_HEADING;
    const title = document.getElementById('diagnostic-console-title');
    if (title) title.textContent = consoleHeading;
    const tabs = document.querySelector('.diagnostic-console-tabs');
    if (tabs) tabs.setAttribute('aria-label', consoleHeading);

    const actions = {
      clear: window.CONVEY_COPY.CONSOLE_ACTION_CLEAR,
      'send-all': window.CONVEY_COPY.CONSOLE_ACTION_SEND_ALL
    };
    for (const [action, label] of Object.entries(actions)) {
      const button = document.querySelector(`[data-diagnostic-action="${action}"]`);
      if (button) button.textContent = label;
    }
    const close = document.querySelector('[data-diagnostic-action="close"]');
    if (close) {
      close.setAttribute('aria-label', window.CONVEY_COPY.CONSOLE_ACTION_CLOSE);
    }

    const reportingOff = document.querySelector('[data-diagnostic-reporting-off]');
    if (reportingOff) {
      reportingOff.textContent = window.CONVEY_COPY.CONSOLE_REPORTING_OFF;
    }

    const tabLabels = {
      all: window.CONVEY_COPY.CONSOLE_TAB_ALL,
      error: window.CONVEY_COPY.CONSOLE_TAB_ERRORS,
      warning: window.CONVEY_COPY.CONSOLE_TAB_WARNINGS,
      info: window.CONVEY_COPY.CONSOLE_TAB_INFO
    };
    for (const [filter, label] of Object.entries(tabLabels)) {
      const button = document.querySelector(`[data-diagnostic-filter="${filter}"]`);
      const count = button?.querySelector('[data-diagnostic-count]');
      if (button) {
        button.textContent = `${label} `;
        if (count) button.appendChild(count);
      }
    }

    const empty = document.getElementById('diagnostic-console-empty');
    if (empty) {
      empty.textContent = window.CONVEY_COPY.CONSOLE_EMPTY;
    }
  }

  function showShellError(retry, detail) {
    const target = document.getElementById('main-content') || document.body;
    if (window.SurfaceState) {
      target.innerHTML = window.SurfaceState.error({ retry: true, detail });
    } else {
      target.innerHTML =
        '<div class="surface-state surface-state--error" role="alert">' +
        `<h2 class="surface-state-heading">${window.JournalI18n.renderHtml('journal.window.shared.shell.boot.section.heading.couldn.t.load.this.section')}</h2>` +
        `<p class="surface-state-desc">${window.JournalI18n.renderHtml('journal.window.shared.shell.boot.visible.text.reload.to.try.again')}</p>` +
        `<button type="button" class="surface-state-retry">${window.JournalI18n.renderHtml('journal.window.shared.shell.boot.button.label.try.again')}</button>` +
        '</div>';
    }
    const button = target.querySelector('.surface-state-retry');
    if (button) {
      button.addEventListener('click', retry, { once: true });
    }
  }

  function applyBodyState(app) {
    document.title = window.JournalI18n.render('journal.window.chrome.view.status.document.title', [
      ['view', String(app.name || '')], ['fallback_label', String(app.label)]
    ]);
  }

  function appsByRank(shell, groupField, rankField, group) {
    return [...(shell.apps || [])]
      .filter((app) => app[groupField] === group)
      .sort((left, right) => left[rankField] - right[rankField]);
  }

  function normalizeBadgeSpec(spec) {
    if (!spec || typeof spec !== 'object') return null;
    const count = Number.isFinite(spec.count) && spec.count > 0 ? Math.floor(spec.count) : 0;
    const marker = typeof spec.marker === 'string' ? spec.marker.trim() : '';
    if (count <= 0 && !marker) return null;
    const tone = ['danger', 'warn', 'review', 'neutral'].includes(spec.tone) ? spec.tone : 'neutral';
    return { count, tone, marker };
  }

  function badgeSrText(appName, spec) {
    if (appName === 'home') {
      return spec.count > 0
        ? window.JournalI18n.render('journal.window.shell.badge.attention', [['count', String(spec.count)]])
        : window.JournalI18n.render('journal.window.shell.badge.uncertain');
    }
    return spec.count > 0
      ? window.JournalI18n.render('journal.window.shell.badge.review', [['count', String(spec.count)]])
      : '';
  }

  function badgeMarkup(appName) {
    const spec = badgesByView.get(appName);
    if (!spec) return '';
    const visible = spec.count > 0 ? (spec.count > 99 ? '99+' : String(spec.count)) : spec.marker;
    const sr = badgeSrText(appName, spec);
    return (
      `<span class="app-chrome-badge app-chrome-badge--${escapeHtml(spec.tone)}" data-chrome-badge="${escapeHtml(appName)}">` +
      `<span aria-hidden="true">${escapeHtml(visible)}</span>` +
      (sr ? `<span class="visually-hidden"> ${escapeHtml(sr)}</span>` : '') +
      '</span>'
    );
  }

  function setBadge(view, spec) {
    if (typeof view !== 'string' || !view) return;
    const normalized = normalizeBadgeSpec(spec);
    if (normalized) badgesByView.set(view, normalized);
    else badgesByView.delete(view);

    for (const containerId of ['app-rail', 'app-dock']) {
      const nav = document.getElementById(containerId);
      if (!nav) continue;
      const links = nav.querySelectorAll ? nav.querySelectorAll('[data-app-name]') : [];
      for (const link of links) {
        if (link.getAttribute('data-app-name') !== view) continue;
        const existing = link.querySelector('[data-chrome-badge]');
        if (!normalized) {
          existing?.remove();
          continue;
        }
        const wrapper = document.createElement('span');
        wrapper.innerHTML = badgeMarkup(view);
        const nextNode = wrapper.firstElementChild;
        if (!nextNode) continue;
        if (existing) existing.replaceWith(nextNode);
        else link.appendChild(nextNode);
      }
    }
  }

  function getBadge(view) {
    const found = badgesByView.get(view);
    return found ? Object.assign({}, found) : null;
  }

  function appLink(app, currentAppName, className) {
    const isCurrent = app.name === currentAppName;
    const icon = app.icon_svg || escapeHtml(app.icon);
    const label = escapeHtml(app.label);
    return (
      `<a class="${className}${isCurrent ? ' is-current' : ''}" href="${escapeHtml(`/app/${encodeURIComponent(app.name)}/`)}"` +
      ` data-app-name="${escapeHtml(app.name)}"${isCurrent ? ' aria-current="page"' : ''}>` +
      `<span class="app-chrome-icon" aria-hidden="true">${icon}</span>` +
      `<span class="app-chrome-label">${label}</span>` +
      badgeMarkup(app.name) +
      '</a>'
    );
  }

  function pinnedApps(shell) {
    const names = Array.isArray(shell.pinned) ? shell.pinned : [];
    const seen = new Set();
    const apps = [];
    for (const name of names) {
      if (name === 'home' || name === 'more' || seen.has(name)) continue;
      const app = findApp(shell, name);
      if (!app) continue;
      seen.add(name);
      apps.push(app);
    }
    return apps;
  }

  function moreLink(shell, currentAppName, surfaceApps, className) {
    const more = findApp(shell, 'more');
    const icon = more && more.icon_svg ? more.icon_svg : '';
    const onMore = currentAppName === 'more';
    const onSurface = surfaceApps.some((app) => app.name === currentAppName);
    const isCurrent = onMore || !onSurface;
    return (
      `<a class="${className}${isCurrent ? ' is-current' : ''}" href="/app/more/" data-app-name="more"${onMore ? ' aria-current="page"' : ''}>` +
      `<span class="app-chrome-icon" aria-hidden="true">${icon}</span>` +
      `<span class="app-chrome-label">${window.JournalI18n.renderHtml('journal.window.shared.shell.boot.visible.text.more')}</span>` +
      badgeMarkup('more') +
      '</a>'
    );
  }

  function aboutLink(shell) {
    const version = String(shell.version || '').replace(/^v/, '');
    return `<a class="journal-about-link" href="/app/settings/#about" aria-label="${window.JournalI18n.renderHtml('journal.window.shared.shell.boot.accessible.name.about')}">${escapeHtml(version)}</a>`;
  }

  function railGroupMarkup(group, labelId, itemsHtml) {
    const headingId = `app-rail-group-${group}`;
    return (
      `<div class="app-rail-group" role="group" data-rail-group="${group}" aria-labelledby="${headingId}">` +
      `<div class="app-rail-group-label" id="${headingId}">${window.JournalI18n.renderHtml(labelId)}</div>` +
      itemsHtml.join('') +
      '</div>'
    );
  }

  function renderAppRail(shell, currentAppName) {
    const rail = document.getElementById('app-rail');
    const currentApp = findApp(shell, currentAppName);
    if (!rail || !currentApp) return;
    const home = findApp(shell, 'home');
    const pins = pinnedApps(shell);
    const keepsPins = pins.filter((app) => !app.focus || app.focus.group === 'keeps');
    const shapePins = pins.filter((app) => app.focus && app.focus.group === 'shape');
    const runsPins = pins.filter((app) => app.focus && app.focus.group === 'runs');
    const management = appsByRank(shell, 'rail_group', 'rail_rank', 'management');
    const surfaceApps = [
      ...(home ? [home] : []),
      ...keepsPins,
      ...shapePins,
      ...runsPins,
      ...management
    ];
    const keepsItems = [
      ...(home ? [home] : []),
      ...keepsPins
    ].map((app) => appLink(app, currentAppName, 'app-rail-link'));
    const shapeItems = [
      ...shapePins.map((app) => appLink(app, currentAppName, 'app-rail-link')),
      moreLink(shell, currentAppName, surfaceApps, 'app-rail-link')
    ];
    const runsItems = [
      ...runsPins.map((app) => appLink(app, currentAppName, 'app-rail-link')),
      ...management.map((app) => appLink(app, currentAppName, 'app-rail-link')),
      aboutLink(shell)
    ];

    rail.innerHTML = [
      railGroupMarkup('keeps', 'journal.window.shell.rail.group.keeps', keepsItems),
      '<div class="app-rail-divider app-rail-divider--between" aria-hidden="true"></div>',
      railGroupMarkup('shape', 'journal.window.shell.rail.group.shape', shapeItems),
      '<div class="app-rail-spacer" aria-hidden="true"></div>',
      '<div class="app-rail-divider" aria-hidden="true"></div>',
      railGroupMarkup('runs', 'journal.window.shell.rail.group.runs', runsItems)
    ].join('');
  }

  function renderAppDock(shell, currentAppName) {
    const dock = document.getElementById('app-dock');
    const currentApp = findApp(shell, currentAppName);
    if (!dock || !currentApp) return;
    const home = findApp(shell, 'home');
    const pins = pinnedApps(shell).slice(0, 2);
    const top = home ? [home, ...pins] : pins;
    dock.innerHTML = [
      ...top.map((app) => appLink(app, currentAppName, 'app-dock-link')),
      moreLink(shell, currentAppName, top, 'app-dock-link')
    ].join('');
  }

  function renderStatusInstrument() {
    // Retained as a safe no-op for callers; Home and the Home rail/dock badge
    // now carry live status without a standalone chrome box.
  }

  // The view the rail marks as current, after a move inside the page.
  function markCurrent(name) {
    const shell = window.solShellData;
    if (!shell || !findApp(shell, name)) return;
    renderAppRail(shell, name);
    renderAppDock(shell, name);
  }

  function updatePinnedViews(shell, pinnedList) {
    shell.pinned = Array.isArray(pinnedList) ? pinnedList.slice() : [];
    const current = pathContext().appName;
    if (!current || !findApp(shell, current)) return;
    renderAppRail(shell, current);
    renderAppDock(shell, current);
  }

  function updatePinOffersSeen(shell, seenList) {
    if (!shell) return;
    shell.pin_offers_seen = Array.isArray(seenList) ? seenList.slice() : [];
  }

  function shouldShowPinOffer(view, shellArg) {
    const shell = shellArg || window.solShellData;
    if (!shell || typeof view !== 'string' || !view) return false;
    const app = findApp(shell, view);
    if (!app || !app.pinnable) return false;
    const pinned = Array.isArray(shell.pinned) ? shell.pinned : [];
    const seen = Array.isArray(shell.pin_offers_seen) ? shell.pin_offers_seen : [];
    return !pinned.includes(view) && !seen.includes(view);
  }

  function mountPinOffer(hostEl, opts) {
    if (!hostEl || !opts || typeof opts.view !== 'string') return false;
    const view = opts.view;
    const message = String(opts.message || '').trim();
    if (!message) {
      hostEl.replaceChildren?.();
      hostEl.hidden = true;
      return false;
    }
    if (!window.solShellData && typeof window.whenShellReady === 'function') {
      window.whenShellReady(() => {
        if (hostEl.isConnected === false) return;
        mountPinOffer(hostEl, opts);
      });
      return false;
    }
    const shell = window.solShellData;
    if (!shell) return false;
    const app = findApp(shell, view);
    if (!app || !app.pinnable) {
      hostEl.replaceChildren?.();
      hostEl.hidden = true;
      return false;
    }

    if (hostEl.dataset?.pinOfferState === 'pinned' && hostEl.dataset?.pinOfferView === view) {
      return true;
    }

    if (!shouldShowPinOffer(view, shell)) {
      if (hostEl.dataset) {
        delete hostEl.dataset.pinOfferState;
        delete hostEl.dataset.pinOfferView;
      }
      hostEl.replaceChildren?.();
      hostEl.hidden = true;
      return false;
    }

    const primaryLink = opts.primaryLink && opts.primaryLink.href && opts.primaryLink.label
      ? `<a class="pin-offer-link" href="${escapeHtml(opts.primaryLink.href)}">${escapeHtml(opts.primaryLink.label)}</a>`
      : '';
    const pinLabel = window.JournalI18n.render('journal.window.pin.offer.action.pin', [
      ['view', view],
      ['view_name', String(app.label || view)]
    ]);
    const notNowLabel = window.JournalI18n.render('journal.window.pin.offer.not.now');
    const pinnedText = window.JournalI18n.render('journal.window.pin.offer.pinned', [
      ['view', view],
      ['view_name', String(app.label || view)]
    ]);
    const undoLabel = window.JournalI18n.render('journal.window.pin.offer.undo');

    const closeOffer = () => {
      if (hostEl.dataset) {
        delete hostEl.dataset.pinOfferState;
        delete hostEl.dataset.pinOfferView;
      }
      hostEl.replaceChildren?.();
      hostEl.hidden = true;
    };

    const setBusy = (busy) => {
      const buttons = hostEl.querySelectorAll ? hostEl.querySelectorAll('button') : [];
      for (const btn of buttons) btn.disabled = busy;
    };

    const renderPinnedState = () => {
      if (hostEl.dataset) {
        hostEl.dataset.pinOfferView = view;
        hostEl.dataset.pinOfferState = 'pinned';
      }
      hostEl.hidden = false;
      hostEl.innerHTML =
        `<div class="pin-offer pin-offer--pinned" role="status" aria-live="polite" data-pin-offer="${escapeHtml(view)}">` +
        `<p class="pin-offer-text">${escapeHtml(pinnedText)}</p>` +
        '<div class="pin-offer-actions">' +
        primaryLink +
        `<button type="button" class="pin-offer-btn pin-offer-btn--quiet" data-pin-offer-action="undo">${escapeHtml(undoLabel)}</button>` +
        '</div>' +
        '</div>';
      const undoBtn = hostEl.querySelector('[data-pin-offer-action="undo"]');
      undoBtn?.addEventListener('click', async () => {
        setBusy(true);
        try {
          const reply = await window.apiJson('/api/shell/pins', {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ view, pinned: false })
          });
          if (reply && Array.isArray(reply.pinned)) {
            updatePinnedViews(shell, reply.pinned);
          }
          closeOffer();
        } catch (err) {
          setBusy(false);
          window.logError?.(err, { context: 'pin-offer-undo', view });
        }
      });
    };

    if (hostEl.dataset) {
      hostEl.dataset.pinOfferView = view;
      hostEl.dataset.pinOfferState = 'offer';
    }
    hostEl.hidden = false;
    hostEl.innerHTML =
      `<div class="pin-offer" data-pin-offer="${escapeHtml(view)}">` +
      `<p class="pin-offer-text">${escapeHtml(message)}</p>` +
      '<div class="pin-offer-actions">' +
      primaryLink +
      `<button type="button" class="pin-offer-btn pin-offer-btn--primary" data-pin-offer-action="pin">${escapeHtml(pinLabel)}</button>` +
      `<button type="button" class="pin-offer-btn pin-offer-btn--quiet" data-pin-offer-action="dismiss">${escapeHtml(notNowLabel)}</button>` +
      '</div>' +
      '</div>';

    const pinBtn = hostEl.querySelector('[data-pin-offer-action="pin"]');
    const dismissBtn = hostEl.querySelector('[data-pin-offer-action="dismiss"]');

    pinBtn?.addEventListener('click', async () => {
      setBusy(true);
      try {
        const [pinReply, seenReply] = await Promise.all([
          window.apiJson('/api/shell/pins', {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ view, pinned: true })
          }),
          window.apiJson('/api/shell/pin-offers', {
            method: 'PUT',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ view })
          }).catch(() => null)
        ]);
        if (pinReply && Array.isArray(pinReply.pinned)) {
          updatePinnedViews(shell, pinReply.pinned);
        }
        const currentSeen = Array.isArray(shell.pin_offers_seen) ? shell.pin_offers_seen : [];
        const nextSeen = seenReply && Array.isArray(seenReply.pin_offers_seen)
          ? seenReply.pin_offers_seen
          : (currentSeen.includes(view) ? currentSeen : [...currentSeen, view]);
        updatePinOffersSeen(shell, nextSeen);
        renderPinnedState();
      } catch (err) {
        setBusy(false);
        window.logError?.(err, { context: 'pin-offer-pin', view });
      }
    });

    dismissBtn?.addEventListener('click', async () => {
      setBusy(true);
      try {
        const reply = await window.apiJson('/api/shell/pin-offers', {
          method: 'PUT',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ view })
        });
        const currentSeen = Array.isArray(shell.pin_offers_seen) ? shell.pin_offers_seen : [];
        const nextSeen = reply && Array.isArray(reply.pin_offers_seen)
          ? reply.pin_offers_seen
          : (currentSeen.includes(view) ? currentSeen : [...currentSeen, view]);
        updatePinOffersSeen(shell, nextSeen);
        closeOffer();
      } catch (err) {
        setBusy(false);
        window.logError?.(err, { context: 'pin-offer-dismiss', view });
      }
    });

    return true;
  }

  if (!window.solShellChrome) {
    window.solShellChrome = {
      renderAppRail,
      renderAppDock,
      renderStatusInstrument,
      updatePinnedViews,
      updatePinOffersSeen,
      markCurrent,
      setBadge,
      getBadge,
      shouldShowPinOffer,
      mountPinOffer,
    };
  }

  function seedGlobals(shell) {
    if (window.JournalClock && shell.clock) {
      window.JournalClock.seed(shell.clock);
    }
    window.CONVEY_SETTINGS = {
      reportingEnabled: shell.settings?.reporting_enabled !== false
    };
  }

  async function loadBackground(app) {
    if (!app.background_url) return;
    try {
      const response = await fetch(app.background_url, { credentials: 'same-origin' });
      if (!response.ok) {
        throw new Error(window.JournalI18n.render('journal.window.shared.api.request.failed.http', [['http_status', String(response.status)]]));
      }
      const code = await response.text();
      new Function(code)();
    } catch (err) {
      window.AppServices?.markBackgroundFailing?.(app.name, err);
      window.logError?.(err, { context: 'app-bg-register', app: app.name });
    }
  }

  // The shell's state as the rest of the shell reads it: the app definitions
  // that ship in this bundle, for the apps this journal serves (named by
  // `/api/shell`), with the per-request part beside them.
  // A name this bundle has no definition for means the journal is now another
  // build than this page: the page reloads once (the same rule as a stale
  // asset), and meanwhile shows the apps it does know.
  const SHELL_BUILD_RELOAD_STAMP_KEY = 'solstone.shellBuildReloadAt';

  function localizeApp(app) {
    const text = (value) => typeof value === "string" && value.startsWith("journal.window.") ? window.JournalI18n.render(value) : value;
    const nav = app.date_nav;
    const dateNav = nav && {
      ...nav,
      unit: {
        one: text(nav.unit.one),
        other: text(nav.unit.other),
        none: text(nav.unit.none)
      }
    };
    return {
      ...app,
      label: text(app.label),
      description: text(app.description),
      source: text(app.source),
      date_nav: dateNav
    };
  }

  function shellFrom(state) {
    const registry = window.SOL_SHELL_REGISTRY || {};
    const definitions = new Map((registry.apps || []).map((app) => [app.name, app]));
    const names = Array.isArray(state && state.app_names) ? state.app_names : [];
    if (names.some((name) => !definitions.has(name)) && typeof window.reloadOnce === 'function') {
      window.reloadOnce(SHELL_BUILD_RELOAD_STAMP_KEY);
    }
    return Object.assign({}, state, {
      apps: names.map((name) => definitions.get(name)).filter(Boolean).map(localizeApp),
      settings: registry.settings || {}
    });
  }

  // The answer the page's head started is taken inside apiJson, by its address.
  async function readShell() {
    return shellFrom(await window.apiJson('/api/shell', { scope: 'shell' }));
  }

  // The rest of the shell is a second file. A fast view loads it when that
  // view is ready, so the wait for the view does not include it. A view
  // still reading loads it three seconds after this script starts, so a
  // slow first read does not hold the rest of the page. The flag latches
  // only once the address is known.
  const SHELL_REST_AFTER_MS = 3000;
  let shellRestRequested = false;
  function loadShellRest() {
    if (shellRestRequested) return;
    const holder = document.querySelector('script[data-shell-part="rest"]');
    const src = holder && holder.getAttribute('data-src');
    if (!src) return;
    shellRestRequested = true;
    const el = document.createElement('script');
    el.src = src;
    el.async = false;
    document.head.appendChild(el);
  }
  window.loadShellRest = loadShellRest;
  setTimeout(loadShellRest, SHELL_REST_AFTER_MS);

  async function boot() {
    performance.mark('shell:scripts-loaded');
    const startedAt = Date.now();
    // The provenance table loads beside the shell API; the workspace mounts
    // once it settles or 1500 ms after boot began, whichever is first.
    const labelsPromise = (window.JournalFormat && typeof window.JournalFormat.beginStreamLabels === 'function')
      ? window.JournalFormat.beginStreamLabels()
      : Promise.resolve(false);

    const context = pathContext();
    applyChromeCopy();
    try {
      const shell = await readShell();
      performance.mark('shell:answered');
      const app = findApp(shell, context.appName);
      if (!app) {
        throw new Error('Unknown app');
      }
      seedGlobals(shell);
      renderAppRail(shell, app.name);
      renderAppDock(shell, app.name);
      applyBodyState(app);
      await window.solRouter.openDay(app);
      window.resolveSolShellReady(shell);

      for (const backgroundApp of shell.apps || []) {
        await loadBackground(backgroundApp);
      }

      if (!app.workspace_url) {
        throw new Error('Workspace unavailable');
      }

      let raceTimerId = null;
      const raceTimeoutPromise = new Promise((resolve) => {
        const remaining = Math.max(0, 1500 - (Date.now() - startedAt));
        raceTimerId = setTimeout(() => {
          resolve('timeout');
        }, remaining);
      });

      const winner = await Promise.race([
        labelsPromise.then(() => 'labels', () => 'labels'),
        raceTimeoutPromise,
      ]);
      if (winner === 'labels' && raceTimerId !== null) {
        clearTimeout(raceTimerId);
      }
      if (winner === 'timeout' && window.JournalFormat
        && typeof window.JournalFormat.logStreamLabelProblem === 'function') {
        window.JournalFormat.logStreamLabelProblem('slow');
      }

      try {
        await window.mountWorkspaceFragment(app.workspace_url, { appName: app.name });
      } finally {
        // A pilot view keeps the page from here on, even one whose first
        // load failed: it reads again when the live stream connects.
        window.solRouter?.start(app.name);
      }
    } catch (error) {
      loadShellRest();
      if (window.logError) {
        window.logError(error, { context: 'shell-boot' });
      }
      showShellError(boot, error);
    }
  }

  boot();
})();
