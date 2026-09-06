// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Navigation status and inline Health details
window.whenShellReady(() => {
  const statusIcon = document.querySelector('#status-instrument .status-icon');
  const statusPane = document.getElementById('status-pane');
  const statusConsoleLink = document.getElementById('status-pane-console-link');
  const liveRegion = document.getElementById('status-live-region');
  let statusPaneOpen = false;
  let lastCaptureStatusForPane = null;
  let _lastHistoryLen = -1;

  // Shared label updater — called from here and from websocket.js
  window.updateStatusLabel = function() {
    if (!statusIcon) return;
    const label = window.appEvents?.statusLabel || 'connecting';
    statusIcon.setAttribute('aria-label', label);
    statusIcon.setAttribute('title', label);
    const unread = window.AppServices?.quietNotifs?.unviewedCount?.() || 0;
    statusIcon.href = ['degraded', 'offline'].includes(lastCaptureStatusForPane)
      ? '/app/health/#registeredClientsCard'
      : unread ? '/app/health/#quiet-notifs-section' : '/app/health/#healthSystemDetails';
    const visibleLabel = document.querySelector('#status-instrument .status-label');
    if (visibleLabel) visibleLabel.textContent = label;
    if (liveRegion) liveRegion.textContent = label;
  };

  function updateDiagnosticConsoleLink() {
    if (!statusConsoleLink) return;
    const base = window.CONVEY_COPY?.CONSOLE_LINK_LABEL || 'system messages';
    const unread = window.convey?.diagnosticConsole?.unreadCount?.() || 0;
    statusConsoleLink.textContent = unread > 0 ? `${base} (${unread})` : base;
    statusConsoleLink.setAttribute('aria-label', unread > 0 ? `${base}, ${unread} unread` : base);
  }

  let initialDestinationPending = true;
  let initialHealthReady = false;
  function revealInitialDestination() {
    if (!statusPaneOpen || !initialDestinationPending || !initialHealthReady) return;
    const id = window.location.hash.slice(1);
    if (!['healthSystemDetails', 'quiet-notifs-section', 'registeredClientsCard'].includes(id)) return;
    const target = document.getElementById(id);
    if (!target || target.getClientRects().length === 0) return;
    initialDestinationPending = false;
    requestAnimationFrame(() => target.scrollIntoView({block: 'start'}));
  }
  // Initial API renders can change the page height after the workspace mounts.
  document.addEventListener('health:initial-ready', () => {
    initialHealthReady = true;
    revealInitialDestination();
  });

  document.addEventListener('workspace:mounted', () => {
    const host = document.getElementById('healthSystemDetails');
    statusPaneOpen = Boolean(host);
    if (host && statusPane) {
      host.appendChild(statusPane);
      statusPane.hidden = false;
      renderQuietNotifs();
      updateStatusPane();
      fetchSystemStatus();
      revealInitialDestination();
    }
  });
  statusConsoleLink?.addEventListener('click', (event) => {
    event.preventDefault();
    window.convey?.diagnosticConsole?.open?.();
    window.convey?.diagnosticConsole?.markAllRead?.();
    updateDiagnosticConsoleLink();
  });

  // Update status pane metrics
  function updateStatusPane() {
    window.renderStatusMark?.();
    updateDiagnosticConsoleLink();
    if (!window.appEvents) return;
    if (!statusPaneOpen) return;

    const metrics = window.appEvents.getMetrics();
    const statusSentence = document.getElementById('status-sentence');
    const statusDetail = document.getElementById('status-detail');
    const wsStatusRaw = document.getElementById('ws-status-raw');
    const wsUptimeRaw = document.getElementById('ws-uptime-raw');
    const wsLastMessageRaw = document.getElementById('ws-last-message-raw');

    if (statusSentence) {
      // Headline answers the same question the mark answers (is your journal
      // receiving your life right now?). Transport health stays in technical
      // details below — naming that axis, rather than claiming "all systems
      // connected" under an offline mark.
      const markLabel = window.appEvents?.statusLabel;
      if (markLabel) {
        statusSentence.textContent = markLabel;
        statusSentence.style.color = '';
      } else if (metrics.state === 'connecting') {
        statusSentence.textContent = 'connecting…';
        statusSentence.style.color = '';
      } else {
        statusSentence.textContent = 'connection lost. reconnecting';
        statusSentence.style.color = '';
      }
    }

    if (statusDetail) {
      if (metrics.state === 'connected') {
        // G3-104: the pane speaks the shared relativeTime ladder, so the shell
        // and Health never describe the same instant differently. Under a
        // minute reads as "just now" rather than a vague "a few seconds".
        const MINUTE = 60000;
        const uptimeText = metrics.uptimeMs < MINUTE
          ? 'connected just now'
          : `connected for ${relativeTime(metrics.uptimeMs)}`;
        if (metrics.lastMessageMs !== null) {
          const lastUpdate = metrics.lastMessageMs < MINUTE
            ? 'updated just now'
            : `updated ${relativeTime(metrics.lastMessageMs)} ago`;
          statusDetail.textContent = `${uptimeText} · ${lastUpdate}`;
        } else {
          statusDetail.textContent = uptimeText;
        }
      } else if (metrics.state === 'connecting') {
        statusDetail.textContent = '';
      } else {
        statusDetail.textContent = 'some features may be delayed';
      }
    }

    if (wsStatusRaw) {
      wsStatusRaw.textContent = metrics.state;
    }

    if (wsUptimeRaw) {
      if (metrics.connected) {
        wsUptimeRaw.textContent = formatDuration(Math.floor(metrics.uptimeMs / 1000));
      } else {
        wsUptimeRaw.textContent = '-';
      }
    }

    if (wsLastMessageRaw) {
      if (metrics.lastMessageMs !== null) {
        const seconds = Math.floor(metrics.lastMessageMs / 1000);
        wsLastMessageRaw.textContent = `${relativeTime(seconds * 1000)} ago`;
      } else if (metrics.connected) {
        wsLastMessageRaw.textContent = 'no messages yet';
      } else {
        wsLastMessageRaw.textContent = '-';
      }
    }

    renderQuietNotifs();
    const quietSection = document.getElementById('quiet-notifs-section');
    const quietBounds = quietSection?.getBoundingClientRect();
    if (document.visibilityState !== 'hidden' && quietBounds?.height > 0
        && quietBounds.bottom > 0 && quietBounds.top < window.innerHeight
        && window.AppServices?.quietNotifs?.unviewedCount() > 0) {
      window.AppServices.quietNotifs.markViewed();
    }
    // Update notification history
    updateNotificationHistory();
    updateBellState();
  }

  function fetchSystemStatus() {
    fetch('/api/system/status')
      .then(r => r.ok ? r.json() : null)
      .catch(() => null)
      .then(data => {
        if (data) {
          lastCaptureStatusForPane = data.capture?.status ?? 'unknown';
          window.appEvents?.setCaptureStatus?.(data.capture?.status ?? 'unknown');
          if (statusPaneOpen) {
            renderCaptureSection(data.capture);
            renderVersionSection(data.version);
          }
        } else {
          lastCaptureStatusForPane = 'unknown';
          window.appEvents?.setCaptureStatus?.('unknown');
        }
      });
  }

  function captureMonthDay(ms) {
    return new Date(ms).toLocaleDateString('en-US', { month: 'short', day: 'numeric' }).toLowerCase();
  }

	  function renderCaptureSection(capture) {
	    const section = document.getElementById('capture-status-section');
	    const text = document.getElementById('capture-status-text');
    if (!section || !text) return;

    section.style.display = '';
	    const status = capture?.status;
		    if (status === 'active') {
		      text.style.color = 'var(--success-ink)';
		    } else if (status === 'degraded') {
		      text.textContent = '';
		      text.style.color = '';
		      const degraded = (capture.clients || []).filter(o => o.status === 'degraded' && o.ingest_rejection);
		      const appendLine = (value, cssText) => {
		        const line = document.createElement('div');
		        line.textContent = value;
		        if (cssText) line.style.cssText = cssText;
		        text.appendChild(line);
		        return line;
		      };
		      const appendHealthLink = () => {
		        const link = document.createElement('a');
		        link.href = '/app/health';
		        link.textContent = 'view device health →';
		        link.style.cssText = 'display: inline-block; margin-top: 4px; color: var(--danger); font-size: 12px;';
		        text.appendChild(link);
		      };

		      if (!degraded.length) {
		        appendLine('a device needs attention', 'color: var(--danger); font-weight: 600;');
		        appendLine("a device isn't reaching your journal.", 'color: var(--danger); font-size: 12px;');
		        appendHealthLink();
		        return;
		      }

		      const first = degraded[0];
		      const rej = first.ingest_rejection;
		      const name = (first.name || '').trim();
		      const title = name ? name + ' needs attention' : 'a device needs attention';
		      const hasFirstTs = typeof rej.first_ts === 'number' && isFinite(rej.first_ts);
		      const hasActiveCount = typeof rej.active_count === 'number' && isFinite(rej.active_count);
		      appendLine(title, 'color: var(--danger); font-weight: 600;');

		      let consequence;
		      if (hasFirstTs && hasActiveCount) {
			        consequence = "what it sensed hasn't reached your journal since " + captureMonthDay(rej.first_ts) + ', ' + rej.active_count + ' uploads turned away.';
		      } else if (hasActiveCount) {
			        consequence = "what it senses isn't reaching your journal, " + rej.active_count + ' uploads turned away.';
		      } else if (hasFirstTs) {
			        consequence = "what it sensed hasn't reached your journal since " + captureMonthDay(rej.first_ts) + '.';
		      } else {
			        consequence = "what it senses isn't reaching your journal.";
		      }
		      appendLine(consequence, 'color: var(--danger); font-size: 12px;');

		      const recovery = rej.version
		        ? (name || 'this device') + ' is running the solstone app v' + rej.version + '. update or restart the solstone app on that device, then the next time it adds to your journal, this clears.'
		        : 'update or restart it on that device, then a valid upload clears this.';
		      appendLine(recovery, 'color: var(--ink-soft); font-size: 12px;');

		      const parts = [];
		      if (rej.reason_code) parts.push('reason: ' + rej.reason_code);
		      if (rej.stream) parts.push('stream: ' + rej.stream);
		      if (rej.summary) parts.push(rej.summary);
		      if (typeof rej.latest_ts === 'number' && isFinite(rej.latest_ts)) {
		        parts.push('last rejected ' + relativeTime(Date.now() - rej.latest_ts) + ' ago');
		      }
		      if (parts.length) {
		        appendLine(parts.join(' · '), 'color: var(--ink-soft); font-size: 11px;');
		      }

		      appendHealthLink();
		      if (degraded.length > 1) {
		        appendLine('and ' + (degraded.length - 1) + ' more need attention', 'color: var(--ink-soft); font-size: 12px; margin-top: 2px;');
		      }
		      return;
	    } else if (status === 'offline') {
      text.style.color = 'var(--danger)';
    } else if (status === 'stale') {
      text.style.color = 'var(--warn-ink)';
    } else {
      text.style.color = 'var(--ink-faint)';
    }
    if (status === 'no_clients') {
      text.textContent = 'no devices are running the solstone app yet. set one up to start your journal.';
    } else if (status === 'offline' || status === 'stale') {
      text.textContent = silentDeviceSentence(capture);
    } else if (status === 'active') {
      text.textContent = 'devices are adding to your journal';
    } else {
      text.textContent = 'device status is unavailable right now.';
    }
  }

  // Name the device and how long it has been quiet. "a device" gave the owner
  // nothing to act on, and the health glance now carries the same rollup.
  function silentDeviceSentence(capture) {
    const named = (capture && capture.clients ? capture.clients : [])
      .filter(client => client.status === 'offline' || client.status === 'stale')
      .map(client => ({
        name: String(client.name || '').trim(),
        ageMs: captureAgeMs(client.last_accepted_ingest_at)
      }))
      .filter(client => client.name)
      .sort((a, b) => (b.ageMs === null ? -1 : b.ageMs) - (a.ageMs === null ? -1 : a.ageMs));
    if (named.length === 0) {
      return "a device hasn't added to your journal recently.";
    }
    if (named.length === 1) {
      const only = named[0];
      return only.ageMs === null
        ? only.name + " hasn't added to your journal recently."
        : only.name + " hasn't added to your journal in " + relativeTime(only.ageMs) + '.';
    }
    const parts = named.map(
      client => client.name + (client.ageMs === null ? '' : ' (' + relativeTime(client.ageMs) + ')')
    );
    return named.length + " devices haven't added to your journal recently: " + parts.join(', ') + '.';
  }

  function captureAgeMs(timestamp) {
    const parsed = timestamp ? Date.parse(timestamp) : NaN;
    if (!Number.isFinite(parsed)) return null;
    const age = Date.now() - parsed;
    return age < 0 ? null : age;
  }

  function renderVersionSection(version) {
    const section = document.getElementById('version-section');
    const text = document.getElementById('version-text');
    if (!section || !text) return;

    section.style.display = '';
    if (version?.update_available) {
      text.textContent = '';
      text.appendChild(document.createTextNode('v' + (version.current || '?') + ' · '));
      const span = document.createElement('span');
      span.style.color = 'var(--warn-ink)';
      span.textContent = 'update available (v' + (version.latest || '?') + ')';
      text.appendChild(span);
      text.style.color = '';
    } else {
      text.textContent = 'v' + (version?.current || 'unknown');
      text.style.color = 'var(--ink-faint)';
    }
  }

	  function renderQuietNotifs() {
    const section = document.getElementById('quiet-notifs-section');
    const list = document.getElementById('quiet-notifs-list');
    if (!section || !list) return;

    const notifs = window.AppServices?.quietNotifs?.getAll() || [];
    // The settings notifications row links straight here, so the section stays
    // on the page with an honest empty state rather than collapsing to 0x0 and
    // landing the owner at the top of health (G3-107).
    section.style.display = '';

    if (notifs.length === 0) {
      list.textContent = '';
      const empty = document.createElement('span');
      empty.style.color = 'var(--ink-faint-paper)';
      empty.textContent = 'no notifications held back';
      list.appendChild(empty);
      return;
    }

    const newIds = new Set(notifs.map(n => String(n.id)));
    const existingById = new Map();
    for (const child of Array.from(list.children)) {
      const k = child.getAttribute('data-id');
      if (k) existingById.set(k, child);
    }
    // drop stale rows AND any non-keyed node (e.g. a prior "no quiet notifications" span)
    for (const child of Array.from(list.children)) {
      const k = child.getAttribute('data-id');
      if (!k || !newIds.has(k)) list.removeChild(child);
    }

    for (const n of notifs) {
      const key = String(n.id);
      let row = existingById.get(key);
      let btn, panel;
      if (!row) {
        row = document.createElement('div');
        row.setAttribute('data-id', key);
        row.style.cssText = 'padding: 4px 0;';

        const panelId = 'quiet-notif-panel-' + key;
        btn = document.createElement('button');
        btn.type = 'button';
        btn.setAttribute('data-action', 'toggle-quiet-notif');
        btn.setAttribute('aria-expanded', 'false');
        btn.setAttribute('aria-controls', panelId);
        btn.style.cssText = 'display: flex; align-items: center; gap: 8px; width: 100%; text-align: left; background: none; border: none; padding: 0; margin: 0; font: inherit; color: inherit; cursor: pointer;';
        row.appendChild(btn);

        panel = document.createElement('div');
        panel.id = panelId;
        panel.hidden = true;
        panel.setAttribute('data-quiet-notif-panel', 'true');
        panel.style.cssText = 'padding: 4px 0 2px; color: var(--danger); font-size: 13px; white-space: pre-wrap; word-break: break-word;';
        row.appendChild(panel);
      } else {
        btn = row.querySelector('[data-action="toggle-quiet-notif"]');
        panel = row.querySelector('[data-quiet-notif-panel]');
      }

      const relativeAge = window.AppServices.notifications._getRelativeTime(n.ts);
      btn.textContent = '';
      const ageSpan = document.createElement('span');
      ageSpan.style.cssText = 'color: var(--ink-faint); font-size: 11px; flex-shrink: 0;';
      ageSpan.textContent = relativeAge;
      btn.appendChild(ageSpan);

      const srcCode = document.createElement('code');
      srcCode.style.cssText = 'flex-shrink: 0; font-size: 11px;';
      srcCode.textContent = n.source || '';
      btn.appendChild(srcCode);

      const snippet = document.createElement('span');
      snippet.style.cssText = 'color: var(--danger); font-size: 13px; flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;';
      snippet.textContent = n.message || '';
      btn.appendChild(snippet);

      const hint = document.createElement('span');
      hint.setAttribute('data-quiet-notif-hint', 'true');
      hint.style.cssText = 'flex-shrink: 0; color: var(--ink-faint); font-size: 11px;';
      hint.textContent = btn.getAttribute('aria-expanded') === 'true' ? 'hide details' : 'show details';
      btn.appendChild(hint);

      panel.textContent = n.message || '';

      list.appendChild(row);
    }
  }

  const quietNotifsList = document.getElementById('quiet-notifs-list');
  if (quietNotifsList) {
    quietNotifsList.addEventListener('click', (e) => {
      const btn = e.target.closest('[data-action="toggle-quiet-notif"]');
      if (!btn) return;
      const expanded = btn.getAttribute('aria-expanded') === 'true';
      btn.setAttribute('aria-expanded', expanded ? 'false' : 'true');
      const panel = document.getElementById(btn.getAttribute('aria-controls'));
      if (panel) panel.hidden = expanded;
      const hint = btn.querySelector('[data-quiet-notif-hint]');
      if (hint) hint.textContent = expanded ? 'show details' : 'hide details';
    });
  }

  function updateNotificationHistory() {
    const container = document.getElementById('notification-history');
    if (!container || !window.AppServices?.notifications) return;

    const history = window.AppServices.notifications.getHistory();
    if (!statusPaneOpen && history.length === _lastHistoryLen) return;
    _lastHistoryLen = history.length;
    const escape = window.AppServices.escapeHtml;
    const resolveIcon = value => window.AppServices.notifications._resolveIcon(value);

    if (history.length === 0) {
      container.innerHTML = '<span style="color: var(--ink-faint-paper);">no notifications yet</span>';
      return;
    }

    container.innerHTML = history.map(n => {
      const relativeAge = window.AppServices.notifications._getRelativeTime(n.timestamp);

      const action = window.AppServices.sameOriginPath(n.action);
      if (action) {
        return `<a href="${escape(action)}" class="status-pane-history-item" style="display: flex; align-items: center; gap: 8px; padding: 6px 8px; margin: 0 -8px; border-radius: 4px; text-decoration: none; color: inherit;">
          <span class="icon-slot" style="font-size: 16px; flex-shrink: 0;" aria-hidden="true">${resolveIcon(n.icon)}</span>
          <span style="font-weight: 500; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${escape(n.title)}</span>
          <span style="color: var(--ink-faint); font-size: 11px; flex-shrink: 0;">${relativeAge}</span>
        </a>`;
      } else {
        return `<div style="display: flex; align-items: center; gap: 8px; padding: 6px 8px; margin: 0 -8px;">
          <span class="icon-slot" style="font-size: 16px; flex-shrink: 0;" aria-hidden="true">${resolveIcon(n.icon)}</span>
          <span style="font-weight: 500; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${escape(n.title)}</span>
          <span style="color: var(--ink-faint); font-size: 11px; flex-shrink: 0;">${relativeAge}</span>
        </div>`;
      }
    }).join('');
  }

  function formatDuration(seconds) {
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    const secs = seconds % 60;
    if (minutes < 60) return `${minutes}m ${secs}s`;
    const hours = Math.floor(minutes / 60);
    const mins = minutes % 60;
    return `${hours}h ${mins}m`;
  }

  function updateBellState() {
    const bell = document.getElementById('notif-bell');
    if (!bell || !('Notification' in window)) {
      if (bell) bell.style.display = 'none';
      return;
    }
    const perm = Notification.permission;
    bell.setAttribute('data-perm', perm);
    if (perm === 'granted') {
      bell.innerHTML = `<span aria-hidden="true">${(window.ConveyIcons?.svg('bell') || '')}</span>`;
      bell.title = 'browser notifications enabled';
      bell.setAttribute('aria-label', 'browser notifications enabled');
    } else if (perm === 'denied') {
      bell.innerHTML = `<span aria-hidden="true">${(window.ConveyIcons?.svg('bell-off') || '')}</span>`;
      bell.title = 'notifications blocked. update in browser settings';
      bell.setAttribute('aria-label', 'notifications blocked. update in browser settings');
    } else {
      bell.innerHTML = `<span aria-hidden="true">${(window.ConveyIcons?.svg('bell') || '')}</span>`;
      bell.title = 'enable browser notifications';
      bell.setAttribute('aria-label', 'enable browser notifications');
    }
  }

  const bellEl = document.getElementById('notif-bell');
  if (bellEl) {
    bellEl.addEventListener('click', async (e) => {
      e.stopPropagation();
      if (!('Notification' in window)) return;
      if (Notification.permission === 'default') {
        await AppServices.requestNotificationPermission();
        updateBellState();
      }
    });
  }

  fetchSystemStatus();
  setInterval(fetchSystemStatus, 60000);
  // Update status pane every second
  setInterval(updateStatusPane, 1000);
  window.addEventListener('diagnostic-console-updated', updateDiagnosticConsoleLink);
  // Initial update
  setTimeout(updateStatusPane, 100);
});
