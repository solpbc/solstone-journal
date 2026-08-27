// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function(){
  const HEALTH_LOGS_COPY = {
    "LOGS_LEVEL_FILTER_LABEL": "level",
    "LOGS_LEVEL_OPTION_ALL": "all levels",
    "LOGS_LEVEL_OPTION_ERROR": "errors only",
    "LOGS_LEVEL_OPTION_INFO": "info & above",
    "LOGS_LEVEL_OPTION_WARNING": "warnings & errors",
    "LOGS_SERVICE_COLLAPSED": "── {service} ── ({n} lines, ★ {errors} errors)",
    "LOGS_SERVICE_FILTER_LABEL": "service",
    "LOGS_STREAM_FILTER_LABEL": "stream"
  };
  const HEALTH_GLANCE_COPY = {
    "HEALTH_GLANCE_CATCHING_UP": "i'm catching up on {n} task(s) in the background. last update {age} ago.",
    "HEALTH_GLANCE_CLIENT_SILENT": "one of your devices hasn't reached your journal recently.",
    "HEALTH_GLANCE_OK": "everything's working. the solstone app last added to your journal {age} ago.",
    "HEALTH_GLANCE_BRAIN_ATTENTION": "{headline}",
    "HEALTH_GLANCE_SERVICES_ATTENTION": "{n} service(s) need attention: {service_names}.",
    "HEALTH_GLANCE_SERVICES_UNREACHABLE": "i couldn't reach my own services. check that your journal is running."
  };

  let brainSnapshot = null;
  let backlogCopy = {};

  fetch('/app/health/api/info')
    .then(r => r.json())
    .then(info => {
      state.localHost = info.hostname;
      brainSnapshot = info.brain || brainSnapshot;
      updateObserve();
      renderBrainHealth();
      updateStatusSummary();
    })
    .catch(() => {
      if (!state.connected) {
        connectError = true;
        updateStatusSummary();
      }
    });

  fetch('/app/stats/api/usage?day=' + todayKey())
    .then(r => {
      if (!r.ok) throw new Error('token usage unavailable');
      return r.json();
    })
    .then(data => {
      const cost = Number(data?.total?.cost);
      if (Number.isFinite(cost)) {
        state.todayCostUSD = cost;
      }
      updateStatusSummary();
    })
    .catch(() => {
      state.todayCostUSD = null;
      elements.glanceCostValue.textContent = '—';
      updateStatusSummary();
    });

  // State management
  let connectError = false;
  let recentEventTimestamps = [];

  const state = {
    services: new Map(),        // Running services
    connected: false,
    crashed: new Map(),         // Crashed services (separate from running)
    tasks: [],
    health: null,
    queues: {},                 // From supervisor.status
    schedules: [],              // From supervisor.status
    agents: new Map(),
    agentCount: 0,              // Quick count from cortex.status
    imports: new Map(),
    think: null,                // Think status snapshot (null when idle)
    thinkActive: false,         // Whether think is currently running
    serviceLogs: new Map(),     // service name -> array of {ts, stream, line}
    logFollow: true,            // Auto-scroll log viewport
    logsCollapsed: true,
    logLevelFilter: 'all',
    logCollapsedServices: new Map(),
    logErrorCount: 0,
    logTotalCount: 0,
    lastLogTs: null,
    lastAgentFinishTs: null,
    todayCostUSD: null,
    clients: new Map(),       // keyed by stream name
    recentErrors: [],
    agentErrorsOk: true,
    recentErrorsFilter: null,
    pendingRecentErrorsFocus: false,
    pendingLogAnchor: null,
    localHost: null,
    deepLinkMode: false,
    lastLogFilter: null,        // Last rendered filter state for incremental append
    lastEventTs: null,           // Timestamp of last event from WebSocket
  };

  const escapeHtml = (value) => window.AppServices.escapeHtml(value);

  // DOM elements
  const elements = {
    healthGlanceSentence: document.getElementById('healthGlanceSentence'),
    glanceCostValue: document.getElementById('glanceCostValue'),
    glanceActivityValue: document.getElementById('glanceActivityValue'),
    glanceErrorsValue: document.getElementById('glanceErrorsValue'),
    glanceErrorsLabel: document.getElementById('glanceErrorsLabel'),
    serviceDots: document.getElementById('serviceDots'),
    agentsValue: document.getElementById('agentsValue'),
    tasksValue: document.getElementById('tasksValue'),
    healthValue: document.getElementById('healthValue'),
    vitalsStatus: document.getElementById('vitalsStatus'),
    observeModeBadge: document.getElementById('observeModeBadge'),
    observeModeLabel: document.getElementById('observeModeLabel'),
    screencastStatus: document.getElementById('screencastStatus'),
    screencastDetail: document.getElementById('screencastDetail'),
    tmuxStatus: document.getElementById('tmuxStatus'),
    tmuxDetail: document.getElementById('tmuxDetail'),
    audioStatus: document.getElementById('audioStatus'),
    audioDetail: document.getElementById('audioDetail'),
    activityStatus: document.getElementById('activityStatus'),
    activityDetail: document.getElementById('activityDetail'),
    describeStatus: document.getElementById('describeStatus'),
    describeDetail: document.getElementById('describeDetail'),
    transcribeStatus: document.getElementById('transcribeStatus'),
    transcribeDetail: document.getElementById('transcribeDetail'),
    observeSourceNote: document.getElementById('observeSourceNote'),
    clientsCard: document.getElementById('clientsCard'),
    clientsGrid: document.getElementById('clientsGrid'),
    registeredClientsCard: document.getElementById('registeredClientsCard'),
    registeredClientsStrip: document.getElementById('registeredClientsStrip'),
    observeContent: document.getElementById('observeContent'),
    observeEmpty: document.getElementById('observeEmpty'),
    cortexSection: document.getElementById('cortexSection'),
    cortexGrid: document.getElementById('cortexGrid'),
    importerSection: document.getElementById('importerSection'),
    importerGrid: document.getElementById('importerGrid'),
    errorSummary: document.getElementById('errorSummary'),
    errorSummaryContent: document.getElementById('errorSummaryContent'),
    allQuietCard: document.getElementById('allQuietCard'),
    idleCardStats: document.getElementById('idleCardStats'),
    thinkCard: document.getElementById('thinkCard'),
    thinkInfo: document.getElementById('thinkInfo'),
    thinkProgress: document.getElementById('thinkProgress'),
    thinkAgents: document.getElementById('thinkAgents'),
    queuesSection: document.getElementById('queuesSection'),
    queuesValue: document.getElementById('queuesValue'),
    schedulesSection: document.getElementById('schedulesSection'),
    schedulesValue: document.getElementById('schedulesValue'),
    logsViewport: document.getElementById('logsViewport'),
    logServiceFilter: document.getElementById('logServiceFilter'),
    logLevelFilter: document.getElementById('logLevelFilter'),
    logStreamFilter: document.getElementById('logStreamFilter'),
    logFollowBtn: document.getElementById('logFollowBtn'),
    logClearBtn: document.getElementById('logClearBtn'),
    logErrorBadge: document.getElementById('logErrorBadge'),
    logsConnectionNote: document.getElementById('logsConnectionNote'),
    logsSummaryBadge: document.getElementById('logsSummaryBadge'),
    brainHealthStatus: document.getElementById('brainHealthStatus'),
    brainCheckBtn: document.getElementById('brainCheckBtn'),
    logsAnnouncer: document.getElementById('logsAnnouncer'),
    logsCollapseIndicator: document.getElementById('logsCollapseIndicator'),
    connectionIndicator: document.getElementById('connectionIndicator'),
    trustIndicator: document.getElementById('trustIndicator'),
    vitalsBar: document.querySelector('.vitals-bar'),
    vitalsCheckBtn: document.getElementById('vitalsCheckBtn'),
    logExportBtn: document.getElementById('logExportBtn'),
  };

  function logHealthError(error, context) {
    if (window.logError) {
      window.logError(error, { context });
    } else if (window.console && window.console.error) {
      window.console.error(error);
    }
  }

  function applyHealthCopy() {
    document.querySelectorAll('[data-health-copy]').forEach((element) => {
      const key = element.getAttribute('data-health-copy');
      if (!key || !Object.hasOwn(HEALTH_LOGS_COPY, key)) {
        logHealthError(new Error(`missing health copy key: ${key || ''}`), 'health copy render');
        return;
      }
      element.textContent = HEALTH_LOGS_COPY[key];
    });
  }

  async function getJson(path) {
    if (window.apiJson) {
      return window.apiJson(path);
    }
    const response = await fetch(path, { headers: { Accept: 'application/json' } });
    const payload = await response.json();
    if (!response.ok) throw payload;
    return payload;
  }

  function clearHealthStateError() {
    const errorHost = document.querySelector('[data-health-state-error]');
    if (!errorHost) return;
    errorHost.hidden = true;
    errorHost.textContent = '';
  }

  function renderHealthStateError(error) {
    const errorHost = document.querySelector('[data-health-state-error]');
    if (!errorHost) {
      logHealthError(error, 'health state fetch failed');
      return;
    }
    errorHost.hidden = false;
    if (window.SurfaceState) {
      errorHost.innerHTML = window.SurfaceState.error({
        heading: "I couldn't load health state",
        desc: window.CONVEY_COPY?.RELOAD_HINT || 'reload to try again.',
        retry: true,
        detail: error,
        headingLevel: 'h3'
      });
    } else {
      errorHost.innerHTML = '<button type="button" class="surface-state-retry">try again</button>';
    }
    errorHost.querySelector('.surface-state-retry')?.addEventListener('click', () => {
      loadHealthState();
    });
    logHealthError(error, 'health state fetch failed');
  }

  function renderBacklogState(backlogState) {
    const data = backlogState || {};
    backlogCopy = data.copy || {};
    const verdictLine = document.querySelector('#backlogVerdict .backlog-verdict-line');
    if (verdictLine) verdictLine.textContent = data.verdict || '';
    clearHealthStateError();

    const host = document.querySelector('[data-backlog-stuck-rows]');
    if (!host) return;
    const rowsHost = host.querySelector('[data-backlog-rows]');
    const rows = Array.isArray(data.stuck_rows) ? data.stuck_rows : [];
    if (!rowsHost || rows.length === 0) {
      host.hidden = true;
      if (rowsHost) rowsHost.replaceChildren();
      return;
    }

    host.hidden = false;
    const heading = host.querySelector('[data-backlog-heading]');
    const description = host.querySelector('[data-backlog-description]');
    if (heading) heading.textContent = backlogCopy.bucket_heading || '';
    if (description) description.textContent = backlogCopy.bucket_description || '';
    rowsHost.replaceChildren();
    rows.forEach((row) => {
      const rowEl = document.createElement('div');
      rowEl.className = 'backlog-row';

      const main = document.createElement('div');
      main.className = 'backlog-row-main';

      const day = document.createElement('span');
      day.className = 'backlog-row-day';
      day.textContent = row.day || '';
      main.appendChild(day);

      const badge = document.createElement('span');
      badge.className = 'backlog-badge';
      badge.textContent = backlogCopy.day_badge || '';
      main.appendChild(badge);

      const reason = document.createElement('span');
      reason.className = 'backlog-row-reason';
      reason.textContent = row.reason || '';
      main.appendChild(reason);

      const detailFields = ['reason_code', 'provider', 'model'].filter(field => row[field]);
      if (detailFields.length > 0) {
        const details = document.createElement('details');
        details.className = 'backlog-row-details';
        const summary = document.createElement('summary');
        summary.textContent = 'details';
        details.appendChild(summary);
        detailFields.forEach((field) => {
          const detail = document.createElement('span');
          detail.textContent = `${field}=${row[field]}`;
          details.appendChild(detail);
        });
        main.appendChild(details);
      }

      rowEl.appendChild(main);

      if (row.depth) {
        const depth = document.createElement('span');
        depth.className = 'backlog-depth';
        depth.textContent = String(row.depth);
        rowEl.appendChild(depth);
      }

      const actions = document.createElement('div');
      actions.className = 'backlog-row-actions';

      const process = document.createElement('button');
      process.type = 'button';
      process.className = 'backlog-action';
      process.dataset.day = row.day || '';
      process.dataset.flavor = 'process-now';
      process.textContent = backlogCopy.action_process_now || '';
      actions.appendChild(process);

      const redo = document.createElement('button');
      redo.type = 'button';
      redo.className = 'backlog-action';
      redo.dataset.day = row.day || '';
      redo.dataset.flavor = 'from-scratch';
      redo.dataset.confirm = backlogCopy.confirm_redo_scratch || '';
      redo.textContent = backlogCopy.action_redo_scratch || '';
      actions.appendChild(redo);

      const status = document.createElement('span');
      status.className = 'backlog-action-status';
      actions.appendChild(status);

      rowEl.appendChild(actions);
      rowsHost.appendChild(rowEl);
    });
    wireBacklogReprocessActions();
  }

  function renderAgentErrorsState(agentErrors) {
    const data = agentErrors || {};
    state.agentErrorsOk = data.ok !== false;
    seedAgentErrors(Array.isArray(data.items) ? data.items : []);
    if (elements.glanceErrorsValue) {
      elements.glanceErrorsValue.textContent = state.agentErrorsOk ? String(data.count || 0) : '—';
    }
    if (elements.glanceErrorsLabel) {
      elements.glanceErrorsLabel.textContent = data.label || 'errors today';
    }
    updateStatusSummary();
  }

  async function loadHealthState() {
    clearHealthStateError();
    try {
      const payload = await getJson('/app/health/api/state');
      renderBacklogState(payload.backlog);
      renderAgentErrorsState(payload.agent_errors);
    } catch (error) {
      renderHealthStateError(error);
    }
  }

  let timeoutFired = false;
  let programmaticScroll = false;
  const STALE_MS = 30000;
  // Human-readable service names
  const SERVICE_NAMES = {
    supervisor: 'System Manager',
    convey: 'Web Interface',
    cortex: 'AI Engine',
    sense: 'Media Processor',
    observe: 'Screen & Audio',
    think: 'Background Analysis',
    importer: 'file importer',
    schedule: 'Task Scheduler',
  };

  function serviceName(internal) {
    return SERVICE_NAMES[internal] || internal;
  }

  function sweepUnresolvedSkeletons() {
    if (timeoutFired) return;
    timeoutFired = true;
    const targets = [
      elements.vitalsStatus,
      elements.serviceDots,
      elements.healthValue,
    ];
    for (const el of targets) {
      if (el && el.querySelector('.skeleton, .skeleton-dark')) {
        el.textContent = 'unavailable';
      }
    }
  }

  function armSkeletonTimeout() {
    setTimeout(sweepUnresolvedSkeletons, 7000);
  }

  // Utility functions
  function formatElapsed(seconds) {
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    const secs = seconds % 60;
    return `${minutes}m ${secs}s`;
  }

  function formatDuration(ms) {
    return formatElapsed(Math.floor(ms / 1000));
  }

  function relativeTime(ms) {
    let seconds = Math.floor(ms / 1000);
    if (!Number.isFinite(seconds) || seconds < 0) seconds = 0;
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes}m`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `${hours}h`;
    const days = Math.floor(hours / 24);
    return `${days}d`;
  }

	  function truncate(str, len) {
	    if (!str) return '';
	    return str.length > len ? str.substring(0, len) + '...' : str;
	  }

	  function dayKeyFromTimestamp(ts) {
	    const date = new Date(ts);
	    if (Number.isNaN(date.getTime())) return '';
	    const year = String(date.getFullYear());
	    const month = String(date.getMonth() + 1).padStart(2, '0');
	    const day = String(date.getDate()).padStart(2, '0');
	    return year + month + day;
	  }

	  function todayKey() {
	    return dayKeyFromTimestamp(Date.now());
	  }

	  function resolveRecentErrorsDay(day) {
	    if (!day || day === 'today') return todayKey();
	    return /^\d{8}$/.test(day) ? day : null;
	  }

	  function recentErrorMatchesFilter(entry) {
	    const filter = state.recentErrorsFilter;
	    if (!filter) return true;
	    const requestedDay = resolveRecentErrorsDay(filter.day);
	    if (requestedDay && dayKeyFromTimestamp(entry.ts) !== requestedDay) return false;
	    if (filter.talent && entry.name !== filter.talent) return false;
	    return true;
	  }

  function getAgentId(id) {
    return String(id).slice(-4);
  }

  function formatNextRun(epochMs) {
    if (!epochMs) return '';
    const delta = epochMs - Date.now();
    if (delta < 0) return 'overdue';
    const mins = Math.floor(delta / 60000);
    if (mins < 60) return `in ${mins}m`;
    const hours = Math.floor(mins / 60);
    if (hours < 24) return `in ${hours}h`;
    const days = Math.floor(hours / 24);
    return `in ${days}d`;
  }

  function renderInfoItems(parent, items) {
    const filtered = items.filter(item => item.value != null);
    while (parent.children.length > filtered.length) {
      parent.removeChild(parent.lastChild);
    }
    filtered.forEach((item, i) => {
      let el = parent.children[i];
      if (!el) {
        el = document.createElement('div');
        el.className = 'info-item';
        const label = document.createElement('div');
        label.className = 'info-label';
        el.appendChild(label);
        el.appendChild(document.createElement('div'));
        parent.appendChild(el);
      }
      el.children[0].textContent = item.label;
      el.children[1].textContent = item.value;
    });
  }

	  function selectGlanceSentence(state, now) {
    const activeAgents = Array.from(state.agents.values()).filter(agent => agent.event !== 'finish' && agent.event !== 'error').length;
    const activeImports = Array.from(state.imports.values()).filter(imp => imp.event !== 'completed' && imp.event !== 'error').length;
    const staleHeartbeats = state.health?.stale_heartbeats || [];

    if (state.connected === false && connectError === true) {
      return { key: 'HEALTH_GLANCE_SERVICES_UNREACHABLE', vars: {} };
    }

    if (state.crashed.size > 0 || staleHeartbeats.length > 0) {
      const names = Array.from(new Set([
        ...Array.from(state.crashed.keys()),
        ...staleHeartbeats,
      ])).sort();
      return {
        key: 'HEALTH_GLANCE_SERVICES_ATTENTION',
        vars: {
          n: String(names.length),
          service_names: names.map(serviceName).join(', '),
        },
      };
    }

    if (brainSnapshot && ['blocked', 'unhealthy', 'unknown'].includes(brainSnapshot.state)) {
      return {
        key: 'HEALTH_GLANCE_BRAIN_ATTENTION',
        vars: { headline: brainSnapshot.headline || '' },
      };
    }

    const clients = Array.from(state.clients.values());
    if (clients.length > 0 && clients.every(client => (now - client.lastSeen) >= STALE_MS)) {
      const ageMs = Math.min(...clients.map(client => now - client.lastSeen));
      return {
        key: 'HEALTH_GLANCE_CLIENT_SILENT',
        vars: { age: relativeTime(ageMs) },
      };
    }

	    if (activeAgents > 0 || activeImports > 0) {
      return {
        key: 'HEALTH_GLANCE_CATCHING_UP',
        vars: {
          n: String(activeAgents + activeImports),
          age: relativeTime(now - (state.lastEventTs || now)),
        },
      };
    }

    if (state.services.size > 0 || state.crashed.size > 0) {
      return {
        key: 'HEALTH_GLANCE_OK',
        vars: { age: relativeTime(now - (state.lastEventTs || now)) },
      };
    }

    return null;
	  }

  function formatGlanceSentence(selection) {
    if (!selection) return '';
    let text = HEALTH_GLANCE_COPY[selection.key] || '';
    for (const [key, value] of Object.entries(selection.vars || {})) {
      text = text.replaceAll('{' + key + '}', value);
    }
    return text;
  }

  function updateStatusSummary() {
    const now = Date.now();
    const selection = selectGlanceSentence(state, now);
    elements.healthGlanceSentence.textContent = formatGlanceSentence(selection);

    elements.glanceCostValue.textContent = Number.isFinite(state.todayCostUSD)
      ? '$' + state.todayCostUSD.toFixed(2)
      : '—';

    const recentActivity = recentEventTimestamps.filter(ts => (now - ts) < 3600000).length;
    elements.glanceActivityValue.textContent = String(recentActivity);

    const today = todayKey();
    if (!state.agentErrorsOk) {
      elements.glanceErrorsValue.textContent = '—';
      elements.glanceErrorsLabel.textContent = 'errors today';
    } else {
      const errorsToday = state.recentErrors.filter(error => dayKeyFromTimestamp(error.ts) === today).length;
      elements.glanceErrorsValue.textContent = String(errorsToday);
      elements.glanceErrorsLabel.textContent = (errorsToday === 1 ? 'error today' : 'errors today');
    }
  }

  function renderBrainHealth() {
    const box = elements.brainHealthStatus;
    if (!box) return;
    const brain = brainSnapshot || {};
    const identity = brain.identity || {};
    const evidence = brain.evidence || {};
    const component = brain.failing_component ? ` (${brain.failing_component})` : '';
    const lines = [];
    if (brain.headline) lines.push(brain.headline);
    if (identity.lane && identity.provider && identity.model) {
      if (brain.state === 'ready') {
        const checked = evidence.age_text ? `, checked ${evidence.age_text} ago` : '';
        lines.push(`${identity.lane} ${identity.provider}/${identity.model}${checked}`);
      } else {
        lines.push(`${identity.lane} ${identity.provider}/${identity.model} — ${brain.reason_text || ''}${component}`);
      }
    } else if (identity.lane || identity.provider || identity.model) {
      lines.push(`${brain.reason_text || ''}${component}`);
    }
    box.innerHTML = '';
    lines.forEach((line) => {
      const p = document.createElement('p');
      p.textContent = line;
      box.appendChild(p);
    });
    const action = brain.action || null;
    const button = elements.brainCheckBtn;
    if (!button) return;
    if (!action?.label) {
      button.hidden = true;
      button.onclick = null;
      return;
    }
    button.textContent = action.label;
    button.hidden = false;
    if (action.href) {
      button.onclick = () => {
        window.location.href = action.href;
      };
    } else if (action.refresh) {
      button.onclick = () => requestBrainCheck();
    } else {
      button.onclick = null;
    }
  }

  function requestBrainCheck() {
    const button = elements.brainCheckBtn;
    if (button) button.disabled = true;
    return fetch('/app/health/api/brain/check', {method: 'POST'})
      .then(r => r.json())
      .then(payload => {
        brainSnapshot = payload.brain || brainSnapshot;
        renderBrainHealth();
        updateStatusSummary();
      })
      .catch(() => {
        renderBrainHealth();
      })
      .finally(() => {
        if (button) button.disabled = false;
      });
  }

  function updateAllQuiet() {
    const allHidden = elements.cortexSection.classList.contains('hidden') &&
      elements.importerSection.classList.contains('hidden') &&
      elements.thinkCard.classList.contains('hidden');
    if (allHidden) updateAllQuietContent();
    elements.allQuietCard.classList.toggle('hidden', !allHidden);
  }

  function updateAllQuietContent() {
    const container = elements.idleCardStats;
    let idx = 0;

    function ensureChild(i) {
      let el = container.children[i];
      if (!el) {
        el = document.createElement('div');
        el.className = 'idle-stat';
        container.appendChild(el);
      }
      return el;
    }

    if (state.lastAgentFinishTs) {
      const ago = relativeTime(Date.now() - state.lastAgentFinishTs);
      const el = ensureChild(idx++);
      el.textContent = 'last talent finished ' + ago + ' ago';
      el.style.color = '';
    }

    const errCount = state.recentErrors.length;
    const errEl = ensureChild(idx++);
    if (!state.agentErrorsOk) {
      errEl.textContent = "couldn't check talent errors today.";
      errEl.style.color = '#92400e';
    } else if (errCount > 0) {
      errEl.textContent = errCount + ' recent error' + (errCount !== 1 ? 's' : '');
      errEl.style.color = '#dc2626';
    } else {
      errEl.textContent = 'no recent errors';
      errEl.style.color = '';
    }

    const now = Date.now();
    const nextSchedule = state.schedules
      .filter(s => s.next_run && s.next_run > now)
      .sort((a, b) => a.next_run - b.next_run)[0];
    if (nextSchedule) {
      const el = ensureChild(idx++);
      el.textContent = 'next: ' + (nextSchedule.name || 'scheduled') + ' ' + formatNextRun(nextSchedule.next_run);
      el.style.color = '';
    }

    while (container.children.length > idx) {
      container.removeChild(container.lastChild);
    }
  }

  function updateLogsBadge() {
    if (state.deepLinkMode) return;
    let text = state.logTotalCount + ' line' + (state.logTotalCount !== 1 ? 's' : '');
    if (state.lastLogTs) {
      const d = new Date(state.lastLogTs);
      const hh = String(d.getHours()).padStart(2, '0');
      const mm = String(d.getMinutes()).padStart(2, '0');
      const ss = String(d.getSeconds()).padStart(2, '0');
      text += ' · ' + hh + ':' + mm + ':' + ss;
    }
    elements.logsSummaryBadge.textContent = text;
  }
  updateLogsBadge();

  function isAtBottom(viewport, tol = 50) {
    return viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight <= tol;
  }

  function scrollLogsToBottom(viewport = elements.logsViewport) {
    programmaticScroll = true;
    viewport.scrollTop = viewport.scrollHeight;
    requestAnimationFrame(() => {
      programmaticScroll = false;
    });
  }

  function recentErrorGroupKey(entry) {
    if (entry.key) return entry.key;
    if (entry.reason_code) {
      return [
        entry.reason_code || '',
        entry.provider || '',
        entry.model || '',
      ].join(':');
    }
    return [
      'fallback',
      entry.type || '',
      entry.service || '',
      entry.name || '',
      entry.stage || '',
      entry.error || '',
    ].join(':');
  }

  function recentErrorOwnerMessage(entry) {
    if (entry.summary) return entry.summary;
    if (entry.reason_code && window.renderChatReason) {
      return window.renderChatReason(entry.reason_code, entry.provider || '').message;
    }
    return entry.error || window.CONVEY_COPY.UNKNOWN_ERROR;
  }

  function recentErrorDetailText(entry) {
    const parts = [];
    if (entry.reason_code) parts.push(`reason_code=${entry.reason_code}`);
    if (entry.provider) parts.push(`provider=${entry.provider}`);
    if (entry.model) parts.push(`model=${entry.model}`);
    if (entry.service) parts.push(`service=${entry.service}`);
    if (entry.stage) parts.push(`stage=${entry.stage}`);
    return parts.join(' · ');
  }

  function groupedRecentErrors() {
    const groups = new Map();
    state.recentErrors.forEach((entry, realIdx) => {
      if (!recentErrorMatchesFilter(entry)) return;
      const key = recentErrorGroupKey(entry);
      const existing = groups.get(key);
      if (!existing) {
        groups.set(key, {
          key,
          e: entry,
          realIdx,
          count: 1,
          lastTs: entry.ts || 0,
        });
        return;
      }
      existing.count += 1;
      if ((entry.ts || 0) >= existing.lastTs) {
        existing.e = entry;
        existing.realIdx = realIdx;
        existing.lastTs = entry.ts || 0;
      }
    });
    return Array.from(groups.values()).sort((a, b) => a.lastTs - b.lastTs);
  }

  function appendRecentError(entry) {
    if (entry?.id) {
      const dedupeIdx = state.recentErrors.findIndex(existing =>
        existing?.id === entry.id && (existing.type || '') === (entry.type || '')
      );
      if (dedupeIdx !== -1) {
        state.recentErrors[dedupeIdx] = { ...state.recentErrors[dedupeIdx], ...entry };
        updateErrorSummary();
        return;
      }
    }
    state.recentErrors.push(entry);
    if (state.recentErrors.length > 50) state.recentErrors.shift();
    updateErrorSummary();
  }

  let recentErrorPanelSeq = 0;

  function updateErrorSummary() {
    if (state.recentErrors.length === 0 && !state.recentErrorsFilter && state.agentErrorsOk) {
      elements.errorSummary.classList.add('hidden');
      return;
    }

    elements.errorSummary.classList.remove('hidden');
    const container = elements.errorSummaryContent;
    const recent = groupedRecentErrors().slice(-10).reverse();

    const newKeys = new Set();
    const entries = recent.map(group => {
      const { e, realIdx, key, count } = group;
      newKeys.add(key);
      return { e, realIdx, key, count };
    });

    const existingByKey = new Map();
    for (const child of Array.from(container.children)) {
      const k = child.getAttribute('data-key');
      if (k) existingByKey.set(k, child);
    }

    for (const [k, child] of existingByKey) {
      if (!newKeys.has(k)) container.removeChild(child);
    }

    for (const child of Array.from(container.children)) {
      if (!child.getAttribute('data-key')) container.removeChild(child);
    }

    if (!state.agentErrorsOk && entries.length > 0) {
      const degraded = document.createElement('div');
      degraded.style.cssText = 'padding: 0.3em 0; font-size: 0.85em; color: #92400e;';
      degraded.textContent = "couldn't check talent errors today.";
      container.appendChild(degraded);
    }

    if (state.recentErrorsFilter) {
      const filter = state.recentErrorsFilter;
      const label = document.createElement('div');
      label.style.cssText = 'padding: 0.2em 0 0.45em; font-size: 0.8em; color: #6b7280; display: flex; gap: 0.5em; align-items: center;';
      const dayText = filter.day === 'today' ? "today's" : filter.day;
      label.appendChild(document.createTextNode(`showing ${dayText} errors`));
      if (filter.talent) {
        label.appendChild(document.createTextNode(` for ${filter.talent}`));
      }
      const clearBtn = document.createElement('button');
      clearBtn.type = 'button';
      clearBtn.setAttribute('data-action', 'clear-error-filter');
      clearBtn.className = 'error-advice-link';
      clearBtn.textContent = 'clear';
      label.appendChild(clearBtn);
      container.appendChild(label);
    }

    if (entries.length === 0) {
      const empty = document.createElement('div');
      empty.style.cssText = 'padding: 0.3em 0; font-size: 0.85em; color: #6b7280;';
      empty.textContent = !state.agentErrorsOk
        ? "couldn't check talent errors today."
        : (state.recentErrorsFilter ? 'no matching recent errors yet.' : 'no recent errors.');
      container.appendChild(empty);
      if (state.pendingRecentErrorsFocus) {
        state.pendingRecentErrorsFocus = false;
        requestAnimationFrame(() => elements.errorSummary.scrollIntoView({ behavior: 'smooth', block: 'start' }));
      }
      return;
    }

    for (const { e, realIdx, key, count } of entries) {
      let row = existingByKey.get(key);
      if (!row) {
        row = document.createElement('div');
        row.setAttribute('data-key', key);
        row.style.cssText = 'padding: 0.3em 0; font-size: 0.85em; color: #374151; border-bottom: 1px solid #e5e7eb; display: flex; align-items: baseline; gap: 0.4em; flex-wrap: wrap;';

        const panelId = 'recent-error-panel-' + (++recentErrorPanelSeq);

        const summaryBtn = document.createElement('button');
        summaryBtn.type = 'button';
        summaryBtn.setAttribute('data-action', 'toggle-error');
        summaryBtn.setAttribute('aria-expanded', 'false');
        summaryBtn.setAttribute('aria-controls', panelId);
        summaryBtn.style.cssText = 'flex: 1 1 auto; min-width: 0; text-align: left; background: none; border: none; padding: 0; margin: 0; font: inherit; color: inherit; cursor: pointer;';
        row.appendChild(summaryBtn);

        const actions = document.createElement('span');
        actions.style.cssText = 'margin-left: auto; display: flex; gap: 0.3em;';
        const viewBtn = document.createElement('button');
        viewBtn.setAttribute('data-action', 'view-logs');
        viewBtn.className = 'error-action-btn';
        viewBtn.textContent = 'view logs';
        actions.appendChild(viewBtn);
        const dismissBtn = document.createElement('button');
        dismissBtn.setAttribute('data-action', 'dismiss');
        dismissBtn.className = 'error-dismiss-btn';
        dismissBtn.textContent = 'dismiss';
        actions.appendChild(dismissBtn);
        row.appendChild(actions);

        const panel = document.createElement('div');
        panel.id = panelId;
        panel.hidden = true;
        panel.setAttribute('data-error-panel', 'true');
        panel.style.cssText = 'flex-basis: 100%; width: 100%; padding: 0.35em 0 0.1em; color: #374151;';
        row.appendChild(panel);
      }

      const icon = e.type === 'agent' ? '⚙' : e.type === 'import' ? '↓' : '⚠';
      const ago = relativeTime(Date.now() - (e.ts || Date.now()));
      const summaryBtn = row.querySelector('[data-action="toggle-error"]');
      summaryBtn.textContent = '';
      summaryBtn.appendChild(document.createTextNode(icon + ' '));
      const strong = document.createElement('strong');
      strong.textContent = e.name;
      summaryBtn.appendChild(strong);
      summaryBtn.appendChild(document.createTextNode(' — ' + truncate(recentErrorOwnerMessage(e), 70) + ' '));
      if (count > 1) {
        const countSpan = document.createElement('span');
        countSpan.style.cssText = 'color: #6b7280; font-size: 0.85em; font-weight: 600;';
        countSpan.textContent = `×${count} `;
        summaryBtn.appendChild(countSpan);
      }
      const timeSpan = document.createElement('span');
      timeSpan.style.cssText = 'color: #9ca3af; font-size: 0.85em;';
      timeSpan.textContent = ago + ' ago';
      summaryBtn.appendChild(timeSpan);

      const panel = row.querySelector('[data-error-panel]');
      panel.textContent = '';
      const fullMsg = document.createElement('div');
      fullMsg.textContent = recentErrorOwnerMessage(e);
      panel.appendChild(fullMsg);
      const detailText = recentErrorDetailText(e);
      if (detailText) {
        const tech = document.createElement('div');
        tech.style.cssText = 'margin-top: 0.3em; font-size: 0.85em; color: #6b7280;';
        tech.textContent = detailText;
        panel.appendChild(tech);
      }

      const viewBtn = row.querySelector('[data-action="view-logs"]');
      viewBtn.setAttribute('data-error-index', realIdx);
      const dismissBtn = row.querySelector('[data-action="dismiss"]');
      dismissBtn.setAttribute('data-error-index', realIdx);

      container.appendChild(row);
    }

    if (state.pendingRecentErrorsFocus) {
      state.pendingRecentErrorsFocus = false;
      requestAnimationFrame(() => elements.errorSummary.scrollIntoView({ behavior: 'smooth', block: 'start' }));
    }

    // Error-type-specific advice templates
    const types = new Set(recent.map(({ e }) => e.type));
    if (types.has('agent')) {
      const advice = document.createElement('div');
      advice.style.cssText = 'padding: 0.25em 0; font-size: 0.8em; color: #9ca3af;';
      advice.appendChild(document.createTextNode('Talent errors usually resolve on the next run. '));
      const btn = document.createElement('button');
      btn.setAttribute('data-action', 'view-logs');
      btn.setAttribute('data-service', 'cortex');
      btn.className = 'error-advice-link';
      btn.textContent = 'view ai engine logs';
      advice.appendChild(btn);
      advice.appendChild(document.createTextNode(' for details.'));
      container.appendChild(advice);
    }
    if (types.has('import')) {
      const importErr = recent.find(({ e }) => e.type === 'import')?.e;
      const stageText = importErr?.stage && importErr.stage !== 'unknown' ? ' at ' + importErr.stage : '';
      const advice = document.createElement('div');
      advice.style.cssText = 'padding: 0.25em 0; font-size: 0.8em; color: #9ca3af;';
      advice.appendChild(document.createTextNode('import failed' + stageText + '. check the source file and retry, or '));
      const btn = document.createElement('button');
      btn.setAttribute('data-action', 'view-logs');
      btn.setAttribute('data-service', 'importer');
      btn.className = 'error-advice-link';
      btn.textContent = 'view file importer logs';
      advice.appendChild(btn);
      advice.appendChild(document.createTextNode('.'));
      container.appendChild(advice);
    }
    // Default fallback for any unrecognized error types (connection/service errors)
    if (Array.from(types).some(t => t !== 'agent' && t !== 'import')) {
      const advice = document.createElement('div');
      advice.style.cssText = 'padding: 0.25em 0; font-size: 0.8em; color: #9ca3af;';
      advice.appendChild(document.createTextNode('service error detected. '));
      const btn = document.createElement('button');
      btn.setAttribute('data-action', 'view-logs');
      btn.setAttribute('data-service', 'supervisor');
      btn.className = 'error-advice-link';
      btn.textContent = 'view system manager logs';
      advice.appendChild(btn);
      advice.appendChild(document.createTextNode(' for details.'));
      container.appendChild(advice);
    }
  }

	  function viewServiceLogs(service, ts) {
	    // Set filter to the relevant service
	    const option = Array.from(elements.logServiceFilter.options).find(o => o.value === service);
	    if (option) {
	      elements.logServiceFilter.value = service;
	    }
	    if (ts) state.pendingLogAnchor = ts;
	    // Expand logs if collapsed
	    if (state.logsCollapsed) {
      state.logsCollapsed = false;
      document.querySelector('.logs-card').classList.remove('logs-collapsed');
      elements.logsCollapseIndicator.textContent = '▼ hide';
      document.querySelector('.logs-header').setAttribute('aria-expanded', 'true');
    }
    renderLogs();
    document.querySelector('.logs-card').scrollIntoView({ behavior: 'smooth' });
  }

  elements.errorSummaryContent.addEventListener('click', (e) => {
    const btn = e.target.closest('[data-action]');
    if (!btn) return;
    const action = btn.dataset.action;
    if (action === 'toggle-error') {
      const expanded = btn.getAttribute('aria-expanded') === 'true';
      btn.setAttribute('aria-expanded', expanded ? 'false' : 'true');
      const panel = document.getElementById(btn.getAttribute('aria-controls'));
      if (panel) panel.hidden = expanded;
      return;
    }
    if (action === 'dismiss') {
      const idx = parseInt(btn.dataset.errorIndex, 10);
      if (!isNaN(idx) && idx >= 0 && idx < state.recentErrors.length) {
        state.recentErrors.splice(idx, 1);
        updateErrorSummary();
        updateStatusSummary();
      }
	    } else if (action === 'view-logs') {
      // If data-service is set (from advice template), use it directly
      const service = btn.dataset.service;
      if (service) {
        viewServiceLogs(service);
        return;
      }
      // Otherwise derive from the error entry
      const idx = parseInt(btn.dataset.errorIndex, 10);
	      if (!isNaN(idx) && idx >= 0 && idx < state.recentErrors.length) {
	        const err = state.recentErrors[idx];
	        const svc = err.service || (err.type === 'agent' ? 'cortex' : err.type === 'import' ? 'importer' : 'supervisor');
	        viewServiceLogs(svc, err.ts);
	      }
	    } else if (action === 'clear-error-filter') {
	      state.recentErrorsFilter = null;
	      updateErrorSummary();
	    }
	  });

  elements.importerGrid.addEventListener('click', (e) => {
    const btn = e.target.closest('[data-action="retry"]');
    if (!btn) return;
    const importId = btn.dataset.importId;
    if (!importId) return;
    const card = btn.closest('[data-key]');
    const errorEl = card?.querySelector('.activity-card-error');
    btn.disabled = true;
    btn.textContent = 'retrying...';
    window.apiJson('/app/health/api/retry-import', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ import_id: importId })
    })
      .then(() => {
        btn.textContent = 'retry sent';
        btn.style.color = '#9ca3af';
        if (errorEl) {
          errorEl.textContent = '';
        }
      })
      .catch((err) => {
        window.logError(err, { context: 'health: retry-import failed' });
        btn.disabled = false;
        btn.textContent = 'retry';
        btn.style.color = '';
        if (errorEl) {
          errorEl.textContent = err.serverMessage || 'retry failed';
        }
      });
  });

  // Client-side elapsed time updater
  let elapsedTimer = null;
  function updateElapsed() {
    // Update agent elapsed times based on start timestamp
    let needsUpdate = false;
    const now = Date.now();
    state.agents.forEach((agent, id) => {
      if (agent.startTs && agent.event !== 'finish' && agent.event !== 'error') {
        agent.elapsed_seconds = Math.floor((now - agent.startTs) / 1000);
        needsUpdate = true;
      }
    });
    // Stop timer when no agents remain
    if (state.agents.size === 0) {
      clearInterval(elapsedTimer);
      elapsedTimer = null;
      return;
    }
    if (needsUpdate) {
      updateCortexGrid();
    }
  }

  function startElapsedTimer() {
    if (elapsedTimer || document.hidden) return;
    elapsedTimer = setInterval(updateElapsed, 1000);
  }

  function updateVitalsA11y() {
    const sections = document.querySelectorAll('.vitals-content > .vitals-section');
    const runningCount = state.services.size;
    const crashedCount = Array.from(state.crashed.values()).filter(info => info.phase !== 'backoff').length;
    const retryingCount = state.crashed.size - crashedCount;
    const serviceParts = [];
    if (runningCount > 0) serviceParts.push(runningCount + ' active');
    if (retryingCount > 0) serviceParts.push(retryingCount + ' retrying');
    if (crashedCount > 0) serviceParts.push(crashedCount + ' needs attention');
    sections[0]?.setAttribute('aria-label', 'Services: ' + (serviceParts.join(', ') || 'none'));

    sections[1]?.setAttribute('aria-label', 'Talents: ' + state.agentCount + ' running');
    sections[2]?.setAttribute('aria-label', 'Tasks: ' + state.tasks.length + ' active');

    const staleCount = state.health?.stale_heartbeats?.length || 0;
    let healthLabel = 'loading';
    if (staleCount > 0) {
      healthLabel = 'warning, ' + staleCount + ' service' + (staleCount === 1 ? '' : 's') + ' not responding';
    } else if (crashedCount > 0) {
      healthLabel = 'error, services need attention';
    } else if (retryingCount > 0) {
      healthLabel = 'warning, services retrying';
    } else if (state.health) {
      healthLabel = 'ok';
    }
    sections[3]?.setAttribute('aria-label', 'Health: ' + healthLabel);

    const queueEntries = Object.entries(state.queues).filter(([, count]) => count > 0);
    sections[4]?.setAttribute(
      'aria-label',
      'Queues: ' + (queueEntries.map(([cmd, count]) => cmd + ' ' + count).join(', ') || 'none')
    );

    sections[5]?.setAttribute(
      'aria-label',
      'Schedules: ' + (state.schedules.map(schedule => {
        const name = schedule.name || 'unnamed';
        const next = formatNextRun(schedule.next_run);
        return next ? name + ' next ' + next : name;
      }).join(', ') || 'none')
    );
  }

  // Update vitals bar
  function updateVitals() {
    if (connectError && !state.connected) {
      elements.vitalsStatus.textContent = '';
      const indicator = document.createElement('span');
      indicator.className = 'status-indicator crashed';
      elements.vitalsStatus.appendChild(indicator);
      const errText = document.createElement('span');
      errText.textContent = ' Connection error';
      elements.vitalsStatus.appendChild(errText);
      updateStatusSummary();
      return;
    }

    // Combine running and crashed services
    const allServices = [];

    // Running services first
    state.services.forEach((info, name) => {
      allServices.push({ name, info, crashed: false });
    });

    // Then crashed services
    state.crashed.forEach((info, name) => {
      allServices.push({ name, info, crashed: true });
    });

    if (allServices.length > 0) {
      const container = elements.serviceDots;
      const existingByKey = new Map();
      for (const child of Array.from(container.children)) {
        const key = child.getAttribute('data-service');
        if (key) {
          existingByKey.set(key, child);
        } else {
          container.removeChild(child);
        }
      }

      const newKeys = new Set(allServices.map(s => s.name));

      for (const [key, child] of existingByKey) {
        if (!newKeys.has(key)) container.removeChild(child);
      }

      for (const { name, info, crashed } of allServices) {
        let dot = existingByKey.get(name);
        if (!dot) {
          dot = document.createElement('div');
          dot.setAttribute('data-service', name);
          const indicator = document.createElement('span');
          dot.appendChild(indicator);
          const label = document.createElement('span');
          dot.appendChild(label);
          container.appendChild(dot);
        }
        const retrying = crashed && info.phase === 'backoff';
        const statusClass = retrying ? 'restarting' : (crashed ? 'crashed' : 'active');
        const statusLabel = retrying ? 'retrying' : statusClass;
        const restartInfo = retrying ? ' (retrying)' : '';
        dot.className = retrying ? 'service-dot restarting' : (crashed ? 'service-dot crashed' : 'service-dot');
        dot.children[0].className = 'status-indicator ' + statusClass;
        dot.children[0].setAttribute('aria-label', serviceName(name) + ': ' + statusLabel + restartInfo);
        dot.children[1].setAttribute('title', name);
        dot.children[1].textContent = serviceName(name) + restartInfo;
      }
    }

    // Agents count
    elements.agentsValue.firstElementChild.textContent = state.agentCount + ' running';

    // Tasks
    const taskCount = state.tasks.length;
    elements.tasksValue.firstElementChild.textContent = taskCount + ' active';

    // Health with stale heartbeat names
    if (state.health) {
      const staleHeartbeats = state.health.stale_heartbeats || [];
      const staleCount = staleHeartbeats.length;
      const hasCrashed = Array.from(state.crashed.values()).some(info => info.phase !== 'backoff');
      const hasRetrying = state.crashed.size > 0 && !hasCrashed;

      const hv = elements.healthValue;
      if (!hv.querySelector('.health-main')) {
        hv.textContent = '';
        const main = document.createElement('span');
        main.className = 'health-main';
        hv.appendChild(main);
        const staleList = document.createElement('span');
        staleList.className = 'stale-list';
        hv.appendChild(staleList);
      }
      const mainSpan = hv.children[0];
      const staleListSpan = hv.children[1];

      if (staleCount > 0) {
        const staleNames = staleHeartbeats.map(s => serviceName(s)).join(', ');
        mainSpan.textContent = staleCount + ' service' + (staleCount === 1 ? '' : 's') + ' not responding';
        mainSpan.style.color = '#f59e0b';
        staleListSpan.textContent = '(' + staleNames + ')';
        staleListSpan.style.display = '';
        updateVitalsStatus('warning');
      } else if (hasCrashed) {
        mainSpan.textContent = 'services need attention';
        mainSpan.style.color = '#f87171';
        staleListSpan.textContent = '';
        staleListSpan.style.display = 'none';
        updateVitalsStatus('error');
      } else if (hasRetrying) {
        mainSpan.textContent = 'services retrying';
        mainSpan.style.color = '#f59e0b';
        staleListSpan.textContent = '';
        staleListSpan.style.display = 'none';
        updateVitalsStatus('warning');
      } else {
        mainSpan.textContent = 'ok';
        mainSpan.style.color = '';
        staleListSpan.textContent = '';
        staleListSpan.style.display = 'none';
        updateVitalsStatus('ok');
      }
    }

    // Queues (always visible)
    const queueEntries = Object.entries(state.queues).filter(([, count]) => count > 0);
    if (queueEntries.length > 0) {
      let wrapper = elements.queuesValue.querySelector('.vitals-chips');
      if (!wrapper) {
        elements.queuesValue.textContent = '';
        wrapper = document.createElement('div');
        wrapper.className = 'vitals-chips';
        elements.queuesValue.appendChild(wrapper);
      }
      const existingByKey = new Map();
      for (const child of Array.from(wrapper.children)) {
        existingByKey.set(child.getAttribute('data-key'), child);
      }
      const newKeys = new Set(queueEntries.map(([cmd]) => cmd));
      for (const [key, child] of existingByKey) {
        if (!newKeys.has(key)) wrapper.removeChild(child);
      }
      for (const [cmd, count] of queueEntries) {
        let chip = existingByKey.get(cmd);
        if (!chip) {
          chip = document.createElement('span');
          chip.className = 'vitals-chip';
          chip.setAttribute('data-key', cmd);
          wrapper.appendChild(chip);
        }
        chip.textContent = cmd + ': ' + count;
      }
    } else {
      elements.queuesValue.textContent = '—';
    }

    // Schedules (always visible, enriched)
    if (state.schedules.length > 0) {
      let wrapper = elements.schedulesValue.querySelector('.vitals-chips');
      if (!wrapper) {
        elements.schedulesValue.textContent = '';
        wrapper = document.createElement('div');
        wrapper.className = 'vitals-chips';
        elements.schedulesValue.appendChild(wrapper);
      }
      const existingByKey = new Map();
      for (const child of Array.from(wrapper.children)) {
        existingByKey.set(child.getAttribute('data-key'), child);
      }
      const newKeys = new Set(state.schedules.map(s => s.name || 'unnamed'));
      for (const [key, child] of existingByKey) {
        if (!newKeys.has(key)) wrapper.removeChild(child);
      }
      for (const s of state.schedules) {
        const key = s.name || 'unnamed';
        const next = formatNextRun(s.next_run);
        const due = s.due ? `<span aria-hidden="true">${(window.ConveyIcons?.svg('alarm-clock') || '')}</span>` : '';
        let chip = existingByKey.get(key);
        if (!chip) {
          chip = document.createElement('span');
          chip.className = 'vitals-chip';
          chip.setAttribute('data-key', key);
          wrapper.appendChild(chip);
        }
        chip.innerHTML = escapeHtml(key) + due + (next ? ' · ' + escapeHtml(next) : '');
        chip.setAttribute('title', s.every || '');
      }
    } else {
      elements.schedulesValue.textContent = '—';
    }

    updateVitalsA11y();
    updateStatusSummary();
  }

  function updateVitalsStatus(status) {
    const el = elements.vitalsStatus;
    el.classList.remove('warning', 'error');

    // Ensure stable child structure: [indicator, text, severity-label]
    if (!el.querySelector('.status-indicator') || el.children.length < 3 || !el.querySelector('.severity-label')) {
      el.textContent = '';
      const indicator = document.createElement('span');
      indicator.className = 'status-indicator';
      el.appendChild(indicator);
      const text = document.createElement('span');
      el.appendChild(text);
      const severity = document.createElement('span');
      severity.className = 'severity-label';
      el.appendChild(severity);
    }

    const indicator = el.children[0];
    const text = el.children[1];
    const severity = el.children[2];

    if (status === 'ok') {
      indicator.className = 'status-indicator active';
      indicator.setAttribute('aria-label', 'System status: healthy');
      text.textContent = 'all systems go';
      severity.textContent = 'healthy';
    } else if (status === 'warning') {
      indicator.className = 'status-indicator restarting';
      indicator.setAttribute('aria-label', 'System status: warning');
      text.textContent = 'some services slow to respond';
      severity.textContent = 'warning';
      el.classList.add('warning');
    } else if (status === 'error') {
      indicator.className = 'status-indicator crashed';
      indicator.setAttribute('aria-label', 'System status: error');
      text.textContent = 'services need attention';
      severity.textContent = 'error';
      el.classList.add('error');
    }
  }

  // Update observe mode badge
  function updateObserveMode(displayedClient = null) {
    if (state.clients.size === 0) {
      elements.observeModeBadge.className = 'health-badge idle';
      elements.observeModeLabel.textContent = 'idle';
      return;
    }

    const mode = (displayedClient || state.clients.get(state.localHost))?.mode;
    const badge = elements.observeModeBadge;
    const label = elements.observeModeLabel;

    if (mode === 'screencast') {
      badge.className = 'health-badge recording';
      label.textContent = 'taking in your screen';
    } else if (mode === 'tmux') {
      badge.className = 'health-badge tmux';
      label.textContent = 'terminal sessions';
    } else if (mode === 'idle') {
      badge.className = 'health-badge idle';
      label.textContent = 'idle';
    } else {
      badge.className = 'health-badge idle';
      label.textContent = 'idle';
    }
  }

  // Update observe card
  function updateObserve() {
    if (state.clients.size === 0) {
      elements.observeEmpty.classList.remove('hidden');
      elements.observeContent.classList.add('hidden');
      elements.observeSourceNote.classList.add('hidden');
      elements.observeSourceNote.textContent = '';
      updateObserveMode();
      updateStatusSummary();
      return;
    }
    elements.observeEmpty.classList.add('hidden');
    elements.observeContent.classList.remove('hidden');

    const confirmedPrimary = state.localHost ? state.clients.get(state.localHost) : null;
    const fallbackEntry = Array.from(state.clients.entries())
      .filter(([stream]) => !stream.endsWith('.tmux'))
      .sort((a, b) => (b[1].lastSeen || 0) - (a[1].lastSeen || 0))[0] || null;
    const displayedStream = confirmedPrimary ? state.localHost : (fallbackEntry ? fallbackEntry[0] : null);
    const primary = confirmedPrimary || (fallbackEntry ? fallbackEntry[1] : null);
    const tmux = displayedStream ? state.clients.get(displayedStream + '.tmux') : null;
    const confirmedLocal = Boolean(state.localHost && confirmedPrimary && displayedStream === state.localHost);
    if (!confirmedLocal && displayedStream) {
      elements.observeSourceNote.textContent = state.localHost
        ? `this host's stream isn't reporting yet — showing ${displayedStream}`
        : `this host is unknown — showing ${displayedStream}`;
      elements.observeSourceNote.classList.remove('hidden');
    } else {
      elements.observeSourceNote.textContent = '';
      elements.observeSourceNote.classList.add('hidden');
    }
    const channels = [
      {
        statusEl: elements.screencastStatus,
        detailEl: elements.screencastDetail,
        idleText: 'Not recording',
        extract: () => {
          if (!primary?.screencast) return null;
          const recording = primary.screencast.recording;
          if (!recording) return { status: 'Not recording' };
          const streams = primary.screencast.streams || [];
          const elapsed = primary.screencast.window_elapsed_seconds || 0;
          const streamCount = streams.length;
          const displayLabel = streamCount === 1 ? 'display' : 'displays';
          const mins = Math.max(1, Math.round(elapsed / 60));
          return {
            status: `taking in ${streamCount} ${displayLabel}, ~${mins} min`,
            detail: streamCount > 0
              ? streams.map(s => `${s.position || 'unknown'} ${s.connector || 'unknown'}`).join(', ')
              : '',
          };
        },
      },
      {
        statusEl: elements.tmuxStatus,
        detailEl: elements.tmuxDetail,
        idleText: 'Not capturing',
        extract: () => {
          if (!tmux?.tmux) return null;
          if (!tmux.tmux.capturing) return { status: 'Not capturing' };
          const captures = tmux.tmux.captures || 0;
          const sessions = tmux.tmux.sessions || [];
          const elapsed = tmux.tmux.window_elapsed_seconds || 0;
          const mins = Math.max(1, Math.round(elapsed / 60));
          return {
            status: `observing (${captures} snapshots, ~${mins} min)`,
            detail: sessions.length > 0 ? sessions.join(', ') : '',
          };
        },
      },
      {
        statusEl: elements.audioStatus,
        detailEl: elements.audioDetail,
        idleText: 'Listening (quiet)',
        extract: () => {
          if (!primary?.audio) return null;
          const hits = primary.audio.threshold_hits || 0;
          const willSave = primary.audio.will_save ? ' · saving' : '';
          return {
            status: hits > 0
              ? `Listening (${hits} sound${hits === 1 ? '' : 's'} detected)${willSave}`
              : 'Listening (quiet)',
          };
        },
      },
      {
        statusEl: elements.activityStatus,
        detailEl: elements.activityDetail,
        idleText: 'idle',
        extract: () => {
          if (!primary?.activity) return null;
          const idleMs = primary.activity.idle_time_ms || 0;
          if (primary.activity.power_save) return { status: 'Power saving' };
          if (primary.activity.screen_locked) return { status: 'Screen locked' };
          if (primary.activity.sink_muted) return { status: 'Audio muted' };
          return { status: `idle: ${Math.floor(idleMs/1000)}s` };
        },
      },
      {
        statusEl: elements.describeStatus,
        detailEl: elements.describeDetail,
        idleText: 'idle',
        extract: () => {
          if (!primary?.describe) return null;
          return { processor: primary.describe };
        },
      },
      {
        statusEl: elements.transcribeStatus,
        detailEl: elements.transcribeDetail,
        idleText: 'idle',
        extract: () => {
          if (!primary?.transcribe) return null;
          return { processor: primary.transcribe };
        },
      },
    ];

    updateObserveMode(primary);

    channels.forEach(ch => {
      const result = ch.extract();
      if (!result) {
        ch.statusEl.textContent = ch.idleText;
        ch.detailEl.textContent = '';
      } else if (result.processor) {
        // Describe/transcribe processor logic
        const p = result.processor;
        const running = p.running || [];
        const isRunning = running.length > 0;
        const queued = p.queued?.length || 0;
        if (isRunning && queued > 0) {
          ch.statusEl.textContent = running.length > 1
            ? `running (${running.length}, +${queued} queued)`
            : `running (+${queued} queued)`;
        } else if (isRunning) {
          ch.statusEl.textContent = running.length > 1 ? `running (${running.length})` : 'running';
        } else if (queued > 0) {
          ch.statusEl.textContent = `queued: ${queued}`;
        } else {
          ch.statusEl.textContent = 'idle';
        }
        if (isRunning && running[0]?.file) {
          ch.detailEl.textContent = truncate(running[0].file.split('/').pop(), 30);
        } else {
          ch.detailEl.textContent = '';
        }
      } else {
        ch.statusEl.textContent = result.status;
        ch.detailEl.textContent = result.detail || '';
      }
      ch.statusEl.classList.toggle('idle', ch.statusEl.textContent === ch.idleText);
    });

    updateStatusSummary();
  }

  // Update clients
  function updateClients() {
    if (state.clients.size === 0) {
      elements.clientsCard.classList.add('hidden');
      return;
    }

    elements.clientsCard.classList.remove('hidden');

    const byHost = new Map();
    for (const [stream, data] of state.clients) {
      const host = data.host || stream;
      if (!byHost.has(host)) byHost.set(host, []);
      byHost.get(host).push({ stream, data });
    }

    const now = Date.now();

    elements.clientsGrid.innerHTML = '';
    for (const [host, streams] of byHost) {
      const card = document.createElement('div');
      card.className = 'client-host-card';

      const anyActive = streams.some(({ data }) => (now - data.lastSeen) < STALE_MS);
      if (anyActive) card.classList.add('active');
      if (!anyActive) card.classList.add('stale');

      const nameEl = document.createElement('div');
      nameEl.className = 'client-host-name';
      nameEl.textContent = host;
      const statusIcon = document.createElement('span');
      statusIcon.className = 'status-indicator ' + (anyActive ? 'active' : 'stale');
      statusIcon.setAttribute('aria-label', anyActive ? 'active' : 'stale');
      nameEl.prepend(statusIcon);
      nameEl.style.display = 'flex';
      nameEl.style.alignItems = 'center';
      nameEl.style.gap = '0.4em';
      card.appendChild(nameEl);

      const platform = streams[0]?.data?.platform || '';
      if (platform) {
        const platEl = document.createElement('div');
        platEl.className = 'client-host-platform';
        platEl.textContent = platform;
        card.appendChild(platEl);
      }

      for (const { stream, data } of streams) {
        const row = document.createElement('div');
        row.className = 'client-stream-row';

        const dotIdx = stream.indexOf('.');
        const qualifier = dotIdx >= 0 ? stream.slice(dotIdx + 1) : 'desktop';

        const stale = (now - data.lastSeen) >= STALE_MS;
        if (stale) row.classList.add('stale');

        const qualEl = document.createElement('div');
        qualEl.className = 'client-stream-qualifier';
        qualEl.textContent = qualifier;
        row.appendChild(qualEl);

        const modeEl = document.createElement('div');
        modeEl.className = 'client-stream-mode';
        modeEl.textContent = data.mode || '—';
        row.appendChild(modeEl);

        if (stale) {
          const badgeEl = document.createElement('span');
          badgeEl.className = 'stale-badge';
          badgeEl.setAttribute('aria-label', 'stale — not responding');
          badgeEl.textContent = 'stale';
          row.appendChild(badgeEl);
        }

        card.appendChild(row);
      }

      elements.clientsGrid.appendChild(card);
    }
  }

	  function monthDay(ms) {
	    return new Date(ms).toLocaleDateString('en-US', { month: 'short', day: 'numeric' }).toLowerCase();
	  }

	  function registeredClientMeta(client) {
	    const lastCapture = client.last_accepted_ingest_at && Date.parse(client.last_accepted_ingest_at);
	    if (Number.isFinite(lastCapture)) {
	      const deltaMs = Date.now() - lastCapture;
	      if (deltaMs < 0) return 'last added from future';
	      return `last added ${relativeTime(deltaMs)} ago`;
	    }
	    const lastSeen = client.last_seen_at && Date.parse(client.last_seen_at);
	    if (!Number.isFinite(lastSeen)) return 'no capture yet';
	    const deltaMs = Date.now() - lastSeen;
	    if (deltaMs < 0) return 'last seen from future';
	    return `last reported ${relativeTime(deltaMs)} ago`;
	  }

	  function requestBacklogReprocess(button) {
	    const row = button.closest('.backlog-row');
	    if (!row) return;
	    const day = button.dataset.day;
	    const flavor = button.dataset.flavor;
	    if (flavor === 'from-scratch' && !window.confirm(button.dataset.confirm)) {
	      return;
	    }
	    const buttons = Array.from(row.querySelectorAll('.backlog-action'));
	    const statusEl = row.querySelector('.backlog-action-status');
	    buttons.forEach((rowButton) => { rowButton.disabled = true; });
	    if (statusEl) statusEl.textContent = '';
	    window.apiJson('/app/health/api/reprocess', {
	      method: 'POST',
	      headers: { 'Content-Type': 'application/json' },
	      body: JSON.stringify({ day, flavor })
	    })
	      .then((result) => {
	        if (!statusEl) return;
	        if (result && (result.status === 'already_complete' || result.status === 'held_by_backoff')) {
	          statusEl.textContent = result.message || '';
	          buttons.forEach((rowButton) => { rowButton.disabled = false; });
	          return;
	        }
	        statusEl.textContent = backlogCopy.queued_feedback || '';
	      })
	      .catch((err) => {
	        buttons.forEach((rowButton) => { rowButton.disabled = false; });
	        window.logError(err, { context: 'health: reprocess failed' });
	        if (statusEl) {
	          statusEl.textContent = err && err.serverMessage ? err.serverMessage : 'try again';
	        }
	      });
	  }

	  function wireBacklogReprocessActions() {
	    const host = document.getElementById('backlogNeedsHand');
	    if (!host) return;
	    host.querySelectorAll('.backlog-action').forEach((button) => {
	      if (button.dataset.reprocessBound === 'true') return;
	      button.dataset.reprocessBound = 'true';
	      button.addEventListener('click', () => requestBacklogReprocess(button));
	    });
	  }

  function renderRegisteredClients(clients) {
    if (!clients || clients.length === 0) {
      elements.registeredClientsCard.classList.add('hidden');
      elements.registeredClientsStrip.innerHTML = '';
      return;
    }

    elements.registeredClientsCard.classList.remove('hidden');
    elements.registeredClientsStrip.innerHTML = '';
    for (const client of clients) {
      let stateClass = ['connected', 'stale', 'disconnected'].includes(client.state)
        ? client.state
        : 'disconnected';
      let labelText = client.capture_state || client.label || 'unknown';
      if (client.capture_state === 'active') stateClass = 'connected';
      if (client.capture_state === 'stale') stateClass = 'stale';
      if (client.capture_state === 'offline') stateClass = 'disconnected';
      if (client.failing) {
        stateClass = 'failing';
        labelText = 'failing';
      } else if (client.capture_state === 'no_capture') {
        labelText = 'no capture yet';
      } else if (client.capture_state === 'unknown') {
        labelText = 'capture unknown';
      }
      const row = document.createElement('div');
      row.className = 'registered-client-row';

      const nameEl = document.createElement('span');
      nameEl.className = 'registered-client-name';
      nameEl.textContent = client.display_label || client.device_label || client.cid_short || client.cid || 'unnamed device';
      row.appendChild(nameEl);

      const labelEl = document.createElement('span');
      labelEl.className = `registered-client-label ${stateClass}`;
      labelEl.textContent = labelText;
      row.appendChild(labelEl);

      if (client.failing && client.ingest_rejection) {
        const rej = client.ingest_rejection;
        const parts = [];
        if (typeof rej.active_count === 'number' && isFinite(rej.active_count)
            && typeof rej.first === 'string' && Number.isFinite(Date.parse(rej.first))) {
          parts.push(rej.active_count + ' rejected since ' + monthDay(Date.parse(rej.first)));
        }
        if (rej.reason_code) parts.push(rej.reason_code);
        if (parts.length) {
          const detailEl = document.createElement('span');
          detailEl.className = 'registered-client-detail';
          detailEl.textContent = parts.join(' · ');
          row.appendChild(detailEl);
        }
        const recoveryEl = document.createElement('span');
        recoveryEl.className = 'registered-client-recovery';
        recoveryEl.textContent = 'update or restart the solstone app on ' + (client.display_label || client.device_label || 'that device');
        row.appendChild(recoveryEl);
      }

      const metaEl = document.createElement('span');
      metaEl.className = 'registered-client-meta';
      metaEl.textContent = registeredClientMeta(client);
      row.appendChild(metaEl);

      const skewEl = document.createElement('span');
      skewEl.className = 'registered-client-skew' + (client.clock_skew ? '' : ' hidden');
      skewEl.textContent = 'clock skew';
	      row.appendChild(skewEl);

	      elements.registeredClientsStrip.appendChild(row);
    }
  }

  async function loadRegisteredClients() {
    try {
      const response = await fetch('/app/network/api/clients');
      if (!response.ok) throw new Error(`HTTP ${response.status}`);
      const payload = await response.json();
      renderRegisteredClients(payload?.clients || []);
    } catch (err) {
      console.warn('Failed to load registered clients:', err);
    }
  }

  // Update cortex grid
  function updateCortexGrid() {
    const activeAgents = Array.from(state.agents.values()).filter(a => a.event !== 'finish' && a.event !== 'error');

    if (activeAgents.length === 0) {
      elements.cortexSection.classList.add('hidden');
      updateAllQuiet();
      updateStatusSummary();
      return;
    }

    elements.cortexSection.classList.remove('hidden');

    const container = elements.cortexGrid;
    const existingByKey = new Map();
    for (const child of Array.from(container.children)) {
      const k = child.getAttribute('data-key');
      if (k) existingByKey.set(k, child);
    }
    const newKeys = new Set(activeAgents.map(a => a.use_id));
    for (const [k, child] of existingByKey) {
      if (!newKeys.has(k)) container.removeChild(child);
    }
    for (const agent of activeAgents) {
      const key = agent.use_id;
      let card = existingByKey.get(key);
      if (!card) {
        card = document.createElement('div');
        card.className = 'activity-card agent-active';
        card.setAttribute('data-key', key);
        const idEl = document.createElement('div');
        idEl.className = 'activity-card-id';
        card.appendChild(idEl);
        const nameEl = document.createElement('div');
        nameEl.className = 'activity-card-name';
        card.appendChild(nameEl);
        const stateEl = document.createElement('div');
        stateEl.className = 'activity-card-state';
        card.appendChild(stateEl);
        const elapsedEl = document.createElement('div');
        elapsedEl.className = 'activity-card-elapsed';
        card.appendChild(elapsedEl);
        const providerEl = document.createElement('div');
        providerEl.className = 'activity-card-provider';
        card.appendChild(providerEl);
        container.appendChild(card);
      }
      const stateLabel = agent.event === 'thinking' ? 'Thinking...' :
                        (agent.event === 'tool_start' || agent.event === 'tool_end') ? 'working...' : 'running...';
      const elapsed = agent.elapsed_seconds ? formatElapsed(agent.elapsed_seconds) : '0s';
      card.children[0].textContent = '...' + getAgentId(agent.use_id);
      card.children[1].textContent = agent.name || 'default';
      card.children[2].textContent = stateLabel;
      card.children[3].textContent = elapsed;
      card.children[4].textContent = agent.provider || 'unknown';
    }
    updateAllQuiet();
    updateStatusSummary();
  }

  // Update importer grid
  function updateImporterGrid() {
    const activeImports = Array.from(state.imports.values()).filter(i => i.event !== 'completed');

    if (activeImports.length === 0) {
      elements.importerSection.classList.add('hidden');
      updateAllQuiet();
      updateStatusSummary();
      return;
    }

    elements.importerSection.classList.remove('hidden');

    const container = elements.importerGrid;
    const existingByKey = new Map();
    for (const child of Array.from(container.children)) {
      const k = child.getAttribute('data-key');
      if (k) existingByKey.set(k, child);
    }
    const newKeys = new Set(activeImports.map(i => i.import_id));
    for (const [k, child] of existingByKey) {
      if (!newKeys.has(k)) container.removeChild(child);
    }
    for (const imp of activeImports) {
      const key = imp.import_id;
      const isError = imp.event === 'error';
      let card = existingByKey.get(key);
      if (!card) {
        card = document.createElement('div');
        card.setAttribute('data-key', key);
        const idEl = document.createElement('div');
        idEl.className = 'activity-card-id';
        card.appendChild(idEl);
        const nameEl = document.createElement('div');
        nameEl.className = 'activity-card-name';
        card.appendChild(nameEl);
        const stateEl = document.createElement('div');
        stateEl.className = 'activity-card-state';
        card.appendChild(stateEl);
        const errorEl = document.createElement('div');
        errorEl.className = 'activity-card-error';
        card.appendChild(errorEl);
        const retryWrap = document.createElement('div');
        retryWrap.style.marginTop = '0.3em';
        const retryBtn = document.createElement('button');
        retryBtn.setAttribute('data-action', 'retry');
        retryBtn.className = 'import-retry-btn';
        retryBtn.textContent = 'retry';
        retryWrap.appendChild(retryBtn);
        card.appendChild(retryWrap);
        const progressWrap = document.createElement('div');
        progressWrap.className = 'activity-card-progress';
        const progressBar = document.createElement('div');
        progressBar.className = 'activity-card-progress-bar';
        progressWrap.appendChild(progressBar);
        card.appendChild(progressWrap);
        const elapsedEl = document.createElement('div');
        elapsedEl.className = 'activity-card-elapsed';
        card.appendChild(elapsedEl);
        const providerEl = document.createElement('div');
        providerEl.className = 'activity-card-provider';
        card.appendChild(providerEl);
        container.appendChild(card);
      }

      const cardClass = isError ? 'import-error' : 'import-active';
      card.className = 'activity-card ' + cardClass;
      const progress = imp.stage === 'initialization' ? 25 :
                      imp.stage === 'transcribing' ? 50 :
                      imp.stage === 'segmenting' ? 75 : 90;
      const elapsed = imp.elapsed_ms ? formatDuration(imp.elapsed_ms) : '0s';
      const humanStage = imp.stage === 'initialization' ? 'Starting...' :
                         imp.stage === 'transcribing' ? 'Transcribing audio...' :
                         imp.stage === 'segmenting' ? 'Organizing segments...' : 'Processing...';

      if (isError && imp.error) {
        card.children[3].textContent = truncate(imp.error, 40);
        card.children[3].style.display = '';
        card.children[4].style.display = '';
        card.children[4].querySelector('button').setAttribute('data-import-id', imp.import_id);
      } else {
        card.children[3].textContent = '';
        card.children[3].style.display = 'none';
        card.children[4].style.display = 'none';
      }

      card.children[0].textContent = '...' + getAgentId(imp.import_id);
      card.children[1].textContent = truncate(imp.input_file || 'import', 20);
      card.children[2].textContent = (isError ? '! ' : '') + humanStage;
      card.children[2].setAttribute('data-internal-stage', imp.stage || 'processing');
      card.children[5].children[0].style.width = progress + '%';
      card.children[6].textContent = elapsed;
      card.children[7].textContent = imp.file_type || 'unknown';
    }
    updateAllQuiet();
    updateStatusSummary();
  }

  function handleThinkEvent(msg) {
    if (msg.event === 'started') {
      state.thinkActive = true;
      state.think = { mode: msg.mode, day: msg.day };
      updateThinkCard();
    } else if (msg.event === 'status') {
      state.thinkActive = true;
      state.think = { ...state.think, ...msg };
      updateThinkCard();
    } else if (msg.event === 'completed') {
      state.thinkActive = false;
      state.think = null;
      updateThinkCard();
    }
  }

  function updateThinkCard() {
    if (!state.thinkActive || !state.think) {
      elements.thinkCard.classList.add('hidden');
      updateAllQuiet();
      updateStatusSummary();
      return;
    }

    elements.thinkCard.classList.remove('hidden');
    const d = state.think;

    // Info fields
    renderInfoItems(elements.thinkInfo, [
      { label: 'mode', value: d.mode || null },
      { label: 'day', value: d.day || null },
      { label: 'facet', value: d.facet || null },
      { label: 'segment', value: d.segment || null },
    ]);

    // Progress bars
    const progressItems = [];
    if (d.agents_total > 0) {
      progressItems.push({ label: 'Talents: ' + (d.agents_completed || 0) + ' / ' + d.agents_total, pct: Math.round((d.agents_completed || 0) / d.agents_total * 100) });
    }
    if (d.segments_total > 0) {
      progressItems.push({ label: 'Segments: ' + (d.segments_completed || 0) + ' / ' + d.segments_total, pct: Math.round((d.segments_completed || 0) / d.segments_total * 100) });
    }
    const progContainer = elements.thinkProgress;
    while (progContainer.children.length > progressItems.length) {
      progContainer.removeChild(progContainer.lastChild);
    }
    progressItems.forEach((item, i) => {
      let wrap = progContainer.children[i];
      if (!wrap) {
        wrap = document.createElement('div');
        wrap.className = 'think-progress';
        const label = document.createElement('div');
        label.className = 'think-progress-label';
        wrap.appendChild(label);
        const bar = document.createElement('div');
        bar.className = 'think-progress-bar';
        const fill = document.createElement('div');
        fill.className = 'think-progress-fill';
        bar.appendChild(fill);
        wrap.appendChild(bar);
        progContainer.appendChild(wrap);
      }
      wrap.children[0].textContent = item.label;
      wrap.children[1].children[0].style.width = item.pct + '%';
    });

    // Current agents
    if (d.current_agents && d.current_agents.length > 0) {
      elements.thinkAgents.textContent = 'running: ' + d.current_agents.join(', ');
    } else {
      elements.thinkAgents.textContent = '';
    }

    updateAllQuiet();
    updateStatusSummary();
  }

  const LOG_BUFFER_SIZE = 50;

  // mirror of solstone/apps/health/log_classifier.py — keep in sync
  function classifyLogLevel(stream, line) {
    const normalizedStream = (stream || '').trim().toLowerCase();
    const text = line || '';

    if (normalizedStream === 'stderr' && /\b(?:ERROR|CRITICAL|FATAL)\b/.test(text)) return 'error';
    if (/^Traceback \(most recent call last\):/.test(text)) return 'error';
    if (/: error while loading shared libraries:/.test(text)) return 'error';
    if (/\bsolstone isn't running\b/.test(text)) return 'error';
    if (/\b(?:ERROR|CRITICAL|FATAL)\b/.test(text)) return 'error';

    if (/\b(?:WARNING|WARN)\b/.test(text)) return 'warning';
    if (/\bUserWarning\b/.test(text)) return 'warning';
    if (/^\S+\s+W\s+/.test(text)) return 'warning';
    if (/\b(?:not reachable|Connection refused)\b/.test(text)) return 'warning';

    if (/\bDEBUG\b/.test(text)) return 'debug';

    if (/\bINFO\b/.test(text)) return 'info';
    if (/^\S+\s+I\s+/.test(text)) return 'info';

    return 'info';
  }

  function formatTemplate(template, values) {
    return template.replace(/\{(service|n|errors)\}/g, (_, key) => String(values[key]));
  }

  function stripLogPrefix(line) {
    return line.replace(/^\S+\s+\[[^\]]+\]\s+/, '');
  }

  function logLevelMatchesFilter(level) {
    if (state.logLevelFilter === 'error') return level === 'error';
    if (state.logLevelFilter === 'warning') return level === 'error' || level === 'warning';
    if (state.logLevelFilter === 'info') return level === 'error' || level === 'warning' || level === 'info';
    return true;
  }

  function formatLogTime(ts) {
    const date = new Date(ts);
    if (Number.isNaN(date.getTime())) return '';
    return [date.getHours(), date.getMinutes(), date.getSeconds()]
      .map(part => String(part).padStart(2, '0'))
      .join(':');
  }

  function filteredLogRecords(records) {
    const streamFilter = elements.logStreamFilter.value;
    return records.filter((record) => {
      const streamMatch = streamFilter === 'all' || streamFilter === record.stream;
      if (!streamMatch) return false;
      return logLevelMatchesFilter(classifyLogLevel(record.stream, record.line));
    });
  }

  function countErrorRecords(records) {
    return records.filter(record => classifyLogLevel(record.stream, record.line) === 'error').length;
  }

  function toggleLogServiceCollapse(service) {
    const collapsed = state.logCollapsedServices.get(service) === true;
    state.logCollapsedServices.set(service, !collapsed);
    renderLogs();
  }

  function makeLogServiceHeader(service, records) {
    const collapsed = state.logCollapsedServices.get(service) === true;
    const header = document.createElement('div');
    header.className = 'logs-service-header';
    header.setAttribute('data-svc', service);
    header.setAttribute('role', 'button');
    header.setAttribute('tabindex', '0');
    header.setAttribute('aria-expanded', String(!collapsed));
    if (collapsed) {
      header.textContent = formatTemplate(HEALTH_LOGS_COPY.LOGS_SERVICE_COLLAPSED, {
        service: serviceName(service),
        n: records.length,
        errors: countErrorRecords(records),
      });
    } else {
      header.textContent = '── ' + serviceName(service) + ' ──';
    }
    header.addEventListener('click', () => toggleLogServiceCollapse(service));
    header.addEventListener('keydown', (event) => {
      if (event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();
        toggleLogServiceCollapse(service);
      }
    });
    return header;
  }

  function appendLogLine(parent, service, record) {
    const level = classifyLogLevel(record.stream, record.line);
    if (!logLevelMatchesFilter(level)) return false;

    const line = document.createElement('div');
    line.className = 'logs-line logs-level-' + level;
    if (state.logCollapsedServices.get(service) === true) {
      line.classList.add('logs-svc-collapsed');
    }
    if (record.ts) {
      line.dataset.ts = String(record.ts);
      line.dataset.hhmmss = formatLogTime(record.ts);
    }
    line.appendChild(document.createTextNode(record.line));
    parent.appendChild(line);
    return true;
  }

  function applyPendingLogAnchor() {
    const anchor = state.pendingLogAnchor;
    if (!anchor) return;
    state.pendingLogAnchor = null;
    const lines = Array.from(elements.logsViewport.querySelectorAll('.logs-line[data-ts]'));
    if (lines.length === 0) return;
    let best = lines[0];
    let bestDelta = Math.abs(Number(best.dataset.ts) - anchor);
    for (const line of lines.slice(1)) {
      const delta = Math.abs(Number(line.dataset.ts) - anchor);
      if (delta < bestDelta) {
        best = line;
        bestDelta = delta;
      }
    }
    best.classList.add('log-row-anchored');
    best.scrollIntoView({ behavior: 'smooth', block: 'center' });
    setTimeout(() => best.classList.remove('log-row-anchored'), 1600);
  }

  function handleLogsEvent(msg) {
    if (msg.event !== 'line') return;

    const name = msg.name || 'unknown';
    const record = { ts: msg.ts || Date.now(), stream: msg.stream || 'stdout', line: msg.line || '' };
    state.logTotalCount++;
    state.lastLogTs = record.ts;

    // Check if this is a new service before adding to buffer
    const isNew = !state.serviceLogs.has(name);

    // Buffer per service
    if (isNew) {
      state.serviceLogs.set(name, []);
    }
    const buf = state.serviceLogs.get(name);
    buf.push(record);
    if (buf.length > LOG_BUFFER_SIZE) buf.shift();

    if (classifyLogLevel(record.stream, record.line) === 'error') {
      window.AppServices?.quietNotifs?.add({
        source: name,
        message: stripLogPrefix(record.line),
        ts: record.ts
      });
      elements.logsAnnouncer.textContent = stripLogPrefix(record.line);
      state.logErrorCount++;
      elements.logErrorBadge.textContent = state.logErrorCount + ' error' + (state.logErrorCount > 1 ? 's' : '');
      elements.logErrorBadge.classList.remove('hidden');
      if (state.logsCollapsed) {
        state.logsCollapsed = false;
        document.querySelector('.logs-card').classList.remove('logs-collapsed');
        elements.logsCollapseIndicator.textContent = '▼ hide';
        document.querySelector('.logs-header').setAttribute('aria-expanded', 'true');
        renderLogs();
        state.lastLogFilter = null;
      }
    }

    // Update service filter dropdown if new service
    if (isNew) {
      const opt = document.createElement('option');
      opt.value = name;
      opt.textContent = serviceName(name);
      elements.logServiceFilter.appendChild(opt);
    }

    renderLogs(name, record);
    updateLogsBadge();
  }

  function renderLogs(newService, newRecord) {
    if (state.deepLinkMode) return;
    const serviceFilter = elements.logServiceFilter.value;
    const viewport = elements.logsViewport;
    const filterKey = serviceFilter + ':' + elements.logStreamFilter.value + ':' + state.logLevelFilter;
    const atBottom = isAtBottom(viewport);

    // Incremental append: when following, filters unchanged, and we have a new record
    if (newRecord && state.lastLogFilter === filterKey && viewport.children.length > 0) {
      const serviceMatch = serviceFilter === 'all' || serviceFilter === newService;
      const streamMatch = elements.logStreamFilter.value === 'all' || elements.logStreamFilter.value === newRecord.stream;
      const levelMatch = logLevelMatchesFilter(classifyLogLevel(newRecord.stream, newRecord.line));
      if (serviceMatch && streamMatch && levelMatch) {
        if (state.logCollapsedServices.get(newService) === true) {
          renderLogs();
          return;
        }
        if (serviceFilter === 'all' && !viewport.querySelector(`[data-svc="${CSS.escape(newService)}"]`)) {
          viewport.appendChild(makeLogServiceHeader(newService, filteredLogRecords(state.serviceLogs.get(newService) || [])));
        }
        appendLogLine(viewport, newService, newRecord);
        if (state.logFollow && atBottom) {
          scrollLogsToBottom(viewport);
        }
        return;
      }
    }

    state.lastLogFilter = filterKey;

    // Check if user has scrolled away from bottom before updating
    const wasAtBottom = isAtBottom(viewport);

    const fragment = document.createDocumentFragment();
    const services = serviceFilter === 'all'
      ? Array.from(state.serviceLogs.keys()).sort()
      : (state.serviceLogs.has(serviceFilter) ? [serviceFilter] : []);

    for (const svc of services) {
      const lines = state.serviceLogs.get(svc) || [];
      const filtered = filteredLogRecords(lines);
      if (filtered.length === 0) continue;

      fragment.appendChild(makeLogServiceHeader(svc, filtered));

      for (const rec of filtered) {
        appendLogLine(fragment, svc, rec);
      }
    }

    viewport.textContent = '';
    viewport.appendChild(fragment);

    // Auto-scroll if following
    if (state.logFollow && wasAtBottom) {
      scrollLogsToBottom(viewport);
    }
    applyPendingLogAnchor();
  }

  // Event handlers by tract
  function handleSupervisorEvent(msg) {
    if (msg.event === 'status') {
      if (!state.connected) {
        state.connected = true;
        connectError = false;
      }

      // Update running services
      if (msg.services) {
        state.services.clear();
        msg.services.forEach(svc => {
          state.services.set(svc.name, svc);
        });
      }

      // Update crashed services (separate array)
      if (msg.crashed) {
        state.crashed.clear();
        msg.crashed.forEach(svc => {
          state.crashed.set(svc.name, svc);
        });
      } else {
        state.crashed.clear();
      }

      // Update tasks
      state.tasks = msg.tasks || [];

      // Update health
      state.health = {
        stale_heartbeats: msg.stale_heartbeats || []
      };

      // Update queues
      state.queues = msg.queues || {};
      state.schedules = msg.schedules || [];

      updateVitals();
    }
  }

  function handleCortexEvent(msg) {
    // Handle status event first (no use_id at top level)
    if (msg.event === 'status') {
      // Update agent count for vitals
      state.agentCount = msg.running_uses || 0;

      // Status event contains array of uses
      if (msg.uses) {
        // Clear uses not in status (they finished)
        const activeIds = new Set(msg.uses.map(a => a.use_id));
        state.agents.forEach((_, id) => {
          if (!activeIds.has(id)) {
            state.agents.delete(id);
          }
        });

        // Update/add uses from status
        msg.uses.forEach(agent => {
          const existing = state.agents.get(agent.use_id) || {};
          state.agents.set(agent.use_id, {
            ...existing,
            use_id: agent.use_id,
            name: agent.name,
            provider: agent.provider,
            elapsed_seconds: agent.elapsed_seconds,
            event: existing.event || 'thinking'
          });
        });
      }

      updateVitals();
      updateCortexGrid();
      return;
    }

    // Individual agent events require use_id
    const agentId = msg.use_id;
    if (!agentId) return;

    // Track start time for client-side elapsed updates
    const existing = state.agents.get(agentId) || {};
    const startTs = msg.event === 'start' ? msg.ts : existing.startTs;

    state.agents.set(agentId, {
      ...existing,
      use_id: agentId,
      name: msg.name || existing.name,
      provider: msg.provider || existing.provider,
      model: msg.model || existing.model,
      event: msg.event,
      ts: msg.ts,
      startTs: startTs
    });

    // Start elapsed timer when first agent appears
    if (state.agents.size > 0) {
      startElapsedTimer();
    }

    // Remove finished/error agents after delay
    if (msg.event === 'finish' || msg.event === 'error') {
      state.lastAgentFinishTs = msg.ts || Date.now();
      if (msg.event === 'error') {
        appendRecentError({
          type: 'agent',
          id: agentId,
          name: msg.name || existing.name || 'unknown',
          error: msg.error || window.CONVEY_COPY.UNKNOWN_ERROR,
          summary: msg.summary || msg.message || null,
          key: msg.key || msg.semantic_key || null,
          reason_code: msg.reason_code || null,
          provider: msg.provider || existing.provider || null,
          model: msg.model || existing.model || null,
          service: 'cortex',
          ts: Date.now()
        });
      }
      setTimeout(() => {
        state.agents.delete(agentId);
        updateCortexGrid();
      }, 5000);
    }

    updateCortexGrid();
  }

  function handleObserveEvent(msg) {
    if (!msg.stream) return;
    const existing = state.clients.get(msg.stream) || {};
    state.clients.set(msg.stream, { ...existing, ...msg, lastSeen: Date.now() });
    updateObserve();
    updateClients();
  }

  function handleImporterEvent(msg) {
    const importId = msg.import_id;
    if (!importId) return;

    if (msg.event === 'started') {
      state.imports.set(importId, {
        import_id: importId,
        input_file: msg.input_file,
        file_type: msg.file_type,
        stage: msg.stage,
        event: 'started',
        elapsed_ms: 0,
        lastSeen: Date.now()
      });
    } else if (msg.event === 'status') {
      const existing = state.imports.get(importId) || {};
      state.imports.set(importId, {
        ...existing,
        stage: msg.stage,
        elapsed_ms: msg.elapsed_ms,
        event: 'status',
        lastSeen: Date.now()
      });
    } else if (msg.event === 'completed') {
      const existing = state.imports.get(importId) || {};
      state.imports.set(importId, {
        ...existing,
        event: msg.event
      });

      // Remove after delay
      setTimeout(() => {
        state.imports.delete(importId);
        updateImporterGrid();
      }, 5000);
    } else if (msg.event === 'error') {
      const existing = state.imports.get(importId) || {};
      state.imports.set(importId, {
        ...existing,
        event: 'error',
        error: msg.error || window.CONVEY_COPY.UNKNOWN_ERROR,
        lastSeen: Date.now()
      });

      appendRecentError({
        type: 'import',
        id: importId,
        name: msg.input_file || existing.input_file || 'unknown',
        error: msg.error || window.CONVEY_COPY.UNKNOWN_ERROR,
        summary: msg.summary || msg.message || null,
        key: msg.key || msg.semantic_key || null,
        reason_code: msg.reason_code || null,
        provider: msg.provider || null,
        model: msg.model || null,
        service: 'importer',
        stage: existing.stage || msg.stage || 'unknown',
        ts: Date.now()
      });

      // Keep error visible longer
      setTimeout(() => {
        state.imports.delete(importId);
        updateImporterGrid();
      }, 15000);
    }

    updateImporterGrid();
  }

  // Main event handler
  function handleEvent(msg) {
    const eventTs = Date.now();
    state.lastEventTs = eventTs;
    recentEventTimestamps.push(eventTs);
    const cutoff = eventTs - 3600000;
    recentEventTimestamps = recentEventTimestamps.filter(ts => ts >= cutoff);
    if (recentEventTimestamps.length > 500) {
      recentEventTimestamps.splice(0, recentEventTimestamps.length - 500);
    }
    const tract = msg.tract;
    if (tract === 'supervisor') handleSupervisorEvent(msg);
    else if (tract === 'cortex') handleCortexEvent(msg);
    else if (tract === 'observe') handleObserveEvent(msg);
    else if (tract === 'importer') handleImporterEvent(msg);
    else if (tract === 'think') handleThinkEvent(msg);
    else if (tract === 'logs') handleLogsEvent(msg);
    updateStatusSummary();
  }

  // Log controls
  elements.logServiceFilter.addEventListener('change', () => {
    renderLogs();
  });
  elements.logLevelFilter.addEventListener('change', () => {
    state.logLevelFilter = elements.logLevelFilter.value;
    renderLogs();
  });
  elements.logStreamFilter.addEventListener('change', () => {
    renderLogs();
  });
  elements.logsViewport.addEventListener('scroll', () => {
    if (programmaticScroll) return;
    if (!isAtBottom(elements.logsViewport)) {
      state.logFollow = false;
      elements.logFollowBtn.classList.remove('active');
    }
  });
  function toggleLogsCollapse() {
    state.logsCollapsed = !state.logsCollapsed;
    const card = document.querySelector('.logs-card');
    const header = document.querySelector('.logs-header');
    card.classList.toggle('logs-collapsed', state.logsCollapsed);
    header.setAttribute('aria-expanded', String(!state.logsCollapsed));
    elements.logsCollapseIndicator.textContent = state.logsCollapsed ? '▶ show' : '▼ hide';
    if (!state.logsCollapsed) renderLogs();
  }
  document.querySelector('.logs-header').addEventListener('click', (e) => {
    if (state.deepLinkMode || e.target.closest('.logs-controls')) return;
    toggleLogsCollapse();
  });
  document.querySelector('.logs-header').addEventListener('keydown', (e) => {
    if (state.deepLinkMode || e.target.closest('.logs-controls')) return;
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      toggleLogsCollapse();
    }
  });
  elements.logFollowBtn.addEventListener('click', () => {
    state.logFollow = !state.logFollow;
    elements.logFollowBtn.classList.toggle('active', state.logFollow);
    if (state.logFollow) {
      scrollLogsToBottom(elements.logsViewport);
    }
  });
  elements.logClearBtn.addEventListener('click', () => {
    state.serviceLogs.clear();
    state.logTotalCount = 0;
    state.logErrorCount = 0;
    state.lastLogTs = null;
    state.lastLogFilter = null;
    state.logCollapsedServices.clear();
    elements.logErrorBadge.textContent = '';
    elements.logErrorBadge.classList.add('hidden');
    elements.logServiceFilter.innerHTML = '<option value="all">all services</option>';
    updateLogsBadge();
    renderLogs();
  });
  elements.vitalsCheckBtn.addEventListener('click', () => {
    elements.vitalsCheckBtn.textContent = 'checking...';
    elements.vitalsCheckBtn.disabled = true;
    fetch('/app/health/api/info')
      .then(r => r.json())
      .then(info => {
        state.localHost = info.hostname;
        brainSnapshot = info.brain || brainSnapshot;
        updateObserve();
        renderBrainHealth();
        updateStatusSummary();
        elements.vitalsCheckBtn.textContent = 'check now';
        elements.vitalsCheckBtn.disabled = false;
      })
      .catch(() => {
        elements.vitalsCheckBtn.textContent = 'check now';
        elements.vitalsCheckBtn.disabled = false;
      });
  });
  elements.logExportBtn.addEventListener('click', () => {
    const content = elements.logsViewport.textContent;
    const blob = new Blob([content], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'solstone-logs-' + new Date().toISOString().slice(0, 19).replace(/:/g, '') + '.txt';
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    URL.revokeObjectURL(url);
  });

  // Deep-link: display log file content if ?log= param is present
  const deepLinkLog = new URLSearchParams(window.location.search).get('log');
  if (deepLinkLog) {
    state.logsCollapsed = false;
    document.querySelector('.logs-card').classList.remove('logs-collapsed');
    elements.logsCollapseIndicator.textContent = '▼ hide';
    document.querySelector('.logs-header').setAttribute('aria-expanded', 'true');
    const viewport = elements.logsViewport;
    const logsCard = viewport.closest('.logs-card');
    const logsTitle = logsCard.querySelector('.logs-title');
    const logsControls = logsCard.querySelector('.logs-controls');

    // Hide dashboard cards and suppress live log rendering
    const dashboard = document.querySelector('.health-dashboard');
    dashboard.querySelectorAll('.vitals-bar, .observe-card, .clients-card, .registered-clients-card, .activity-grids, .think-card').forEach(el => el.style.display = 'none');
    state.deepLinkMode = true;
    elements.logsSummaryBadge.style.display = 'none';

    // Replace header with log file context
    logsTitle.textContent = 'log file';
    logsControls.innerHTML = '<button id="logBackBtn">← back to dashboard</button>';
    function parseDeepLinkLogLine(rawLine) {
      const prefixed = rawLine.match(/^\S+\s+\[([^\]]+)\]\s?(.*)$/) || rawLine.match(/^\[([^\]]+)\]\s?(.*)$/);
      if (!prefixed) return { stream: 'log', classifierLine: rawLine };
      const label = prefixed[1];
      const splitAt = label.lastIndexOf(':');
      if (splitAt === -1) return { stream: 'log', classifierLine: rawLine };
      return { stream: label.slice(splitAt + 1), classifierLine: prefixed[2] || '' };
    }

    function appendDeepLinkLogLine(parent, rawLine, streamOverride, classifierOverride) {
      const parsed = parseDeepLinkLogLine(rawLine);
      const stream = streamOverride || parsed.stream;
      const classifierLine = classifierOverride || (streamOverride ? rawLine : parsed.classifierLine);
      const level = classifyLogLevel(stream, classifierLine);
      const line = document.createElement('div');
      line.className = 'logs-line logs-level-' + level;
      line.textContent = rawLine;
      parent.appendChild(line);
    }

    viewport.textContent = '';
    appendDeepLinkLogLine(viewport, 'loading...', 'stdout');

    fetch('/app/health/api/log?path=' + encodeURIComponent(deepLinkLog))
      .then(r => r.json().then(data => ({ok: r.ok, data})))
      .then(({ok, data}) => {
        viewport.textContent = '';
        if (!ok) {
          const message = data.error || window.CONVEY_COPY.LOG_READ_FAILED;
          appendDeepLinkLogLine(viewport, message, 'stderr', 'ERROR: ' + message);
          return;
        }
        const pathHeader = document.createElement('div');
        pathHeader.className = 'logs-service-header';
        pathHeader.textContent = '── ' + data.path + ' ──';
        viewport.appendChild(pathHeader);
        data.content.split('\n').forEach((rawLine) => {
          appendDeepLinkLogLine(viewport, rawLine);
        });
      })
      .catch(() => {
        viewport.textContent = '';
        appendDeepLinkLogLine(viewport, 'network error loading log file', 'stderr', 'ERROR: network error loading log file');
      });

    document.addEventListener('click', function(e) {
      if (e.target && e.target.id === 'logBackBtn') {
        window.location.href = '/app/health';
      }
    });

	    // Scroll logs card into view
	    logsCard.scrollIntoView({behavior: 'smooth'});
	  }

	  function runRecentErrorsFocus(day, talent) {
	    state.recentErrorsFilter = { day, talent };
	    state.pendingRecentErrorsFocus = true;
	    updateErrorSummary();
	  }
	  function focusRecentErrors() {
	    const hashParams = new URLSearchParams(window.location.hash.replace(/^#/, ''));
	    if (deepLinkLog || hashParams.get('focus') !== 'recent-errors') return;
	    const day = hashParams.get('day') || 'today';
	    const talent = hashParams.get('talent') || '';
	    runRecentErrorsFocus(day, talent);
	  }
		  function seedAgentErrors(seed) {
		    const entries = Array.isArray(seed) ? seed : [];
		    entries.forEach(entry => appendRecentError(entry));
		    if (entries.length === 0) updateErrorSummary();
		    updateStatusSummary();
		  }

			  // Listen to all Callosum events
	  let healthInitialized = false;

	  function initHealthRealtime() {
	    wireBacklogReprocessActions();
	    if (window.appEvents) {
	      window.appEvents.listen('*', handleEvent);
    }

	    armSkeletonTimeout();
	  }

	  function initHealth() {
	    if (healthInitialized) return;
	    healthInitialized = true;
	    applyHealthCopy();
	    loadHealthState();
	    focusRecentErrors();
	    window.addEventListener('hashchange', focusRecentErrors);
	    document.getElementById('glanceErrors')?.addEventListener('click', (e) => {
	      e.preventDefault();
	      runRecentErrorsFocus('today', '');
	    });
	    initHealthRealtime();
	  }

	  document.addEventListener('workspace:mounted', (event) => {
	    if (!event.detail || event.detail.appName === 'health') {
	      initHealth();
	    }
	  });
	  if (document.readyState === 'complete') {
	    initHealth();
	  }

  function sweepStale() {
    const cutoff = Date.now() - 5 * 60 * 1000;
    let changed = false;
    state.agents.forEach((agent, id) => {
      if ((agent.ts || 0) < cutoff) {
        state.agents.delete(id);
        changed = true;
      }
    });
    if (changed) updateCortexGrid();

    changed = false;
    state.imports.forEach((imp, id) => {
      if ((imp.lastSeen || 0) < cutoff) {
        state.imports.delete(id);
        changed = true;
      }
    });
    if (changed) updateImporterGrid();
  }

  function updateConnectionHealth() {
    const el = elements.connectionIndicator;
    if (!state.lastEventTs) {
      el.textContent = '';
      el.className = 'connection-indicator';
      elements.logsConnectionNote.textContent = '';
      elements.logsConnectionNote.classList.add('hidden');
      return;
    }
    const ago = Math.floor((Date.now() - state.lastEventTs) / 1000);
    const agoText = relativeTime(ago * 1000);
    if (ago >= 60) {
      el.textContent = `⚠ Disconnected (${agoText})`;
      el.className = 'connection-indicator disconnected';
      elements.logsConnectionNote.textContent = 'log updates may be delayed';
      elements.logsConnectionNote.classList.remove('hidden');
    } else if (ago >= 30) {
      el.textContent = `Stale (${agoText})`;
      el.className = 'connection-indicator stale';
      elements.logsConnectionNote.textContent = 'log updates may be delayed';
      elements.logsConnectionNote.classList.remove('hidden');
    } else {
      el.textContent = `Updated ${agoText} ago`;
      el.className = 'connection-indicator';
      elements.logsConnectionNote.textContent = '';
      elements.logsConnectionNote.classList.add('hidden');
    }
  }

  // Sweep stale agents and imports every 60s
  let staleSweepTimer = setInterval(sweepStale, 60000);

  loadRegisteredClients();
  let registeredClientsTimer = setInterval(loadRegisteredClients, 60000);

  // Connection health indicator — updated every 5s
  let connectionHealthTimer = setInterval(updateConnectionHealth, 5000);

  // Pause intervals when tab is hidden
  document.addEventListener('visibilitychange', () => {
    if (document.hidden) {
      clearInterval(staleSweepTimer);
      staleSweepTimer = null;
      clearInterval(connectionHealthTimer);
      connectionHealthTimer = null;
      clearInterval(registeredClientsTimer);
      registeredClientsTimer = null;
      if (elapsedTimer) {
        clearInterval(elapsedTimer);
        elapsedTimer = null;
      }
    } else {
      sweepStale();
      updateConnectionHealth();
      if (state.agents.size > 0) {
        updateElapsed();
        startElapsedTimer();
      }
      staleSweepTimer = setInterval(sweepStale, 60000);
      loadRegisteredClients();
      registeredClientsTimer = setInterval(loadRegisteredClients, 60000);
      connectionHealthTimer = setInterval(updateConnectionHealth, 5000);
    }
  });
})();
