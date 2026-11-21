// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Read-only presentation shared by Backup and Health. Recorded results are
// history, not a coverage, freshness or destination-integrity assessment.
(function () {
  const BACKUP_ERROR_REASONS = {
    "incomplete": window.JournalI18n.render('journal.window.backup.status.window.explanation.some.of.your.journal.couldn.t'),
    "repo_missing": window.JournalI18n.render('journal.window.backup.status.window.explanation.your.backup.destination.couldn.t.be'),
    "locked": window.JournalI18n.render('journal.window.backup.status.window.explanation.the.backup.lock.was.already.held'),
    "auth_failed": window.JournalI18n.render('journal.window.backup.status.window.explanation.the.backup.key.couldn.t.open'),
    "timeout": window.JournalI18n.render('journal.window.backup.status.window.explanation.it.ran.out.of.time'),
    "_missing": window.JournalI18n.render('journal.window.backup.status.window.explanation.the.reason.wasn.t.recorded'),
    "restic_unavailable": window.JournalI18n.render('journal.window.backup.status.window.explanation.encrypted.backup.can.t.run.on'),
    "rclone_unavailable": window.JournalI18n.render('journal.window.backup.status.window.explanation.encrypted.backup.can.t.reach.storage'),
    "backup_unavailable": window.JournalI18n.render('journal.window.backup.status.window.explanation.this.computer.can.t.set.up'),
    "failed": window.JournalI18n.render('journal.window.backup.status.window.explanation.the.backup.tool.stopped.with.an'),
    "unknown": window.JournalI18n.render('journal.window.backup.status.window.explanation.the.backup.tool.stopped.for.a'),
    "broker_unreachable": window.JournalI18n.render('journal.window.backup.status.window.explanation.encrypted.backup.couldn.t.be.reached'),
    "broker_error": window.JournalI18n.render('journal.window.backup.status.window.explanation.encrypted.backup.didn.t.return.usable'),
    "hosted_entitlement_inactive": window.JournalI18n.render('journal.window.backup.status.window.explanation.encrypted.backup.isn.t.turned.on'),
    "binding_invalid": window.JournalI18n.render('journal.window.backup.status.window.explanation.this.computer.s.link.to.encrypted'),
    "binding_superseded": window.JournalI18n.render('journal.window.backup.status.window.explanation.another.copy.of.your.journal.took'),
    "repository_unsupported": window.JournalI18n.render('journal.window.backup.status.window.explanation.the.repository.in.your.backup.settings')
};
  const unavailable = window.JournalI18n.render('journal.window.backup.status.window.explanation.backup.status.is.unavailable.right.now');
  const incomplete = BACKUP_ERROR_REASONS.incomplete;
  const qualification = window.JournalI18n.render('journal.window.backup.status.window.explanation.these.are.recorded.results.they.don');
  const operationKinds = {
    enable: 'setup', enable_hosted: 'setup', rotate: 'rotate', teardown: 'stop',
    restore: 'restore', restore_hosted: 'restore', restore_recovery: 'restore',
    offload: 'prepare', offload_restore: 'restore', offload_approve: 'approve',
  };
  const operationPhases = {
    setting_up: 'active', restoring: 'active', rotating: 'active', tearing_down: 'active',
    cancelling: 'cancelling', cleanup_pending: 'cleanup', done: 'completed', error: 'failed',
    degraded: 'degraded', refused: 'refused', needs_subscription: 'subscription',
  };

  function operationWasCancelled(operation) {
    return operation.phase === 'error' && operation.reason_code === 'cancelled';
  }

  function operationText(operation, form) {
    const phase = operationWasCancelled(operation) ? 'cancelled' : operationPhases[operation.phase];
    return window.JournalI18n.render(`journal.window.backup.status.activity.${phase}.${form}`, [
      ['operation_kind', operationKinds[operation.kind]],
    ]);
  }

  const record = value => value !== null && typeof value === 'object' && !Array.isArray(value);
  const optionalRecord = value => value === null || value === undefined || record(value);
  const date = time => typeof time === 'number' && Number.isFinite(time) && time > 0
    && Number.isFinite(new Date(time * 1000).getTime())
    ? new Date(time * 1000).toLocaleString(window.JournalI18n.formatLocale) : null;

  // 'checked_subset' is restic's --read-data-subset selector, shaped 'n/m':
  // the run read one of m equal parts of the pack data, and n only says which
  // part this week's turn landed on. It is never a count of snapshots or of
  // parts checked, so the sentence reports one part out of m and nothing else.
  function verificationSubsetLine(value) {
    if (typeof value !== 'string' || !/^\d+\/\d+$/.test(value)) return '';
    const [selected, total] = value.split('/').map(Number);
    if (!Number.isSafeInteger(selected) || !Number.isSafeInteger(total)
        || selected <= 0 || total <= 0 || selected > total) return '';
    if (total === 1) return window.JournalI18n.render('journal.window.backup.verification.parts.checked.all');
    return window.JournalI18n.render('journal.window.backup.verification.parts.checked', [
      ['parts_quantity', window.JournalI18n.cardinalQuantity(total)], ['parts', String(total)],
    ]);
  }

  const attemptOutcomes = { ok: 'completed', error: 'failed', never: 'never-recorded', cleared_superseded: 'could-not-run' };
  const checkOutcomes = { ok: 'completed', error: 'failed', skipped: 'could-not-run' };

  function resultText(result, verification = false) {
    const outcomes = verification ? checkOutcomes : attemptOutcomes;
    if (!record(result) || !Object.hasOwn(outcomes, result.status)) {
      return window.JournalI18n.render('journal.window.backup.status.outcome.unknown');
    }
    let detail = '';
    if (!verification && result.status === 'error') {
      detail = Object.hasOwn(BACKUP_ERROR_REASONS, result.error_reason)
        ? BACKUP_ERROR_REASONS[result.error_reason] : BACKUP_ERROR_REASONS._missing;
    }
    if (!verification && result.status === 'ok' && result.error_reason === 'incomplete') {
      detail = incomplete;
    }
    if (verification && result.status === 'ok') {
      detail = verificationSubsetLine(result.checked_subset);
    }
    const at = date(result.time);
    const args = [['outcome', outcomes[result.status]]];
    if (at) args.push(['recorded_at', at]);
    const message = window.JournalI18n.render(
      `journal.window.backup.status.check.result.date.${at ? 'known' : 'unknown'}.${detail ? 'sentence' : 'label'}`,
      args,
    );
    return detail ? `${message} ${detail}` : message;
  }

  function select(snapshot) {
    if (!record(snapshot) || typeof snapshot.enabled !== 'boolean'
        || !['last_backup', 'last_verification', 'operation'].every(key => Object.hasOwn(snapshot, key))
        || !optionalRecord(snapshot.last_backup) || !optionalRecord(snapshot.last_verification)
        || !optionalRecord(snapshot.operation)) {
      return { text: unavailable, tone: 'unavailable', snapshot: null };
    }
    const operation = snapshot.operation;
    if (operation && (!Object.hasOwn(operationKinds, operation.kind)
        || !Object.hasOwn(operationPhases, operation.phase))) {
      return { text: unavailable, tone: 'unavailable', snapshot: null };
    }
    const attempt = snapshot.last_backup;
    const verification = snapshot.last_verification;
    let text, tone = 'neutral';
    // A pause is the owner's choice: neutral, and never read as failing or stale.
    const paused = snapshot.enabled && Number.isInteger(snapshot.paused_at);
    if (!snapshot.enabled && attempt?.status === 'cleared_superseded') {
      text = window.JournalI18n.render('journal.window.backup.status.backup.off.superseded');
      tone = 'attention';
    } else if (!snapshot.enabled) text = window.JournalI18n.render('journal.window.backup.status.backup.off');
    else if (paused) {
      text = date(snapshot.paused_at) ? window.JournalI18n.render('journal.window.backup.status.paused.on.date', [['paused_at', date(snapshot.paused_at)]])
        : window.JournalI18n.render('journal.window.backup.status.window.explanation.you.paused.backup.the.date.isn');
    }
    else if (attempt?.status === 'error') {
      text = window.JournalI18n.render('journal.window.backup.status.window.explanation.the.latest.recorded.backup.attempt.failed');
      tone = 'attention';
    } else if (attempt?.status === 'ok' && attempt.error_reason === 'incomplete') {
      text = window.JournalI18n.render('journal.window.backup.status.window.explanation.the.latest.recorded.backup.copy.is');
      tone = 'attention';
    } else if (verification?.status === 'error') {
      text = window.JournalI18n.render('journal.window.backup.status.window.explanation.the.last.recorded.backup.spot.check');
      tone = 'attention';
    } else if (attempt?.status === 'ok' && date(attempt.time)) {
      text = window.JournalI18n.render('journal.window.backup.status.last.attempt.completed.date', [['completed_at', date(attempt.time)]]);
    } else {
      text = window.JournalI18n.render('journal.window.backup.status.window.explanation.backup.is.on.the.latest.attempt');
      tone = 'unavailable';
    }
    // An operation is activity, never proof that the next copy succeeded. A
    // prior failed/incomplete result stays visible even while work is active.
    if (operation && operation.phase !== 'done') {
      const cancelled = operationWasCancelled(operation);
      text = `${operationText(operation, 'sentence')} ${text}`;
      if (operation.phase === 'cleanup_pending'
          || (!cancelled && ['error', 'degraded', 'refused'].includes(operation.phase))) tone = 'attention';
    }
    return { text, tone, snapshot };
  }

  function renderSummary(element, snapshot) {
    const view = select(snapshot);
    if (element) {
      element.textContent = view.text;
      element.dataset.tone = view.tone;
    }
    return view;
  }

  function renderFacts(element, snapshot) {
    if (!element) return;
    const previous = element.querySelector('details');
    const wasOpen = Boolean(previous?.open);
    const restoreFocus = document.activeElement === previous?.querySelector('summary');
    element.replaceChildren();
    const view = select(snapshot);
    if (!view.snapshot) {
      if (restoreFocus) {
        const status = element.closest('section')?.querySelector('[data-backup-summary]');
        status?.setAttribute('tabindex', '-1');
        status?.focus();
      }
      return;
    }
    const attempt = snapshot.last_backup;
    const success = record(attempt) && Object.hasOwn(attempt, 'last_success')
      ? attempt.last_success : attempt?.status === 'ok' ? attempt : null;
    const operation = snapshot.operation;
    const facts = [
      [window.JournalI18n.render('journal.window.backup.status.fact.name'), !snapshot.enabled ? window.JournalI18n.render('journal.window.backup.status.fact.off') : Number.isInteger(snapshot.paused_at)
        ? (date(snapshot.paused_at) ? window.JournalI18n.render('journal.window.backup.status.paused.label.date', [['paused_at', date(snapshot.paused_at)]]) : window.JournalI18n.render('journal.window.backup.status.window.explanation.paused.by.you.date.isn.t'))
        : window.JournalI18n.render('journal.window.backup.status.fact.on')],
      [window.JournalI18n.render('journal.window.backup.status.fact.operation'), operation
        ? operationText(operation, 'label') : window.JournalI18n.render('journal.window.backup.status.fact.none.reported')],
      [window.JournalI18n.render('journal.window.backup.status.fact.latest.attempt'), resultText(attempt)],
      [window.JournalI18n.render('journal.window.backup.status.fact.last.success'), success?.status === 'ok' && date(success.time) ? resultText(success) : window.JournalI18n.render('journal.window.backup.status.outcome.unknown')],
      [window.JournalI18n.render('journal.window.backup.status.fact.last.check'), resultText(snapshot.last_verification, true)],
    ];
    const list = document.createElement('dl');
    for (const [label, value] of facts) {
      const row = document.createElement('div');
      const term = document.createElement('dt');
      term.textContent = label;
      const description = document.createElement('dd');
      description.textContent = value;
      row.append(term, description);
      list.appendChild(row);
    }
    element.appendChild(list);
    const note = document.createElement('p');
    note.className = 'backup-history-note';
    note.textContent = qualification;
    element.appendChild(note);
    const reasons = [
      [window.JournalI18n.render('journal.window.backup.status.fact.latest.attempt'), attempt?.error_reason, attempt?.detail],
      [window.JournalI18n.render('journal.window.backup.status.fact.last.success'), success?.error_reason, success?.detail],
      [window.JournalI18n.render('journal.window.backup.status.fact.last.check'), snapshot.last_verification?.reason, snapshot.last_verification?.detail],
      [window.JournalI18n.render('journal.window.backup.status.fact.operation'), operation?.reason_code, operation?.detail],
    ].filter(([, reason, detail]) => typeof reason === 'string' || typeof detail === 'string');
    if (reasons.length) {
      const details = document.createElement('details');
      const summary = document.createElement('summary');
      summary.textContent = window.JournalI18n.render('journal.window.backup.status.visible.text.technical.details');
      details.appendChild(summary);
      for (const [label, reason, detail] of reasons) {
        const line = document.createElement('p');
        line.textContent = `${label}: ${[reason, detail].filter(value => typeof value === 'string' && value).join(', ')}`;
        details.appendChild(line);
      }
      details.open = wasOpen;
      element.appendChild(details);
      if (restoreFocus) summary.focus();
    } else if (restoreFocus) {
      const status = element.closest('section')?.querySelector('[data-backup-summary]');
      status?.setAttribute('tabindex', '-1');
      status?.focus();
    }
  }

  const terminalPhases = new Set(['done', 'error', 'needs_subscription', 'degraded', 'refused']);

  function operationActive(operation) {
    return Boolean(operation) && !terminalPhases.has(operation.phase);
  }

  function cleanupRequired(state) {
    return state?.operation?.phase === 'cleanup_pending'
      || state?.cleanup_admission === 'pending'
      || state?.cleanup_admission === 'contended';
  }

  function managedMode(state) {
    return Boolean(state?.enabled) || state?.mode === 'operated';
  }

  function initialPanel(state) {
    if (operationActive(state?.operation) || cleanupRequired(state)) {
      return managedMode(state) ? 'management' : 'destination';
    }
    if (managedMode(state)) return 'management';
    if (state?.recovery_key_confirmed === true) return 'destination';
    return 'intro';
  }

  window.BackupStatus = {
    select,
    renderSummary,
    renderFacts,
    verificationSubsetLine,
    errorReasons: BACKUP_ERROR_REASONS,
    operationActive,
    cleanupRequired,
    managedMode,
    initialPanel,
  };
})();
