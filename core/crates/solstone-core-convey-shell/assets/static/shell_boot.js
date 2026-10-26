// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const DAY_RE = /^\d{8}$/;

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
    return (shell.apps || []).find((app) => app.name === appName) || null;
  }

  const escapeHtml = window.AppServices.escapeHtml;

  function copyText(key, fallback) {
    return window.CONVEY_COPY?.[key] || fallback;
  }

  function applyChromeCopy() {
    const statusLink = document.getElementById('status-pane-console-link');
    const consoleLabel = copyText('CONSOLE_LINK_LABEL', 'system messages');
    if (statusLink) {
      statusLink.textContent = consoleLabel;
      statusLink.setAttribute('aria-label', consoleLabel);
    }

    const consoleHeading = copyText('CONSOLE_HEADING', 'system messages');
    const title = document.getElementById('diagnostic-console-title');
    if (title) title.textContent = consoleHeading;
    const tabs = document.querySelector('.diagnostic-console-tabs');
    if (tabs) tabs.setAttribute('aria-label', consoleHeading);

    const actions = {
      clear: copyText('CONSOLE_ACTION_CLEAR', 'clear'),
      'send-all': copyText('CONSOLE_ACTION_SEND_ALL', 'send to support')
    };
    for (const [action, label] of Object.entries(actions)) {
      const button = document.querySelector(`[data-diagnostic-action="${action}"]`);
      if (button) button.textContent = label;
    }
    const close = document.querySelector('[data-diagnostic-action="close"]');
    if (close) {
      close.setAttribute('aria-label', copyText('CONSOLE_ACTION_CLOSE', 'close'));
    }

    const reportingOff = document.querySelector('[data-diagnostic-reporting-off]');
    if (reportingOff) {
      reportingOff.textContent = copyText(
        'CONSOLE_REPORTING_OFF',
        'sending reports is off.'
      );
    }

    const tabLabels = {
      all: copyText('CONSOLE_TAB_ALL', 'all'),
      error: copyText('CONSOLE_TAB_ERRORS', 'errors'),
      warning: copyText('CONSOLE_TAB_WARNINGS', 'warnings'),
      info: copyText('CONSOLE_TAB_INFO', 'info')
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
      empty.textContent = copyText(
        'CONSOLE_EMPTY',
        'no system messages this session.'
      );
    }
  }

  function showShellError(retry, detail) {
    const target = document.getElementById('main-content') || document.body;
    if (window.SurfaceState) {
      target.innerHTML = window.SurfaceState.error({ retry: true, detail });
    } else {
      target.innerHTML =
        '<div class="surface-state surface-state--error" role="alert">' +
        '<h2 class="surface-state-heading">couldn\'t load this section</h2>' +
        '<p class="surface-state-desc">reload to try again.</p>' +
        '<button type="button" class="surface-state-retry">try again</button>' +
        '</div>';
    }
    const button = target.querySelector('.surface-state-retry');
    if (button) {
      button.addEventListener('click', retry, { once: true });
    }
  }

  function applyBodyState(app) {
    document.title = `${app.label} - journal`;
  }

  function appsByRank(shell, groupField, rankField, group) {
    return [...(shell.apps || [])]
      .filter((app) => app[groupField] === group)
      .sort((left, right) => left[rankField] - right[rankField]);
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
      '<span class="app-chrome-label">more</span></a>'
    );
  }

  function aboutLink(shell) {
    const version = String(shell.version || '').replace(/^v/, '');
    return `<a class="journal-about-link" href="/app/settings/#about" aria-label="about">${escapeHtml(version)}</a>`;
  }

  function renderAppRail(shell, currentAppName) {
    const rail = document.getElementById('app-rail');
    const currentApp = findApp(shell, currentAppName);
    if (!rail || !currentApp) return;
    const status = rail.querySelector('#status-instrument');
    if (status) document.body.appendChild(status);
    const home = findApp(shell, 'home');
    const pins = pinnedApps(shell);
    const management = appsByRank(shell, 'rail_group', 'rail_rank', 'management');
    const top = home ? [home, ...pins] : pins;
    rail.innerHTML = [
      ...top.map((app) => appLink(app, currentAppName, 'app-rail-link')),
      moreLink(shell, currentAppName, [...top, ...management], 'app-rail-link'),
      '<div class="app-rail-spacer" aria-hidden="true"></div>',
      '<div class="app-rail-divider" aria-hidden="true"></div>',
      ...management.map((app) => appLink(app, currentAppName, 'app-rail-link')),
      aboutLink(shell)
    ].join('');
  }

  function renderAppDock(shell, currentAppName) {
    const dock = document.getElementById('app-dock');
    const currentApp = findApp(shell, currentAppName);
    if (!dock || !currentApp) return;
    const status = dock.querySelector('#status-instrument');
    if (status) document.body.appendChild(status);
    const home = findApp(shell, 'home');
    const pins = pinnedApps(shell).slice(0, 2);
    const top = home ? [home, ...pins] : pins;
    dock.innerHTML = [
      ...top.map((app) => appLink(app, currentAppName, 'app-dock-link')),
      moreLink(shell, currentAppName, top, 'app-dock-link')
    ].join('');
  }

  function renderStatusInstrument() {
    const instrument = document.getElementById('status-instrument');
    if (!instrument) return;
    if (!instrument.querySelector('.status-icon')) instrument.innerHTML =
      '<a class="status-icon" href="/app/health/#healthSystemDetails" aria-label="connecting" title="connecting">' +
      '<img class="status-indicator status-indicator--connecting" src="/static/sol-status/mark-connecting.svg" width="22" height="22" alt="" aria-hidden="true">' +
      '<span id="quiet-notif-badge" class="quiet-notif-badge" style="display:none"></span>' +
      '</a>' +
      '<span id="status-live-region" role="status" class="visually-hidden"></span>';
    const mobile = window.matchMedia('(max-width: 700px)');
    const placeStatus = () => {
      const nav = document.getElementById(mobile.matches ? 'app-dock' : 'app-rail');
      const divider = nav.querySelector('.app-rail-divider');
      nav.insertBefore(instrument, divider || null);
    };
    placeStatus();
    if (!instrument.dataset.placementBound) {
      mobile.addEventListener('change', placeStatus);
      instrument.dataset.placementBound = 'true';
    }
  }

  function updatePinnedViews(shell, pinnedList) {
    shell.pinned = Array.isArray(pinnedList) ? pinnedList.slice() : [];
    const current = pathContext().appName;
    if (!current || !findApp(shell, current)) return;
    renderAppRail(shell, current);
    renderAppDock(shell, current);
    renderStatusInstrument();
  }

  if (!window.solShellChrome) {
    window.solShellChrome = {
      renderAppRail,
      renderAppDock,
      renderStatusInstrument,
      updatePinnedViews,
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
        throw new Error(`Request failed (HTTP ${response.status})`);
      }
      const code = await response.text();
      new Function(code)();
    } catch (err) {
      window.AppServices?.markBackgroundFailing?.(app.name, err);
      window.logError?.(err, { context: 'app-bg-register', app: app.name });
    }
  }

  async function boot() {
    const startedAt = Date.now();
    // The provenance table loads beside the shell API; the workspace mounts
    // once it settles or 1500 ms after boot began, whichever is first.
    const labelsPromise = (window.JournalFormat && typeof window.JournalFormat.beginStreamLabels === 'function')
      ? window.JournalFormat.beginStreamLabels()
      : Promise.resolve(false);

    const context = pathContext();
    applyChromeCopy();
    try {
      const shell = await window.apiJson('/api/shell');
      const app = findApp(shell, context.appName);
      if (!app) {
        throw new Error('Unknown app');
      }
      seedGlobals(shell);
      renderAppRail(shell, app.name);
      renderAppDock(shell, app.name);
      renderStatusInstrument();
      applyBodyState(app);
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

      await window.mountWorkspaceFragment(app.workspace_url, { appName: app.name });
    } catch (error) {
      if (window.logError) {
        window.logError(error, { context: 'shell-boot' });
      }
      showShellError(boot, error);
    }
  }

  boot();
})();
