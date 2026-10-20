// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Read-only presentation shared by Backup and Health. Recorded results are
// history, not a coverage, freshness or destination-integrity assessment.
(function () {
  const BACKUP_ERROR_REASONS = {
    "incomplete": "some of your journal couldn't be read, so the copy that was stored is incomplete.",
    "repo_missing": "your backup destination couldn't be found.",
    "locked": "the backup lock was already held.",
    "auth_failed": "the backup key couldn't open the destination.",
    "timeout": "it ran out of time.",
    "_missing": "the reason wasn't recorded.",
    "restic_unavailable": "encrypted backup can't run on this computer.",
    "rclone_unavailable": "encrypted backup can't reach storage from this computer.",
    "backup_unavailable": "this computer can't set up or restore encrypted backup.",
    "failed": "the backup tool stopped with an error.",
    "unknown": "the backup tool stopped for a reason it didn't give.",
    "broker_unreachable": "encrypted backup couldn't be reached.",
    "broker_error": "encrypted backup didn't return usable settings.",
    "hosted_entitlement_inactive": "encrypted backup isn't turned on in the services portal.",
    "binding_invalid": "this computer's link to encrypted backup was refused.",
    "binding_superseded": "this computer's link to encrypted backup was replaced."
};
  const VERIFICATION_SUBSET = {
    one: "checked all of your backup this week.",
    part: "checked 1 of {total} parts of your backup.",
  };
  const unavailable = "backup status is unavailable right now.";
  const incomplete = BACKUP_ERROR_REASONS.incomplete;
  const qualification = "these are recorded results. they don't confirm what's at your current destination or whether it has been verified.";
  const operationKinds = {
    enable: 'backup setup', enable_hosted: 'backup setup', rotate: 'recovery key change',
    teardown: 'turning backup off', restore: 'restore', restore_hosted: 'restore',
    offload_restore: 'restore', offload_approve: 'archive approval',
  };
  const operationPhases = {
    setting_up: 'in progress', restoring: 'in progress', rotating: 'in progress',
    tearing_down: 'in progress', cancelling: 'cancelling', cleanup_pending: 'waiting for cleanup',
    done: 'completed', error: "couldn't finish", degraded: 'completed with problems',
    refused: "couldn't start", needs_subscription: 'waiting to finish in the services portal',
  };
  const record = value => value !== null && typeof value === 'object' && !Array.isArray(value);
  const optionalRecord = value => value === null || value === undefined || record(value);
  const date = time => typeof time === 'number' && Number.isFinite(time) && time > 0
    && Number.isFinite(new Date(time * 1000).getTime())
    ? new Date(time * 1000).toLocaleString() : null;

  // 'checked_subset' is restic's --read-data-subset selector, shaped 'n/m':
  // the run read one of m equal parts of the pack data, and n only says which
  // part this week's turn landed on. It is never a count of snapshots or of
  // parts checked, so the sentence reports one part out of m and nothing else.
  function verificationSubsetLine(value) {
    if (typeof value !== 'string' || !/^\d+\/\d+$/.test(value)) return '';
    const [selected, total] = value.split('/').map(Number);
    if (!Number.isSafeInteger(selected) || !Number.isSafeInteger(total)
        || selected <= 0 || total <= 0 || selected > total) return '';
    return total === 1 ? VERIFICATION_SUBSET.one
      : VERIFICATION_SUBSET.part.replace('{total}', String(total));
  }

  function resultText(result, verification = false) {
    if (!record(result)) return "isn't known";
    const outcomes = verification
      ? { ok: 'completed', error: 'failed', skipped: "couldn't run" }
      : { ok: 'completed', error: 'failed', never: 'no completed attempt recorded' };
    const outcome = Object.hasOwn(outcomes, result.status) ? outcomes[result.status] : null;
    if (!outcome) return "isn't known";
    const when = date(result.time);
    let text = when ? `${outcome}, ${when}` : `${outcome}, date isn't known`;
    if (!verification && result.status === 'error') {
      text += `. ${Object.hasOwn(BACKUP_ERROR_REASONS, result.error_reason)
        ? BACKUP_ERROR_REASONS[result.error_reason] : BACKUP_ERROR_REASONS._missing}`;
    }
    if (!verification && result.status === 'ok' && result.error_reason === 'incomplete') {
      text += `. ${incomplete}`;
    }
    if (verification && result.status === 'ok') {
      const subset = verificationSubsetLine(result.checked_subset);
      if (subset) text += `. ${subset}`;
    }
    return text;
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
    if (!snapshot.enabled) text = 'backup is off.';
    else if (attempt?.status === 'error') {
      text = 'the latest recorded backup attempt failed.';
      tone = 'attention';
    } else if (attempt?.status === 'ok' && attempt.error_reason === 'incomplete') {
      text = 'the latest recorded backup copy is incomplete.';
      tone = 'attention';
    } else if (verification?.status === 'error') {
      text = 'the last recorded backup spot-check failed.';
      tone = 'attention';
    } else if (attempt?.status === 'ok' && date(attempt.time)) {
      text = `the latest backup attempt completed, ${date(attempt.time)}.`;
    } else {
      text = "backup is on. the latest attempt isn't known.";
      tone = 'unavailable';
    }
    // An operation is activity, never proof that the next copy succeeded. A
    // prior failed/incomplete result stays visible even while work is active.
    if (operation && operation.phase !== 'done') {
      const cancelled = operation.phase === 'error' && operation.reason_code === 'cancelled';
      const activity = `${operationKinds[operation.kind]}: ${cancelled ? 'cancelled' : operationPhases[operation.phase]}.`;
      text = `${activity} ${text}`;
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
      ['backup', snapshot.enabled ? 'on' : 'off'],
      ['operation', operation
        ? `${operationKinds[operation.kind]}: ${operation.phase === 'error' && operation.reason_code === 'cancelled'
          ? 'cancelled' : operationPhases[operation.phase]}` : 'none reported'],
      ['latest attempt', resultText(attempt)],
      ['last successful copy', success?.status === 'ok' && date(success.time) ? resultText(success) : "isn't known"],
      ['last spot-check', resultText(snapshot.last_verification, true)],
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
      ['latest attempt', attempt?.error_reason, attempt?.detail],
      ['last successful copy', success?.error_reason, success?.detail],
      ['last spot-check', snapshot.last_verification?.reason, snapshot.last_verification?.detail],
      ['operation', operation?.reason_code, operation?.detail],
    ].filter(([, reason, detail]) => typeof reason === 'string' || typeof detail === 'string');
    if (reasons.length) {
      const details = document.createElement('details');
      const summary = document.createElement('summary');
      summary.textContent = 'technical details';
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

  window.BackupStatus = {
    select, renderSummary, renderFacts, verificationSubsetLine, errorReasons: BACKUP_ERROR_REASONS,
  };
})();
