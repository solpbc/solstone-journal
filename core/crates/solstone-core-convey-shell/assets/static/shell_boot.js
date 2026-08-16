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
    document.body.classList.toggle('has-app-bar', !!app.app_bar);
    const appBar = document.getElementById('appBar');
    if (appBar) {
      appBar.hidden = !app.app_bar;
    }
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

  function renderMenu(shell, currentAppName) {
    const menu = document.querySelector('.menu-bar .menu-items');
    if (!menu) return;
    const apps = shell.apps || [];
    let lastStarredIndex = -1;
    apps.forEach((app, index) => {
      if (app.starred) lastStarredIndex = index;
    });
    menu.innerHTML = apps
      .map((app, index) => {
        const isCurrent = app.name === currentAppName;
        const isLastStarred = index === lastStarredIndex && lastStarredIndex >= 0;
        const icon = app.icon_svg || escapeHtml(app.icon);
        const label = escapeHtml(app.label);
        return (
          `<li class="menu-item${isCurrent ? ' current' : ''}${isLastStarred ? ' last-starred' : ''}" data-app-name="${escapeHtml(app.name)}" data-starred="${app.starred ? 'true' : 'false'}">` +
          `<a href="/app/${escapeHtml(app.name)}/" class="menu-item-link"${isCurrent ? ' aria-current="page"' : ''} tabindex="${isCurrent ? '0' : '-1'}">` +
          `<span class="icon">${icon}</span>` +
          `<span class="label">${label}</span>` +
          '</a>' +
          `<button class="star-toggle" type="button" tabindex="-1" data-app-name="${escapeHtml(app.name)}" aria-label="star ${label}" aria-pressed="${app.starred ? 'true' : 'false'}">${app.starred ? '★' : '☆'}</button>` +
          `<button class="drag-handle" type="button" tabindex="-1" draggable="true" aria-label="reorder ${label}">⋮</button>` +
          '</li>'
        );
      })
      .join('');
  }

  function seedGlobals(shell, app) {
    const chatBar = shell.chat_bar || {};
    window.facetsData = shell.facets || [];
    window.selectedFacet = app.facets_enabled ? shell.selected_facet : null;
    window.appFacetCounts = {};
    window.CONVEY_SETTINGS = {
      reportingEnabled: shell.settings?.reporting_enabled !== false
    };
    window.solChatBarSeed = chatBar.sol_request || null;
    window.solChatBarAttention = chatBar.attention || null;
    const input = document.getElementById('chatBarInput');
    if (input) {
      input.placeholder = chatBar.placeholder || 'send a message…';
    }
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
      applyBodyState(shell, app, context.day);
      renderMenu(shell, app.name);
      seedGlobals(shell, app);
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
