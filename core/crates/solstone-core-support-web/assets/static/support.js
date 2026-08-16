// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function() {
  const REQUEST_TIMEOUT_MS = 12000;
  const defaultSetTimer = (fn, ms) => setTimeout(fn, ms);
  const defaultClearTimer = id => clearTimeout(id);
  let _setTimer = defaultSetTimer;
  let _clearTimer = defaultClearTimer;

  function __setTimers(set, clear) {
    if (!set && !clear) {
      _setTimer = defaultSetTimer;
      _clearTimer = defaultClearTimer;
      return;
    }
    _setTimer = set || defaultSetTimer;
    _clearTimer = clear || defaultClearTimer;
  }

  function fetchWithTimeout(url, opts) {
    const controller = new AbortController();
    let timerId;
    const timeout = new Promise(function(_resolve, reject) {
      timerId = _setTimer(function() {
        controller.abort();
        reject(new Error('timed out'));
      }, REQUEST_TIMEOUT_MS);
    });
    const fetchOpts = Object.assign({}, opts || {}, {signal: controller.signal});
    return Promise.race([window.fetch(url, fetchOpts), timeout]).finally(function() {
      if (timerId !== undefined) _clearTimer(timerId);
    });
  }

  function esc(s) {
    const el = document.createElement('span');
    el.textContent = s;
    return el.innerHTML;
  }

  function attr(s) {
    return esc(s).replace(/"/g, '&quot;').replace(/'/g, '&#39;');
  }

  function runDiagnostics() {
    const btn = document.getElementById('diagnostics-btn');
    const loading = document.getElementById('diagnostics-loading');
    const results = document.getElementById('diagnostics-results');
    const errorEl = document.getElementById('diagnostics-error');

    btn.disabled = true;
    results.style.display = 'none';
    results.innerHTML = '';
    errorEl.style.display = 'none';
    errorEl.innerHTML = '';
    loading.style.display = '';

    fetchWithTimeout('/app/support/api/diagnostics')
      .then(function(r) {
        if (!r.ok) throw new Error('HTTP ' + r.status);
        return r.json();
      })
      .then(function(data) {
        loading.style.display = 'none';
        renderDiagnostics(data, results);
        results.style.display = '';
        btn.disabled = false;
      })
      .catch(function(err) {
        loading.style.display = 'none';
        errorEl.innerHTML = 'couldn\'t run diagnostics: ' + esc(err.message) +
          '<br><span style="color:#666;">try again in a moment.</span>';
        errorEl.style.display = '';
        btn.disabled = false;
      });
  }

  function renderDiagnostics(data, container) {
    var html = '';

    // Summary line
    var services = data.services || {};
    var serviceNames = Object.keys(services);
    var runningCount = serviceNames.filter(function(k) { return services[k] === 'running'; }).length;
    var errorCount = (data.recent_errors || []).length;
    html += '<div class="support-diagnostics-summary">' +
      esc(runningCount + ' of ' + serviceNames.length + ' services running') +
      (errorCount > 0 ? ', ' + esc(errorCount + ' recent error' + (errorCount !== 1 ? 's' : '')) : ', no recent errors') +
      '</div>';

    // Version + Platform section
    var versionPlatformRows = '';
    if (data.version) {
      versionPlatformRows += '<div class="support-diagnostics-row"><span class="support-diagnostics-label">version</span><span>' + esc(String(data.version)) + '</span></div>';
    }
    if (data.platform) {
      var p = data.platform;
      if (p.system) versionPlatformRows += '<div class="support-diagnostics-row"><span class="support-diagnostics-label">system</span><span>' + esc(String(p.system)) + ' ' + esc(String(p.release || '')) + '</span></div>';
      if (p.machine) versionPlatformRows += '<div class="support-diagnostics-row"><span class="support-diagnostics-label">machine</span><span>' + esc(String(p.machine)) + '</span></div>';
      if (p.python) versionPlatformRows += '<div class="support-diagnostics-row"><span class="support-diagnostics-label">python</span><span>' + esc(String(p.python)) + '</span></div>';
    }
    if (versionPlatformRows) {
      html += '<details class="support-diagnostics-section"><summary>version &amp; platform</summary>' +
        '<div class="support-diagnostics-body">' + versionPlatformRows + '</div></details>';
    }

    // Services section
    if (serviceNames.length > 0) {
      var svcRows = '';
      serviceNames.forEach(function(name) {
        var status = services[name] || 'unknown';
        var dotClass = status === 'running' ? 'support-diagnostics-running' : status === 'stopped' ? 'support-diagnostics-stopped' : 'support-diagnostics-unknown';
        svcRows += '<div class="support-diagnostics-row"><span class="support-diagnostics-label">' + esc(String(name)) + '</span>' +
          '<span><span class="support-diagnostics-dot ' + dotClass + '"></span>' + esc(String(status)) + '</span></div>';
      });
      html += '<details class="support-diagnostics-section"><summary>services</summary>' +
        '<div class="support-diagnostics-body">' + svcRows + '</div></details>';
    }

    // Recent errors section
    if (data.recent_errors && data.recent_errors.length > 0) {
      var errRows = '';
      data.recent_errors.forEach(function(e) {
        var t = e.time ? (e.time_approximate ? '~' + e.time : e.time) : '';
        errRows += '<div style="padding:0.2rem 0;">' +
          (t ? '<span style="color:#888;">' + esc(String(t)) + '</span> ' : '') +
          '<strong>' + esc(String(e.service || 'unknown')) + '</strong>: ' +
          esc(String(e.message || '')) + '</div>';
      });
      html += '<details class="support-diagnostics-section" open><summary>recent errors (' + esc(String(data.recent_errors.length)) + ')</summary>' +
        '<div class="support-diagnostics-body">' + errRows + '</div></details>';
    }

    // Config section
    if (data.config && Object.keys(data.config).length > 0) {
      var cfgRows = '';
      Object.keys(data.config).forEach(function(key) {
        cfgRows += '<div class="support-diagnostics-row"><span class="support-diagnostics-label">' + esc(String(key)) + '</span>' +
          '<span>' + esc(String(data.config[key])) + '</span></div>';
      });
      html += '<details class="support-diagnostics-section"><summary>config</summary>' +
        '<div class="support-diagnostics-body">' + cfgRows + '</div></details>';
    }

    // If nothing rendered beyond summary
    if (!versionPlatformRows && serviceNames.length === 0 && (!data.recent_errors || data.recent_errors.length === 0) && (!data.config || Object.keys(data.config).length === 0)) {
      html += '<div style="font-size:0.85rem;color:#666;">No diagnostic data available.</div>';
    }

    container.innerHTML = html;
  }

  async function loadTickets(deps) {
    const list = document.getElementById('tickets-list');
    list.innerHTML = window.SurfaceState.loading({ text: 'checking for tickets' });
    try {
      const resp = await fetchWithTimeout('/app/support/api/tickets');
      if (!resp.ok) {
        if (resp.status === 403) {
          document.getElementById('support-main').style.display = 'none';
          document.getElementById('support-disabled').style.display = '';
          return;
        }
        throw new Error('Failed to load tickets');
      }
      const tickets = await resp.json();

      if (!tickets.length) {
        list.innerHTML = window.SurfaceState.empty({
          icon: window.ConveyIcons.svg('life-buoy'),
          heading: 'no tickets yet — that\'s a good thing',
          desc: 'if something comes up, check the help tab for ways to get support',
          action: '<button type="button" class="surface-state-secondary" id="empty-help-btn">browse help &amp; guidance</button>'
        });
        const helpBtn = document.getElementById('empty-help-btn');
        if (helpBtn) helpBtn.addEventListener('click', () => deps.activateTab('help'));
        deps.activateTab('help');
        return;
      }

      list.innerHTML = tickets.map(t => {
        const ticketId = String(t.id || '');
        const statusClass = 'support-status-' + (t.status || 'open').replace(/[^a-z-]/g, '');
        const createdAt = t.created_at
          ? `${window.relativeTime(Date.now() - new Date(t.created_at + (t.created_at.includes('Z') ? '' : 'Z')).getTime())} ago`
          : '';
        return `<div class="support-ticket" data-id="${attr(ticketId)}" tabindex="0" role="button">
          <div class="support-ticket-header">
            <span class="support-ticket-subject">${esc(t.subject || 'untitled')}</span>
            <span class="support-status ${statusClass}">${esc(t.status || 'open')}</span>
          </div>
          <div class="support-ticket-meta">
            <span class="support-ticket-id">#${esc(ticketId)}</span> &middot;
            ${esc(t.product || '')} &middot;
            ${createdAt}
          </div>
        </div>`;
      }).join('');

      // Click to open detail
      list.querySelectorAll('.support-ticket').forEach(el => {
        el.addEventListener('click', () => deps.openTicket(parseInt(el.dataset.id)));
      });
      list.querySelectorAll('.support-ticket').forEach(el => {
        el.addEventListener('keydown', e => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            deps.openTicket(parseInt(el.dataset.id));
          }
        });
      });
      // Update ticket count and inject badge
      const count = tickets.length;
      let badge = document.getElementById('tab-tickets-badge');
      if (!badge) {
        badge = document.createElement('span');
        badge.id = 'tab-tickets-badge';
        badge.className = 'support-tab-badge';
        document.getElementById('tab-tickets').appendChild(badge);
      }
      badge.textContent = count;
      // Show badge if user already switched away from tickets tab
      const activeTab = document.querySelector('.support-nav button.active');
      const show = activeTab && activeTab.dataset.section !== 'tickets';
      badge.style.opacity = show ? '1' : '0';
      badge.style.pointerEvents = show ? 'auto' : 'none';
    } catch (e) {
      list.innerHTML = window.SurfaceState.error({
        heading: 'unable to load tickets',
        desc: 'check your connection and try refreshing',
        retry: true,
        retryLabel: 'try again',
        detail: e
      });
      const retryBtn = list.querySelector('.surface-state-retry');
      if (retryBtn) retryBtn.addEventListener('click', () => loadTickets(deps));
    }
  }

  window.SupportUI = {
    REQUEST_TIMEOUT_MS,
    fetchWithTimeout,
    runDiagnostics,
    renderDiagnostics,
    loadTickets,
    __setTimers
  };
})();
