// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Status pane toggle logic
window.whenShellReady(() => {
  const statusIcon = document.querySelector('#status-instrument .status-icon');
  const statusPane = document.querySelector('.status-pane');
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

  if (statusIcon && statusPane) {
    statusIcon.addEventListener('click', (e) => {
      e.stopPropagation();
      statusPaneOpen = !statusPaneOpen;
      statusIcon.setAttribute('aria-expanded', statusPaneOpen ? 'true' : 'false');
      window.updateStatusLabel();

      if (statusPaneOpen) {
        statusPane.classList.add('visible');
        statusPane.focus();
        window.AppServices?.quietNotifs?.markViewed();
        renderQuietNotifs();
        updateStatusPane();
        fetchSystemStatus();
      } else {
        statusPane.classList.remove('visible');
        statusIcon.focus();
      }
    });

    // Close status pane when clicking outside
    document.addEventListener('click', (e) => {
      if (statusPaneOpen && statusPane && statusIcon &&
          !statusIcon.contains(e.target) && !statusPane.contains(e.target)) {
        statusPaneOpen = false;
        statusPane.classList.remove('visible');
        statusIcon.setAttribute('aria-expanded', 'false');
        window.updateStatusLabel();
        statusIcon.focus();
      }
    });

    // Escape to close status pane
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && statusPaneOpen) {
        statusPaneOpen = false;
        statusPane.classList.remove('visible');
        statusIcon.setAttribute('aria-expanded', 'false');
        window.updateStatusLabel();
        statusIcon.focus();
      }
    });

    if (statusConsoleLink) {
      statusConsoleLink.addEventListener('click', (e) => {
        e.preventDefault();
        e.stopPropagation();
        window.convey?.diagnosticConsole?.open?.();
        window.convey?.diagnosticConsole?.markAllRead?.();
        updateDiagnosticConsoleLink();
        statusPaneOpen = false;
        statusPane.classList.remove('visible');
        statusIcon.setAttribute('aria-expanded', 'false');
        window.updateStatusLabel();
      });
    }
  }

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
        const uptimeText = `connected for ${formatDurationHuman(Math.floor(metrics.uptimeMs / 1000))}`;
        if (metrics.lastMessageMs !== null) {
          const seconds = Math.floor(metrics.lastMessageMs / 1000);
          const lastUpdate = seconds === 0 ? 'just now' : `${formatDurationHuman(seconds)} ago`;
          statusDetail.textContent = `${uptimeText} · last update ${lastUpdate}`;
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

  function formatClientLastReported(clients) {
    const seen = clients.filter(o => typeof o.last_seen === 'number');
    if (!seen.length) return '';
    const lastSeen = Math.max(...seen.map(o => o.last_seen));
    return relativeTime(Date.now() - lastSeen) + ' ago';
  }

  function captureMonthDay(ms) {
    return new Date(ms).toLocaleDateString('en-US', { month: 'short', day: 'numeric' }).toLowerCase();
  }

	  function restartCaptureFromStatusPane(button, resultEl) {
	    button.disabled = true;
	    button.textContent = 'reconnecting…';
	    resultEl.textContent = '';
	    // Client rows are per registration key; supervisor restarts the shared sense worker.
	    window.apiJson('/app/health/api/restart-capture', {
	      method: 'POST',
	      headers: { 'Content-Type': 'application/json' },
	      body: JSON.stringify({ service: 'sense' })
	    })
	      .then(() => {
	        button.textContent = window.CONVEY_COPY?.ACTION_RECONNECT || 'Reconnect';
	        resultEl.style.color = '#6b7280';
	        resultEl.textContent = 'reconnect requested';
	      })
	      .catch(err => {
	        button.disabled = false;
	        button.textContent = window.CONVEY_COPY?.ACTION_RECONNECT || 'Reconnect';
	        resultEl.style.color = '#ef4444';
	        resultEl.textContent = err?.serverMessage || "couldn't restart processing.";
	      });
	  }

	  function renderCaptureSection(capture) {
	    const section = document.getElementById('capture-status-section');
	    const text = document.getElementById('capture-status-text');
    if (!section || !text) return;

    section.style.display = '';
	    const status = capture?.status;
		    if (status === 'active') {
		      text.style.color = '#10b981';
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
		        link.style.cssText = 'display: inline-block; margin-top: 4px; color: #b91c1c; font-size: 12px;';
		        text.appendChild(link);
		      };

		      if (!degraded.length) {
		        appendLine('a device needs attention', 'color: #b91c1c; font-weight: 600;');
		        appendLine("a device isn't reaching your journal.", 'color: #b91c1c; font-size: 12px;');
		        appendHealthLink();
		        return;
		      }

		      const first = degraded[0];
		      const rej = first.ingest_rejection;
		      const name = (first.name || '').trim();
		      const title = name ? name + ' needs attention' : 'a device needs attention';
		      const hasFirstTs = typeof rej.first_ts === 'number' && isFinite(rej.first_ts);
		      const hasActiveCount = typeof rej.active_count === 'number' && isFinite(rej.active_count);
		      appendLine(title, 'color: #b91c1c; font-weight: 600;');

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
		      appendLine(consequence, 'color: #b91c1c; font-size: 12px;');

		      const recovery = rej.version
		        ? (name || 'this device') + ' is running the solstone app v' + rej.version + '. update or restart the solstone app on that device, then the next time it adds to your journal, this clears.'
		        : 'update or restart it on that device, then a valid upload clears this.';
		      appendLine(recovery, 'color: #6b7280; font-size: 12px;');

		      const parts = [];
		      if (rej.reason_code) parts.push('reason: ' + rej.reason_code);
		      if (rej.stream) parts.push('stream: ' + rej.stream);
		      if (rej.summary) parts.push(rej.summary);
		      if (typeof rej.latest_ts === 'number' && isFinite(rej.latest_ts)) {
		        parts.push('last rejected ' + relativeTime(Date.now() - rej.latest_ts) + ' ago');
		      }
		      if (parts.length) {
		        appendLine(parts.join(' · '), 'color: #6b7280; font-size: 11px;');
		      }

		      appendHealthLink();
		      if (degraded.length > 1) {
		        appendLine('and ' + (degraded.length - 1) + ' more need attention', 'color: #6b7280; font-size: 12px; margin-top: 2px;');
		      }
		      return;
		    } else if (status === 'stale') {
		      const stale = (capture.clients || []).filter(o => o.status === 'stale');
	      const names = stale.map(o => o.name).filter(Boolean).join(', ');
	      const label = stale.length === 1 ? 'device' : 'devices';
	      const lastReported = formatClientLastReported(stale);
	      text.textContent = `${label} ${names || 'device'} last reported ${lastReported || 'recently'}`;
	      text.style.color = '#f59e0b';
	      const button = document.createElement('button');
	      button.type = 'button';
	      button.textContent = window.CONVEY_COPY?.ACTION_RECONNECT || 'Reconnect';
	      button.style.cssText = 'margin-left: 0.5rem; font-size: 12px; padding: 0.15rem 0.45rem; border: 1px solid #d1d5db; border-radius: 4px; background: #fff; cursor: pointer;';
	      const result = document.createElement('span');
	      result.style.cssText = 'margin-left: 0.4rem; font-size: 12px;';
	      button.addEventListener('click', () => restartCaptureFromStatusPane(button, result));
	      text.appendChild(button);
	      text.appendChild(result);
	      return;
	    } else if (status === 'offline') {
      text.style.color = '#ef4444';
    } else {
      text.style.color = '#9ca3af';
    }
	    if (status === 'no_clients') {
	      text.textContent = 'no devices are running the solstone app yet. set one up to start your journal.';
	    } else if (status === 'active' || status === 'offline') {
	      text.textContent = 'device ' + status;
	    } else {
	      text.textContent = "i don't know the status of your devices right now.";
	    }
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
      span.style.color = '#f59e0b';
      span.textContent = 'update available (v' + (version.latest || '?') + ')';
      text.appendChild(span);
      text.style.color = '';
    } else {
      text.textContent = 'v' + (version?.current || 'unknown');
      text.style.color = '#9ca3af';
    }
  }

	  function renderQuietNotifs() {
    const section = document.getElementById('quiet-notifs-section');
    const list = document.getElementById('quiet-notifs-list');
    if (!section || !list) return;

    const notifs = window.AppServices?.quietNotifs?.getAll() || [];
    section.style.display = notifs.length > 0 ? '' : 'none';

    if (notifs.length === 0) {
      list.textContent = '';
      const empty = document.createElement('span');
      empty.style.color = '#9ca3af';
      empty.textContent = 'no quiet notifications';
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
        panel.style.cssText = 'padding: 4px 0 2px; color: #fca5a5; font-size: 13px; white-space: pre-wrap; word-break: break-word;';
        row.appendChild(panel);
      } else {
        btn = row.querySelector('[data-action="toggle-quiet-notif"]');
        panel = row.querySelector('[data-quiet-notif-panel]');
      }

      const relativeAge = window.AppServices.notifications._getRelativeTime(n.ts);
      btn.textContent = '';
      const ageSpan = document.createElement('span');
      ageSpan.style.cssText = 'color: #9ca3af; font-size: 11px; flex-shrink: 0;';
      ageSpan.textContent = relativeAge;
      btn.appendChild(ageSpan);

      const srcCode = document.createElement('code');
      srcCode.style.cssText = 'flex-shrink: 0; font-size: 11px;';
      srcCode.textContent = n.source || '';
      btn.appendChild(srcCode);

      const snippet = document.createElement('span');
      snippet.style.cssText = 'color: #fca5a5; font-size: 13px; flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;';
      snippet.textContent = n.message || '';
      btn.appendChild(snippet);

      const hint = document.createElement('span');
      hint.setAttribute('data-quiet-notif-hint', 'true');
      hint.style.cssText = 'flex-shrink: 0; color: #9ca3af; font-size: 11px;';
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
      container.innerHTML = '<span style="color: #9ca3af;">no recent activity</span>';
      return;
    }

    container.innerHTML = history.map(n => {
      const relativeAge = window.AppServices.notifications._getRelativeTime(n.timestamp);

      if (n.action) {
        return `<a href="${escape(n.action)}" class="status-pane-history-item" style="display: flex; align-items: center; gap: 8px; padding: 6px 8px; margin: 0 -8px; border-radius: 4px; text-decoration: none; color: inherit;">
          <span class="icon-slot" style="font-size: 16px; flex-shrink: 0;" aria-hidden="true">${resolveIcon(n.icon)}</span>
          <span style="font-weight: 500; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${escape(n.title)}</span>
          <span style="color: #9ca3af; font-size: 11px; flex-shrink: 0;">${relativeAge}</span>
        </a>`;
      } else {
        return `<div style="display: flex; align-items: center; gap: 8px; padding: 6px 8px; margin: 0 -8px;">
          <span class="icon-slot" style="font-size: 16px; flex-shrink: 0;" aria-hidden="true">${resolveIcon(n.icon)}</span>
          <span style="font-weight: 500; flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;">${escape(n.title)}</span>
          <span style="color: #9ca3af; font-size: 11px; flex-shrink: 0;">${relativeAge}</span>
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

  function formatDurationHuman(seconds) {
    if (seconds < 10) return 'a few seconds';
    if (seconds < 60) return 'less than a minute';
    const minutes = Math.floor(seconds / 60);
    if (minutes === 1) return 'about a minute';
    if (minutes < 60) return minutes + ' minutes';
    const hours = Math.floor(minutes / 60);
    if (hours === 1) return 'about an hour';
    if (hours < 24) return hours + ' hours';
    const days = Math.floor(hours / 24);
    if (days === 1) return 'about a day';
    return days + ' days';
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
