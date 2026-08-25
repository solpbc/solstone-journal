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

  function escapeHtml(value) {
    return String(value ?? '')
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

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
      clear: copyText('CONSOLE_ACTION_CLEAR', 'Clear'),
      'send-all': copyText('CONSOLE_ACTION_SEND_ALL', 'Send all')
    };
    for (const [action, label] of Object.entries(actions)) {
      const button = document.querySelector(`[data-diagnostic-action="${action}"]`);
      if (button) button.textContent = label;
    }
    const close = document.querySelector('[data-diagnostic-action="close"]');
    if (close) {
      close.setAttribute('aria-label', copyText('CONSOLE_ACTION_CLOSE', 'Close'));
    }

    const reportingOff = document.querySelector('[data-diagnostic-reporting-off]');
    if (reportingOff) {
      reportingOff.textContent = copyText(
        'CONSOLE_REPORTING_OFF',
        'I can show these messages, but sending reports is off.'
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
        "I haven't seen any system messages this session."
      );
    }
  }

  function showShellError(retry) {
    const target = document.getElementById('main-content') || document.body;
    if (window.SurfaceState) {
      target.innerHTML = window.SurfaceState.error({ retry: true });
    } else {
      target.innerHTML =
        '<div class="surface-state surface-state--error" role="alert">' +
        '<h2 class="surface-state-heading">Couldn\'t load this section</h2>' +
        '<p class="surface-state-desc">reload to try again.</p>' +
        '<button type="button" class="surface-state-retry">Try again</button>' +
        '</div>';
    }
    const button = target.querySelector('.surface-state-retry');
    if (button) {
      button.addEventListener('click', retry, { once: true });
    }
  }

  function applyBodyState(shell, app, day) {
    document.title = `${app.label} - journal`;
    const facetBar = document.querySelector('.facet-bar');
    if (facetBar) {
      facetBar.classList.toggle('facets-disabled', !app.facets_enabled);
    }

    const existing = document.getElementById('facet-theme');
    if (existing) existing.remove();
    if (!app.facets_enabled || !shell.selected_facet) return;
    const facet = (shell.facets || []).find((item) => item.name === shell.selected_facet);
    if (!facet || !facet.color) return;
    const style = document.createElement('style');
    style.id = 'facet-theme';
    style.textContent =
      ':root {' +
      `--facet-color: ${facet.color};` +
      `--facet-bg: ${facet.color}1a;` +
      `--facet-border: ${facet.color};` +
      '}';
    document.head.appendChild(style);
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
      `<a class="${className}${isCurrent ? ' is-current' : ''}" href="${escapeHtml(app.workspace_url)}"` +
      ` data-app-name="${escapeHtml(app.name)}"${isCurrent ? ' aria-current="page"' : ''}>` +
      `<span class="app-chrome-icon" aria-hidden="true">${icon}</span>` +
      `<span class="app-chrome-label">${label}</span>` +
      '</a>'
    );
  }

  function launcherToggle(currentApp, visibleApps) {
    const isCurrent = !visibleApps.some((app) => app.name === currentApp.name);
    const label = isCurrent ? `journal apps, current: ${currentApp.label}` : 'journal apps';
    return (
      `<button type="button" class="app-launcher-toggle${isCurrent ? ' is-current' : ''}"` +
      ` data-app-launcher-toggle aria-expanded="false" aria-controls="app-launcher"` +
      ` aria-haspopup="dialog" aria-label="${escapeHtml(label)}">` +
      '<span class="app-chrome-icon" aria-hidden="true">⌘</span>' +
      `<span class="app-chrome-label" aria-hidden="true">${escapeHtml(isCurrent ? currentApp.label : 'more')}</span>` +
      '</button>'
    );
  }

  function renderAppRail(shell, currentAppName) {
    const rail = document.getElementById('app-rail');
    const currentApp = findApp(shell, currentAppName);
    if (!rail || !currentApp) return;
    const primary = appsByRank(shell, 'rail_group', 'rail_rank', 'primary');
    const management = appsByRank(shell, 'rail_group', 'rail_rank', 'management');
    const railApps = [...primary, ...management];
    rail.innerHTML = [
      launcherToggle(currentApp, railApps),
      ...primary.map((app) => appLink(app, currentAppName, 'app-rail-link')),
      '<div class="app-rail-spacer" aria-hidden="true"></div>',
      '<div class="app-rail-divider" aria-hidden="true"></div>',
      ...management.map((app) => appLink(app, currentAppName, 'app-rail-link'))
    ].join('');
  }

  function renderAppDock(shell, currentAppName) {
    const dock = document.getElementById('app-dock');
    const currentApp = findApp(shell, currentAppName);
    if (!dock || !currentApp) return;
    const dockApps = appsByRank(shell, 'rail_group', 'rail_rank', 'primary').slice(0, 3);
    dock.innerHTML = [
      ...dockApps.map((app) => appLink(app, currentAppName, 'app-dock-link')),
      launcherToggle(currentApp, dockApps)
    ].join('');
  }

  function renderAppLauncher(shell, currentAppName) {
    const launcher = document.getElementById('app-launcher');
    if (!launcher) return;
    const groups = [
      ['your_journal', 'your journal'],
      ['understand', 'understand'],
      ['manage', 'manage']
    ];
    const groupMarkup = groups.map(([group, label]) => {
      const apps = appsByRank(shell, 'launcher_group', 'launcher_rank', group);
      const rows = apps.map((app) => {
        const labelText = escapeHtml(app.label);
        const isCurrent = app.name === currentAppName;
        return (
          `<li class="app-launcher-app" data-launcher-app data-label="${labelText.toLowerCase()}">` +
          `<a href="${escapeHtml(app.workspace_url)}"${isCurrent ? ' aria-current="page"' : ''}>${labelText}</a>` +
          '</li>'
        );
      }).join('');
      return (
        `<section class="app-launcher-group" data-launcher-group="${group}">` +
        `<h2>${label}</h2><ul>${rows}</ul></section>`
      );
    }).join('');

    launcher.innerHTML =
      '<div class="app-launcher-panel">' +
      '<header class="app-launcher-header">' +
      '<h1>journal apps</h1>' +
      '<button type="button" data-app-launcher-close aria-label="close">×</button>' +
      '</header>' +
      '<label for="app-launcher-filter">find a journal app</label>' +
      '<input id="app-launcher-filter" type="search" placeholder="find a journal app">' +
      `<div class="app-launcher-groups">${groupMarkup}</div>` +
      '<p class="app-launcher-empty" hidden>no journal apps match that search.</p>' +
      '</div>';

    const filter = document.getElementById('app-launcher-filter');
    const rows = Array.from(launcher.querySelectorAll('[data-launcher-app]'));
    const empty = launcher.querySelector('.app-launcher-empty');
    filter?.addEventListener('input', () => {
      const query = filter.value.trim().toLowerCase();
      let visible = 0;
      rows.forEach((row) => {
        const matches = row.dataset.label.includes(query);
        row.hidden = !matches;
        if (matches) visible += 1;
      });
      launcher.querySelectorAll('[data-launcher-group]').forEach((group) => {
        group.hidden = !group.querySelector('[data-launcher-app]:not([hidden])');
      });
      empty.hidden = visible !== 0;
    });
  }

  function renderFacetStrip(shell, app) {
    const facetStrip = document.getElementById('facet-strip');
    if (!facetStrip || !app.facets_enabled) return;
    facetStrip.removeAttribute('hidden');
  }

  function renderStatusInstrument() {
    const instrument = document.getElementById('status-instrument');
    if (!instrument || instrument.querySelector('.status-icon')) return;
    instrument.innerHTML =
      '<button class="status-icon" type="button" aria-expanded="false" aria-controls="status-pane" aria-label="connecting">' +
      '<img class="status-indicator status-indicator--connecting" src="/static/sol-status/mark-connecting.svg" width="22" height="22" alt="" aria-hidden="true">' +
      '<span id="quiet-notif-badge" class="quiet-notif-badge" style="display:none"></span>' +
      '</button>' +
      '<span class="status-label" aria-hidden="true">connecting</span>' +
      '<span id="status-live-region" role="status" class="visually-hidden"></span>';
  }

  function launcherIsOpen() {
    const launcher = document.getElementById('app-launcher');
    return Boolean(launcher && !launcher.hidden);
  }

  function setLauncherToggleExpanded(expanded) {
    document.querySelectorAll('[data-app-launcher-toggle]').forEach((toggle) => {
      toggle.setAttribute('aria-expanded', String(expanded));
    });
  }

  function openLauncher() {
    const launcher = document.getElementById('app-launcher');
    if (!launcher || launcherIsOpen()) return;
    launcher.inert = false;
    launcher.removeAttribute('inert');
    launcher.hidden = false;
    setLauncherToggleExpanded(true);
  }

  function closeLauncher() {
    const launcher = document.getElementById('app-launcher');
    if (!launcher || !launcherIsOpen()) return;
    launcher.hidden = true;
    launcher.inert = true;
    setLauncherToggleExpanded(false);
  }

  let launcherInteractionsInstalled = false;

  function installLauncherInteractions() {
    if (launcherInteractionsInstalled) return;
    const launcher = document.getElementById('app-launcher');
    if (!launcher) return;
    launcherInteractionsInstalled = true;

    document.addEventListener('click', (event) => {
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest('[data-app-launcher-toggle]')) {
        openLauncher();
        return;
      }
      if (target?.closest('[data-app-launcher-close]')) closeLauncher();
    });
    launcher.addEventListener('click', (event) => {
      if (event.target === launcher) {
        closeLauncher();
        return;
      }
      const target = event.target instanceof Element ? event.target : null;
      if (target?.closest('[data-launcher-app] a')) {
        closeLauncher();
      }
    });
    document.addEventListener('keydown', (event) => {
      if (event.key !== 'Escape' || !launcherIsOpen()) return;
      event.preventDefault();
      event.stopImmediatePropagation();
      closeLauncher();
    });
    window.addEventListener('presentation-mode:change', (event) => {
      if (event.detail?.on) closeLauncher();
    });
  }

  function seedGlobals(shell, app) {
    window.facetsData = shell.facets || [];
    window.selectedFacet = app.facets_enabled ? shell.selected_facet : null;
    window.appFacetCounts = {};
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
    const context = pathContext();
    applyChromeCopy();
    try {
      const shell = await window.apiJson('/api/shell');
      const app = findApp(shell, context.appName);
      if (!app) {
        throw new Error('Unknown app');
      }
      seedGlobals(shell, app);
      renderAppRail(shell, app.name);
      renderAppDock(shell, app.name);
      renderAppLauncher(shell, app.name);
      renderFacetStrip(shell, app);
      renderStatusInstrument();
      installLauncherInteractions();
      applyBodyState(shell, app, context.day);
      window.resolveSolShellReady(shell);

      for (const backgroundApp of shell.apps || []) {
        await loadBackground(backgroundApp);
      }

      if (!app.workspace_url) {
        throw new Error('Workspace unavailable');
      }
      await window.mountWorkspaceFragment(app.workspace_url, { appName: app.name });
    } catch (error) {
      if (window.logError) {
        window.logError(error, { context: 'shell-boot' });
      }
      showShellError(boot);
    }
  }

  boot();
})();
