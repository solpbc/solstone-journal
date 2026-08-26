// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const BACKUP_COPY = {
    "service_name": "encrypted backup",
    "brand_lock": "your journal is always private, only yours.",
    "intro": {
      "title": "encrypted backup",
      "subtitle": "make an encrypted copy of your journal somewhere safe. only you can read it.",
      "bullets": [
        "encrypted on your device before it leaves",
        "optional, always",
        "delete anytime"
      ],
      "optional": "your journal lives on your device; backup is optional.",
      "steps": "you'll save a recovery key, then choose where your backup lives."
    },
    "educate": {
      "stakes": "if you lose your recovery key, no one can recover your journal. not even sol pbc."
    },
    "key": {
      "theft_honesty": "anyone with your recovery key can read everything in your backup. store it like a master password.",
      "pm_caution": "only store your recovery key in a password manager you trust. sol pbc doesn't recommend a specific one.",
      "save_password_manager": "save to my password manager",
      "copy_label": "copy",
      "continue": "continue",
      "clipboard_caveat": "copying puts your recovery key on the clipboard. clear it after you save it."
    },
    "confirm": {
      "prompt": "enter the recovery key you just recorded.",
      "escape": "see key again"
    },
    "destination": {
      "repository_hint": "the restic repository for your bucket, e.g. s3:s3.amazonaws.com/your-bucket",
      "object_lock_warning": "don't enable Compliance-mode Object Lock on the bucket. it conflicts with backup pruning and lock cleanup. if you need immutability, use Governance mode.",
      "object_lock_summary": "bucket setup notes",
      "field_labels": {
        "repository": "repository",
        "backend": "backend",
        "s3": "S3",
        "b2": "B2",
        "access_key_id": "access key id",
        "secret_access_key": "secret access key",
        "b2_key_id": "key id",
        "b2_application_key": "application key"
      },
      "reason_labels": {
        "repo_exists": "destination is reachable and already set up.",
        "repo_missing": "destination is reachable and needs setup.",
        "auth_failed": "the destination rejected the key or credentials. check the recovery key and destination details.",
        "locked": "the destination is busy. try again shortly.",
        "timeout": "the destination took too long to respond. try again shortly.",
        "unreachable": "i couldn't reach the destination. check the repository path and try again."
      },
      "modes": {
        "byo": {
          "title": "your own",
          "desc": "your bucket, your credentials. the default.",
          "note": "sol pbc is never in the path."
        },
        "hosted": {
          "title": "operated by sol pbc",
          "desc": "sol pbc runs the off-device part for you.",
          "note": "sol pbc only ever holds an encrypted copy that was encrypted on your device before it leaves.",
          "cta": "set up backup →"
        }
      }
    },
    "hosted": {
      "setup_hint": "turning this on sets up encrypted backup, operated by sol pbc. turn it on from the services page that opens, then come back here. your journal stays on your device; only the encrypted copy goes to storage sol pbc operates, and sol pbc can never read it.",
      "location_label": "operated by sol pbc",
      "manage_label": "manage in your services →",
      "manage_url": "https://services.solstone.app/services/backup"
    },
    "management": {
      "destructive_action": "turn off & delete backup",
      "destructive_caption": "this deletes all your backup data. no new backups will be created.",
      "teardown_gate_lead": "{days} days of your journal ({size}) exist only in this backup. deleting the backup deletes them everywhere, forever.",
      "teardown_gate_unavailable_lead": "can't verify what exists only in this backup right now. deleting the backup may destroy days of your journal that exist nowhere else.",
      "teardown_gate_zero_lead": "nothing exists only in this backup right now. every day is still on your device.",
      "teardown_confirm_phrase": "delete",
      "teardown_confirm_prompt": "type delete to confirm",
      "teardown_restore_first_action": "restore everything first",
      "retention_hint": "how many recent copies to keep at each interval.",
      "status_labels": {
        "last_backup": "last backup",
        "last_prune": "last prune",
        "last_verification": "last verification",
        "storage_used": "storage used",
        "snapshot_history": "snapshot history",
        "not_available": "not yet available",
        "not_yet": "not yet",
        "ago": "{duration} ago",
        "enabled": "on",
        "disabled": "off",
        "destination": "where your backup lives",
        "retention": "retention",
        "setup": "set up your recovery key"
      },
      "retention_labels": {
        "hourly": "hourly",
        "daily": "daily",
        "weekly": "weekly",
        "monthly": "monthly"
      }
    },
    "restore": {
      "expectation": "a large restore can take a while. you can leave this page open while it runs.",
      "hosted": {
        "choose_lane": "where is the encrypted copy you're restoring from?",
        "byo_desc": "storage you bring yourself, reached with credentials you provide.",
        "operated_desc": "storage sol pbc runs, reached from your services.",
        "lane_title": "restore from sol pbc",
        "lane_intro": "enter your recovery key, then sign in to your services and confirm the restore.",
        "key_label": "your recovery key",
        "key_reassurance": "this journal uses your key and never sends it to sol pbc.",
        "primary": "sign in to restore →",
        "state_b": "waiting for the services page to approve your restore…",
        "state_b_refused": {
          "no_hosted_backup": "sol pbc isn't holding an encrypted copy for the sign-in you used.",
          "hosted_backup_expired": "sol pbc deleted that copy once 30 days had passed since encrypted backup stopped."
        },
        "status_retryable": "something went wrong. you can try again.",
        "errors": {
          "auth_failed": "that recovery key didn't unlock the backup. check the key, then try signing in again.",
          "cancelled": "restore cancelled. you can try again."
        },
        "popup_preflight_failed": "the sign-in window didn't open. try again, and check whether your browser blocked it.",
        "key_required": "enter your recovery key before continuing."
      }
    },
    "status": {
      "last_backup": {
        "never_run": "no completed backup recorded on this computer",
        "failed": "backup attempt failed {duration} ago"
      },
      "last_verification": {
        "ok": "backup spot-checked {duration} ago",
        "failed": "spot-check failed {duration} ago",
        "skipped": "spot-check couldn't run {duration} ago"
      },
      "error_reasons": {
        "incomplete": "some of your journal couldn't be read, so the copy that was stored is incomplete.",
        "repo_missing": "your backup destination couldn't be found.",
        "locked": "the backup lock was already held.",
        "auth_failed": "your destination credentials were refused.",
        "timeout": "it ran out of time.",
        "_missing": "the reason wasn't recorded.",
        "restic_unavailable": "encrypted backup can't run on this computer.",
        "rclone_unavailable": "encrypted backup can't reach storage from this computer.",
        "backup_unavailable": "this computer can't set up or restore encrypted backup."
      }
    },
    "offload": {
      "title": "media offload",
      "stakes": "after this, your backup holds the only copy of your older days. if you lose your recovery key, no one can recover them. not even sol pbc.",
      "stalled_lead": "offload is paused: your backup isn't working. nothing has been deleted.",
      "backup_only_label": "in your backup",
      "restore_expectation": "restoring {size} from your backup. a large restore can take a while.",
      "disable_note": "this stops. days already in your backup stay there, protected and restorable.",
      "unavailable_lead": "can't read offload status right now.",
      "action_error": "media offload couldn't finish. check backup setup, then try again.",
      "invalid_limits": "enter a positive number for each limit, then save again.",
      "enable_hint": "choose how much older media can leave this device after backup verification.",
      "not_ready": "turn on encrypted backup and confirm your recovery key before using media offload.",
      "labels": {
        "budget_gb": "raw media budget",
        "floor_gb": "device free-space floor",
        "budget_short": "budget",
        "floor_short": "floor",
        "raw_media": "on this device",
        "device_free": "device free",
        "device_total": "device total",
        "last_offload": "last offload",
        "last_verify": "last verification",
        "last_restore": "last restore",
        "days": "offloaded days",
        "mb_suffix": "MB",
        "under_1mb": "under 1 MB",
        "gb_suffix": "GB"
      },
      "actions": {
        "enable": "turn on media offload",
        "save": "save limits",
        "disable": "turn off media offload",
        "restore_day": "restore this day"
      },
      "messages": {
        "saved": "saved",
        "empty_days": "no offloaded media yet.",
        "show_all_days": "show all {count} days",
        "degraded": "some of the record of what's in your backup couldn't be read. these days may hold more than shown."
      },
      "stall_reason_labels": {
        "backup_not_ready": "encrypted backup needs to finish setup before media offload can run.",
        "backup_failing": "encrypted backup needs a healthy recent copy before media offload can run.",
        "verification_missing": "backup verification needs to run before media offload can start.",
        "verification_overdue": "backup verification is overdue. media offload will wait for a fresh verification.",
        "verification_failed": "backup verification failed. media offload will wait for a healthy verification.",
        "locked": "media offload is waiting for backup maintenance to finish.",
        "archive_failed": "media offload could not add older media to encrypted backup.",
        "confirm_failed": "media offload could not verify the backed-up media.",
        "confirm_tool_failed": "media offload could not run the verification tool.",
        "unexpected_error": "media offload stopped unexpectedly. try again after backup maintenance runs."
      },
      "restore_reason_labels": {
        "auth_failed": "encrypted backup rejected the recovery key or credentials.",
        "backup_not_ready": "encrypted backup is not ready to restore media.",
        "failed": "media restore could not finish.",
        "insufficient_free_space": "this device needs more free space before restoring media.",
        "ledger_degraded": "some of the record of what's in your backup couldn't be read, so a restore can't be trusted to be complete. try again after the next backup runs.",
        "locked": "media restore is waiting for backup maintenance to finish.",
        "missing_file_after_restore": "media restore finished, but a file was still missing.",
        "nothing_to_restore": "nothing to restore for that day.",
        "repo_missing": "encrypted backup could not find the repository.",
        "restic_unavailable": "the backup tool is not available yet.",
        "rclone_unavailable": "the storage access tool is not available yet.",
        "segment_missing": "that day is no longer available locally.",
        "timeout": "media restore took too long. try again later.",
        "verification_failed": "restored media did not match the backup checksum."
      }
    },
    "phase_labels": {
      "setting_up": "setting up your backup…",
      "restoring": "restoring your journal…",
      "rotating": "making a new recovery key…",
      "tearing_down": "turning off…",
      "done": "done",
      "degraded": "restored, but not verified",
      "refused": "restore wasn't available",
      "error": "couldn't finish",
      "loading": "loading…",
      "empty": "not set up yet"
    },
    "operation_reason_labels": {
      "backup_busy": "another backup task is already running. try again in a moment.",
      "backup_not_confirmed": "confirm your recovery key before turning on backup.",
      "backup_operation_failed": "i couldn't finish that backup action. check the recovery key and destination, then try again.",
      "backup_unavailable": "i couldn't ask the background service to start a backup. start it, then try again.",
      "invalid_key": "that recovery key didn't unlock the backup. re-enter the key from your saved copy.",
      "invalid_config_value": "use non-negative whole numbers, then save again.",
      "invalid_operation_for_state": "finish the current backup setup step, then try again.",
      "invalid_request_value": "check the destination details and try again.",
      "restic_unavailable": "i couldn't prepare the backup tool. try again after setup finishes.",
      "repo_missing": "i couldn't find a backup repository at that destination.",
      "auth_failed": "that recovery key didn't unlock the backup. check the key first, then the destination details.",
      "locked": "the destination is busy. try again shortly.",
      "timeout": "the destination took too long to respond. try again shortly.",
      "failed": "i couldn't finish the backup action. check the recovery key and destination, then try again.",
      "incomplete": "the backup action didn't finish. you can try again.",
      "body_rebuild_failed": "your journal was restored, but your body history couldn't be rebuilt from it. the restore wasn't finalized.",
      "integrity_failed": "your journal was restored to this device, but the backup copy failed its integrity check and may be damaged.",
      "integrity_unverified": "your journal was restored to this device, but the integrity check couldn't run (the backup was busy or timed out), so the backup copy is unverified.",
      "missing_required_field": "fill in the required fields, then try again.",
      "recovery_key_mismatch": "that didn't match your recovery key. re-enter the key from your saved copy.",
      "expired": "the approval took too long. try again.",
      "malformed": "the response couldn't be read. update your journal, then try again.",
      "network_error": "the services page couldn't be reached. check your connection, then try again.",
      "broker_unreachable": "encrypted backup couldn't be reached. check your connection, then try again.",
      "broker_error": "encrypted backup didn't return usable settings. try again shortly.",
      "hosted_entitlement_inactive": "set up backup on the services page that opens, then try again."
    },
    "action_labels": {
      "start": "get started",
      "understand": "i understand",
      "save_destination": "save destination",
      "enable": "turn on backup",
      "backup_now": "back up now",
      "view_key": "view recovery key",
      "rotate_key": "regenerate recovery key",
      "teardown": "turn off & delete backup",
      "save_retention": "save retention",
      "restore": "restore",
      "try_again": "try again",
      "cancel": "cancel"
    },
    "error_intro": "start with the recovery key. if it still fails, check the destination details."
  };
  const STATUS_SELECTION_TABLE = {
    "null|null": { "backup": { "copy_key": "status.last_backup.never_run", "duration_source": null }, "verification": { "copy_key": "management.status_labels.not_yet", "duration_source": null } },
    "null|ok": { "backup": { "copy_key": "status.last_backup.never_run", "duration_source": null }, "verification": { "copy_key": "management.status_labels.not_yet", "duration_source": null } },
    "null|skipped": { "backup": { "copy_key": "status.last_backup.never_run", "duration_source": null }, "verification": { "copy_key": "management.status_labels.not_yet", "duration_source": null } },
    "null|error": { "backup": { "copy_key": "status.last_backup.never_run", "duration_source": null }, "verification": { "copy_key": "management.status_labels.not_yet", "duration_source": null } },
    "ok|null": { "backup": { "copy_key": "management.status_labels.ago", "duration_source": "last_backup.time" }, "verification": { "copy_key": "management.status_labels.not_yet", "duration_source": null } },
    "ok|ok": { "backup": { "copy_key": "management.status_labels.ago", "duration_source": "last_backup.time" }, "verification": { "copy_key": "status.last_verification.ok", "duration_source": "last_verification.time" } },
    "ok|skipped": { "backup": { "copy_key": "management.status_labels.ago", "duration_source": "last_backup.time" }, "verification": { "copy_key": "status.last_verification.skipped", "duration_source": "last_verification.time" } },
    "ok|error": { "backup": { "copy_key": "management.status_labels.ago", "duration_source": "last_backup.time" }, "verification": { "copy_key": "status.last_verification.failed", "duration_source": "last_verification.time" } },
    "error|null": { "backup": { "copy_key": "status.last_backup.failed", "duration_source": "last_backup.time" }, "verification": { "copy_key": "management.status_labels.not_yet", "duration_source": null } },
    "error|ok": { "backup": { "copy_key": "status.last_backup.failed", "duration_source": "last_backup.time" }, "verification": { "copy_key": "status.last_verification.ok", "duration_source": "last_verification.time" } },
    "error|skipped": { "backup": { "copy_key": "status.last_backup.failed", "duration_source": "last_backup.time" }, "verification": { "copy_key": "status.last_verification.skipped", "duration_source": "last_verification.time" } },
    "error|error": { "backup": { "copy_key": "status.last_backup.failed", "duration_source": "last_backup.time" }, "verification": { "copy_key": "status.last_verification.failed", "duration_source": "last_verification.time" } }
  };
  const copy = BACKUP_COPY;
  const BYTES_PER_GB = 1000000000;
  const BYTES_PER_MB = 1000000;
  const MAX_OFFLOAD_DAY_ROWS = 21;
  let state = {};
  let offloadState = { status: 'loading', payload: null };
  let currentRecoveryDisplay = '';
  let offloadDaysExpanded = false;
  let pollTimer = null;
  let restoreLane = null;
  const hostedRestoreAttempt = {
    stage: 'idle',
    capability: null,
    popup: null,
    message: '',
    tone: 'neutral',
    refusedReason: null,
    dismissedRefusal: false,
    fieldError: false,
    pendingPrepare: null,
  };

  const root = document.querySelector('[data-backup-root]');
  if (!root) return;

  function logMissingCopy(path) {
    const error = new Error(`missing backup copy path: ${path}`);
    if (window.logError) {
      window.logError(error, { context: 'backup copy render', path });
    } else if (window.console && window.console.error) {
      window.console.error(error);
    }
  }

  function copyValue(source, path) {
    let cursor = source;
    for (const part of path.split('.')) {
      if (cursor == null || typeof cursor !== 'object' || !(part in cursor)) {
        logMissingCopy(path);
        return undefined;
      }
      cursor = cursor[part];
    }
    if (cursor === undefined) logMissingCopy(path);
    return cursor;
  }

  function applyTextCopy(target, selector, attr, setter, source) {
    for (const element of target.querySelectorAll(selector)) {
      const path = element.getAttribute(attr);
      const value = path ? copyValue(source, path) : undefined;
      if (value !== undefined) setter(element, String(value));
    }
  }

  function applyCopy(target, source) {
    applyTextCopy(target, '[data-copy]', 'data-copy', (element, value) => {
      element.textContent = value;
    }, source);
    applyTextCopy(target, '[data-copy-href]', 'data-copy-href', (element, value) => {
      element.setAttribute('href', value);
    }, source);
    applyTextCopy(target, '[data-copy-aria-label]', 'data-copy-aria-label', (element, value) => {
      element.setAttribute('aria-label', value);
    }, source);
  }

  function renderIntroBullets(target, source) {
    const list = target.querySelector('[data-copy-list="intro.bullets"]');
    if (!list) return;
    const bullets = copyValue(source, 'intro.bullets');
    list.replaceChildren();
    if (!Array.isArray(bullets)) {
      logMissingCopy('intro.bullets');
      return;
    }
    for (const bullet of bullets) {
      const item = document.createElement('li');
      item.textContent = String(bullet);
      list.append(item);
    }
  }

  function renderRetentionGrid(target, source) {
    const grid = target.querySelector('[data-retention-grid]');
    if (!grid) return;
    const labels = copyValue(source, 'management.retention_labels');
    grid.replaceChildren();
    if (!labels || typeof labels !== 'object' || Array.isArray(labels)) {
      logMissingCopy('management.retention_labels');
      return;
    }
    for (const [key, labelText] of Object.entries(labels)) {
      const label = document.createElement('label');
      const text = document.createElement('span');
      text.textContent = String(labelText);
      const input = document.createElement('input');
      input.setAttribute('name', key);
      input.setAttribute('data-retention-field', key);
      input.setAttribute('type', 'number');
      input.setAttribute('min', '0');
      input.setAttribute('step', '1');
      label.append(text, input);
      grid.append(label);
    }
  }

  const phaseLabels = copy.phase_labels || {};
  const actionLabels = copy.action_labels || {};
  const destinationLabels = (copy.destination && copy.destination.reason_labels) || {};
  const operationLabels = copy.operation_reason_labels || {};
  const managementCopy = copy.management || {};
  const statusLabels = managementCopy.status_labels || {};
  const hostedCopy = copy.hosted || {};
  const restoreHostedCopy = (copy.restore && copy.restore.hosted) || {};
  const offloadCopy = copy.offload || {};
  const offloadLabels = offloadCopy.labels || {};
  const offloadMessages = offloadCopy.messages || {};
  const offloadStallLabels = offloadCopy.stall_reason_labels || {};
  const offloadRestoreLabels = offloadCopy.restore_reason_labels || {};
  const offloadRouteErrorReasons = new Set([
    'invalid_config_value',
    'backup_not_confirmed',
    'invalid_operation_for_state',
    'backup_busy',
  ]);
  const terminalPhases = new Set(['done', 'error', 'needs_subscription', 'degraded', 'refused']);

  function panel(name) {
    return root.querySelector(`[data-backup-panel="${name}"]`);
  }

  function showPanel(name) {
    for (const item of root.querySelectorAll('[data-backup-panel]')) {
      item.hidden = item.getAttribute('data-backup-panel') !== name;
    }
  }

  function setText(selector, value) {
    const element = root.querySelector(selector);
    if (element) element.textContent = value || '';
  }

  function setElementHidden(selector, hidden) {
    const element = root.querySelector(selector);
    if (element) element.hidden = hidden;
  }

  function setTextWithTitle(selector, display) {
    const element = root.querySelector(selector);
    if (!element) return;
    element.textContent = (display && display.text) || '';
    const title = display && display.title;
    if (title) {
      element.title = title;
    } else {
      element.removeAttribute('title');
    }
  }

  function operationActive(operation) {
    return operation && !terminalPhases.has(operation.phase);
  }

  function managedMode() {
    return state.enabled || state.mode === 'operated';
  }

  function labelForPhase(phase) {
    return phaseLabels[phase] || phase || '';
  }

  function reasonLabel(reason) {
    return operationLabels[reason] || destinationLabels[reason] || copy.error_intro || '';
  }

  function backupErrorReasonLine(errorReason) {
    const labels = (copy.status && copy.status.error_reasons) || {};
    if (errorReason === null || errorReason === undefined) {
      return labels._missing || null;
    }
    return Object.prototype.hasOwnProperty.call(labels, errorReason) ? labels[errorReason] : null;
  }

  function offloadActionError(err) {
    const reason = err && err.reason_code;
    if (reason === 'invalid_config_value') {
      return offloadCopy.invalid_limits || '';
    }
    if (offloadRouteErrorReasons.has(reason) && operationLabels[reason]) {
      return operationLabels[reason];
    }
    return offloadCopy.action_error || '';
  }

  function maybeOpenPortal(payload) {
    const operation = payload && payload.operation;
    if (operation && operation.portal_url) {
      window.open(operation.portal_url, '_blank', 'noopener');
    }
  }

  function hostedRestoreControls() {
    return {
      field: root.querySelector('[data-restore-hosted-input]'),
      hint: root.querySelector('[data-hosted-restore-hint]'),
      keyControl: root.querySelector('[data-hosted-restore-key-control]'),
      keyReassurance: root.querySelector('[data-hosted-restore-key-reassurance]'),
      primary: root.querySelector('[data-action="restore-hosted-unbound-start"]'),
      attemptCancel: root.querySelector('[data-action="cancel-hosted-restore-attempt"]'),
      outcome: root.querySelector('[data-hosted-restore-outcome]'),
    };
  }

  function hostedRestoreLaneSelected() {
    return restoreLane === 'operated';
  }

  function hostedRestoreAttemptInFlight() {
    return hostedRestoreAttempt.stage !== 'idle' && hostedRestoreAttempt.stage !== 'terminal';
  }

  function setHostedRestoreOutcome(message, tone) {
    hostedRestoreAttempt.message = message || '';
    hostedRestoreAttempt.tone = tone || 'neutral';
  }

  function clearHostedRestoreFieldError() {
    hostedRestoreAttempt.fieldError = false;
    const { field } = hostedRestoreControls();
    if (!field) return;
    field.removeAttribute('aria-invalid');
    field.removeAttribute('aria-errormessage');
  }

  function setHostedRestoreFieldError(message) {
    const { field, outcome } = hostedRestoreControls();
    hostedRestoreAttempt.fieldError = true;
    setHostedRestoreOutcome(message, 'error');
    if (!field || !outcome) return;
    field.setAttribute('aria-invalid', 'true');
    field.setAttribute('aria-errormessage', outcome.id);
  }

  function renderHostedRestoreAttempt() {
    const { field, hint, keyControl, keyReassurance, primary, attemptCancel, outcome } = hostedRestoreControls();
    if (!field || !primary || !outcome) return;
    const refused = hostedRestoreAttempt.stage === 'terminal' && hostedRestoreAttempt.refusedReason;
    if (hint) hint.hidden = Boolean(refused);
    if (keyControl) keyControl.hidden = Boolean(refused);
    if (keyReassurance) keyReassurance.hidden = Boolean(refused);
    primary.hidden = Boolean(refused);
    primary.disabled = Boolean(refused) || hostedRestoreAttemptInFlight() || field.value.trim() === '';
    if (attemptCancel) attemptCancel.hidden = !hostedRestoreAttempt.capability || Boolean(refused);
    outcome.textContent = hostedRestoreAttempt.message;
    outcome.hidden = !hostedRestoreAttempt.message;
    outcome.classList.toggle('is-error', hostedRestoreAttempt.tone === 'error');
    outcome.classList.toggle('is-active', hostedRestoreAttempt.tone === 'active');
    if (hostedRestoreAttempt.fieldError) {
      field.setAttribute('aria-invalid', 'true');
      field.setAttribute('aria-errormessage', outcome.id);
    } else {
      field.removeAttribute('aria-invalid');
      field.removeAttribute('aria-errormessage');
    }
  }

  function closeHostedRestorePopup() {
    const popup = hostedRestoreAttempt.popup;
    hostedRestoreAttempt.popup = null;
    if (!popup || popup.closed || typeof popup.close !== 'function') return;
    try {
      popup.close();
    } catch (_err) {
      // A cross-origin popup can become uncontrollable after navigation.
    }
  }

  function resetHostedRestoreAttempt() {
    closeHostedRestorePopup();
    hostedRestoreAttempt.stage = 'idle';
    hostedRestoreAttempt.capability = null;
    hostedRestoreAttempt.refusedReason = null;
  }

  function dismissHostedRestoreRefusal() {
    resetHostedRestoreAttempt();
    hostedRestoreAttempt.dismissedRefusal = true;
    setHostedRestoreOutcome('', 'neutral');
    clearHostedRestoreFieldError();
  }

  function validHostedRestorePortal(value) {
    try {
      const url = new URL(value);
      if (url.protocol !== 'https:' || !url.hostname || url.username || url.password) return null;
      const expected = root.getAttribute('data-hosted-portal-origin');
      if (expected && url.origin !== new URL(expected).origin) return null;
      return url.href;
    } catch (_err) {
      return null;
    }
  }

  async function cancelHostedRestoreAttempt(options) {
    const settings = options || {};
    const capability = hostedRestoreAttempt.capability;
    if (!capability && hostedRestoreAttempt.stage === 'preparing' && hostedRestoreAttempt.pendingPrepare) {
      hostedRestoreAttempt.pendingPrepare.cancelled = true;
    }
    if (capability) {
      try {
        const payload = await postJson('/app/backup/restore-hosted/cancel', { capability });
        applyPayload(payload);
      } catch (_err) {
        // Cancellation is best-effort. A resolved or expired lease is already clean server-side.
      }
    }
    resetHostedRestoreAttempt();
    if (settings.showCancelled) {
      setHostedRestoreOutcome((restoreHostedCopy.errors && restoreHostedCopy.errors.cancelled) || '', 'neutral');
    } else if (!settings.preserveOutcome) {
      setHostedRestoreOutcome('', 'neutral');
    }
    renderHostedRestoreAttempt();
  }

  async function failHostedRestoreAttempt(err, options) {
    const reason = err && err.reason_code;
    if (hostedRestoreAttempt.capability) {
      await cancelHostedRestoreAttempt({ preserveOutcome: true });
    } else {
      resetHostedRestoreAttempt();
    }
    if (reason === 'invalid_key') {
      setHostedRestoreFieldError(operationLabels.invalid_key || '');
    } else if (reason === 'auth_failed') {
      setHostedRestoreOutcome((restoreHostedCopy.errors && restoreHostedCopy.errors.auth_failed) || '', 'error');
    } else if (options && options.popupPreflight) {
      setHostedRestoreOutcome(restoreHostedCopy.popup_preflight_failed || '', 'error');
    } else {
      setHostedRestoreOutcome(restoreHostedCopy.status_retryable || '', 'error');
    }
    renderHostedRestoreAttempt();
  }

  function renderHostedRestoreOperation() {
    const operation = state.operation;
    if (!hostedRestoreLaneSelected() || !operation || operation.kind !== 'restore_hosted') return;
    if (operation.phase === 'refused') {
      if (hostedRestoreAttempt.dismissedRefusal) {
        renderHostedRestoreAttempt();
        return;
      }
      resetHostedRestoreAttempt();
      hostedRestoreAttempt.stage = 'terminal';
      hostedRestoreAttempt.refusedReason = operation.reason_code || '';
      const refused = restoreHostedCopy.state_b_refused || {};
      setHostedRestoreOutcome(refused[operation.reason_code] || restoreHostedCopy.status_retryable || '', 'error');
      clearHostedRestoreFieldError();
    } else if (operation.phase === 'needs_subscription') {
      resetHostedRestoreAttempt();
      setHostedRestoreOutcome('', 'neutral');
      clearHostedRestoreFieldError();
    } else if (operation.phase === 'error') {
      resetHostedRestoreAttempt();
      if (operation.reason_code === 'invalid_key') {
        setHostedRestoreFieldError(operationLabels.invalid_key || '');
      } else if (operation.reason_code === 'auth_failed') {
        setHostedRestoreOutcome((restoreHostedCopy.errors && restoreHostedCopy.errors.auth_failed) || '', 'error');
      } else {
        setHostedRestoreOutcome(restoreHostedCopy.status_retryable || '', 'error');
      }
    } else if (!operationActive(operation)) {
      resetHostedRestoreAttempt();
      setHostedRestoreOutcome('', 'neutral');
      clearHostedRestoreFieldError();
    }
    renderHostedRestoreAttempt();
  }

  async function beginHostedRestoreAttempt() {
    const { field } = hostedRestoreControls();
    if (!field || hostedRestoreAttemptInFlight() || hostedRestoreAttempt.stage === 'terminal') return;
    if (field.value.trim() === '') {
      setHostedRestoreFieldError(restoreHostedCopy.key_required || '');
      renderHostedRestoreAttempt();
      return;
    }
    clearHostedRestoreFieldError();
    hostedRestoreAttempt.dismissedRefusal = false;
    setHostedRestoreOutcome('', 'neutral');

    if (state.hosted && state.hosted.bound === true) {
      hostedRestoreAttempt.stage = 'polling';
      setHostedRestoreOutcome(restoreHostedCopy.state_b || '', 'active');
      renderHostedRestoreAttempt();
      try {
        await startOperation('/app/backup/restore-hosted', { recovery_key: field.value });
      } catch (err) {
        await failHostedRestoreAttempt(err);
      }
      return;
    }

    let popup;
    let pendingPrepare = null;
    try {
      popup = window.open('', '_blank');
    } catch (err) {
      await failHostedRestoreAttempt(err, { popupPreflight: true });
      return;
    }
    if (!popup || popup.closed) {
      await failHostedRestoreAttempt(null, { popupPreflight: true });
      return;
    }
    hostedRestoreAttempt.popup = popup;
    hostedRestoreAttempt.stage = 'popup_opened';
    setHostedRestoreOutcome('', 'neutral');
    renderHostedRestoreAttempt();

    try {
      hostedRestoreAttempt.stage = 'verifying_popup';
      if (popup.closed) {
        await failHostedRestoreAttempt(null, { popupPreflight: true });
        return;
      }
      hostedRestoreAttempt.stage = 'preparing';
      pendingPrepare = { cancelled: false };
      hostedRestoreAttempt.pendingPrepare = pendingPrepare;
      setHostedRestoreOutcome(restoreHostedCopy.state_b || '', 'active');
      renderHostedRestoreAttempt();
      const prepared = await postJson('/app/backup/restore-hosted/prepare');
      if (!prepared || typeof prepared.capability !== 'string' || prepared.capability === '') {
        throw { reason_code: 'restore_prepare_invalid_capability' };
      }
      if (pendingPrepare.cancelled) {
        if (hostedRestoreAttempt.pendingPrepare === pendingPrepare) {
          hostedRestoreAttempt.pendingPrepare = null;
        }
        try {
          await postJson('/app/backup/restore-hosted/cancel', { capability: prepared.capability });
        } catch (_err) {
          // A resolved or expired lease is already clean server-side.
        }
        return;
      }
      if (hostedRestoreAttempt.stage !== 'preparing') return;
      if (hostedRestoreAttempt.pendingPrepare === pendingPrepare) {
        hostedRestoreAttempt.pendingPrepare = null;
      }
      hostedRestoreAttempt.capability = prepared.capability;
      renderHostedRestoreAttempt();

      const keyed = await postJson('/app/backup/restore-hosted/key', {
        capability: hostedRestoreAttempt.capability,
        recovery_key: field.value,
      });
      if (hostedRestoreAttempt.stage !== 'preparing') return;
      const portalUrl = validHostedRestorePortal(keyed && keyed.portal_url);
      if (!portalUrl) {
        throw { reason_code: 'restore_prepare_invalid_portal' };
      }
      hostedRestoreAttempt.stage = 'key_submitted';
      renderHostedRestoreAttempt();
      await new Promise((resolve) => window.setTimeout(resolve, 0));
      if (hostedRestoreAttempt.stage !== 'key_submitted') return;

      hostedRestoreAttempt.stage = 'navigating';
      setHostedRestoreOutcome(restoreHostedCopy.state_b || '', 'active');
      renderHostedRestoreAttempt();
      popup.opener = null;
      if (popup.location && typeof popup.location.replace === 'function') {
        popup.location.replace(portalUrl);
      } else {
        popup.location = portalUrl;
      }

      hostedRestoreAttempt.stage = 'arming';
      renderHostedRestoreAttempt();
      const armed = await postJson('/app/backup/restore-hosted/arm', {
        capability: hostedRestoreAttempt.capability,
      });
      if (hostedRestoreAttempt.stage !== 'arming') return;
      applyPayload(armed);

      hostedRestoreAttempt.stage = 'activating';
      renderHostedRestoreAttempt();
      const activated = await postJson('/app/backup/restore-hosted/activate', {
        capability: hostedRestoreAttempt.capability,
      });
      if (hostedRestoreAttempt.stage !== 'activating') return;
      hostedRestoreAttempt.stage = 'polling';
      setHostedRestoreOutcome(restoreHostedCopy.state_b || '', 'active');
      applyPayload(activated);
      pollUntilTerminal();
    } catch (err) {
      if (hostedRestoreAttempt.pendingPrepare === pendingPrepare) {
        hostedRestoreAttempt.pendingPrepare = null;
      }
      if (hostedRestoreAttempt.stage !== 'idle') await failHostedRestoreAttempt(err);
    }
  }

  function renderHostedLocation() {
    const section = root.querySelector('[data-hosted-location-section]');
    if (!section) return;
    const hosted = state.hosted || {};
    const operated = state.mode === 'operated' && hosted.bound;
    section.hidden = !operated;
    if (operated) {
      setText('[data-hosted-location]', hostedCopy.location_label || '');
    }
  }

  function formatTime(value) {
    if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
      return statusLabels.not_yet || '';
    }
    try {
      return new Date(value * 1000).toLocaleString();
    } catch (_err) {
      return statusLabels.not_yet || '';
    }
  }

  function validTimestamp(value) {
    return typeof value === 'number' && Number.isFinite(value) && value > 0;
  }

  function timestampRelativeDuration(value) {
    const elapsed = Date.now() - value * 1000;
    if (Number.isFinite(elapsed) && elapsed >= 0 && typeof relativeTime === 'function') {
      return relativeTime(elapsed);
    }
    return null;
  }

  function timestampDisplay(value) {
    if (!validTimestamp(value)) {
      return { text: statusLabels.not_yet || '', title: '' };
    }
    const title = formatTime(value);
    const duration = timestampRelativeDuration(value);
    if (title && duration !== null) {
      return {
        text: (statusLabels.ago || '').replace('{duration}', duration),
        title,
      };
    }
    return { text: title, title };
  }

  function resolveCopyPath(source, path) {
    return path.split('.').reduce((value, key) => value && value[key], source);
  }

  function stateValue(path) {
    return path.split('.').reduce((value, key) => value && value[key], state);
  }

  function selectStatusCopy(lastBackupStatus, lastVerificationStatus) {
    return STATUS_SELECTION_TABLE[`${lastBackupStatus ?? 'null'}|${lastVerificationStatus ?? 'null'}`];
  }

  function statusSelectionDisplay(selection) {
    const template = resolveCopyPath(copy, selection.copy_key) || '';
    if (!selection.duration_source) return { text: template, title: '' };
    const value = stateValue(selection.duration_source);
    if (!validTimestamp(value)) return timestampDisplay(value);
    const title = formatTime(value);
    const duration = timestampRelativeDuration(value) || title;
    return { text: template.replace('{duration}', duration), title };
  }

  function ensureBackupStatusNodes() {
    const backup = root.querySelector('[data-last-backup]');
    let reason = document.querySelector('[data-last-backup-reason]');
    if (backup && !reason) {
      reason = document.createElement('p');
      reason.setAttribute('data-last-backup-reason', '');
      reason.hidden = true;
      backup.insertAdjacentElement('afterend', reason);
    }
    const grid = root.querySelector('.backup-management-grid');
    let verification = document.querySelector('[data-last-verification]');
    if (grid && !verification) {
      const section = document.createElement('section');
      const heading = document.createElement('h2');
      verification = document.createElement('p');
      heading.textContent = resolveCopyPath(copy, 'management.status_labels.last_verification') || '';
      verification.setAttribute('data-last-verification', '');
      section.append(heading, verification);
      grid.append(section);
    }
    return { backup, reason, verification };
  }

  function formatDay(value) {
    if (typeof value !== 'string' || value.length !== 8) return value || '';
    return `${value.slice(0, 4)}-${value.slice(4, 6)}-${value.slice(6, 8)}`;
  }

  function formatDisplayDay(value) {
    if (typeof value !== 'string' || value.length !== 8) return value || '';
    if (typeof formatDateShort === 'function') {
      const formatted = formatDateShort(value);
      if (formatted && formatted !== value) return formatted;
    }
    return formatDay(value);
  }

  function formatGbInput(bytes) {
    if (typeof bytes !== 'number' || !Number.isFinite(bytes) || bytes <= 0) {
      return '';
    }
    const value = bytes / BYTES_PER_GB;
    const rounded = Math.round(value * 100) / 100;
    if (rounded <= 0) return '0.01';
    return String(rounded);
  }

  function formatBytes(bytes) {
    if (typeof bytes !== 'number' || !Number.isFinite(bytes) || bytes < 0) {
      return statusLabels.not_available || '';
    }
    if (bytes === 0) {
      const suffix = offloadLabels.gb_suffix || '';
      return `0${suffix ? ' ' + suffix : ''}`;
    }
    if (bytes < BYTES_PER_MB) {
      return offloadLabels.under_1mb || '';
    }
    const isGb = bytes >= BYTES_PER_GB;
    const suffix = isGb ? offloadLabels.gb_suffix || '' : offloadLabels.mb_suffix || '';
    const divisor = isGb ? BYTES_PER_GB : BYTES_PER_MB;
    const value = bytes / divisor;
    const rounded = value >= 10 ? Math.round(value) : Math.round(value * 10) / 10;
    return `${rounded.toLocaleString()}${suffix ? ' ' + suffix : ''}`;
  }

  function gbToBytes(value) {
    const parsed = Number.parseFloat(value);
    if (!Number.isFinite(parsed) || parsed <= 0) return 0;
    return Math.round(parsed * BYTES_PER_GB);
  }

  function offloadReady() {
    return state.enabled === true && state.recovery_key_confirmed === true;
  }

  function reasonFromOffloadMap(labels, path, reason) {
    if (!reason) return '';
    const label = labels[reason];
    if (!label) logMissingCopy(`${path}.${reason}`);
    return label || '';
  }

  function offloadStallReasonLabel(reason) {
    return reasonFromOffloadMap(offloadStallLabels, 'offload.stall_reason_labels', reason);
  }

  function offloadRestoreReasonLabel(reason) {
    return reasonFromOffloadMap(offloadRestoreLabels, 'offload.restore_reason_labels', reason);
  }

  function formatWorkingProofDisplay(result) {
    if (!result || !result.last_ok_time) return { text: statusLabels.not_yet || '', title: '' };
    return timestampDisplay(result.last_ok_time);
  }

  function formatRestoreResultDisplay(result) {
    if (!result || !result.status) return { text: statusLabels.not_yet || '', title: '' };
    const display = timestampDisplay(result.time);
    const parts = [display.text];
    if (result.status !== 'ok') {
      const reason = offloadRestoreReasonLabel(result.reason);
      if (reason) parts.push(reason);
    }
    return { text: parts.filter(Boolean).join(' · '), title: display.title };
  }

  function offloadRestoreExpectation(bytes) {
    if (typeof bytes !== 'number' || !Number.isFinite(bytes) || bytes < 0) return '';
    return (offloadCopy.restore_expectation || '').replace('{size}', formatBytes(bytes));
  }

  function teardownConfirmPhrase() {
    return managementCopy.teardown_confirm_phrase || '';
  }

  function teardownInputValue() {
    const input = root.querySelector('[data-teardown-input]');
    return input && typeof input.value === 'string' ? input.value : '';
  }

  function teardownConfirmSatisfied() {
    const phrase = teardownConfirmPhrase();
    return phrase !== '' && teardownInputValue() === phrase;
  }

  function updateTeardownConfirmState() {
    const button = root.querySelector('[data-action="teardown-confirm"]');
    if (button) button.disabled = !teardownConfirmSatisfied();
  }

  function offloadDayHasBackupOnly(day) {
    return Boolean(
      day &&
        (day.backup_only_bytes > 0 ||
          day.backup_only_segments > 0 ||
          day.degraded === true),
    );
  }

  function hasBackupOnly(payload) {
    const backupOnly = payload && payload.backup_only;
    if (backupOnly && typeof backupOnly === 'object' && !Array.isArray(backupOnly)) {
      if (
        backupOnly.total_bytes > 0 ||
        backupOnly.total_segments > 0 ||
        backupOnly.total_days > 0 ||
        backupOnly.degraded === true
      ) {
        return true;
      }
    }
    return Array.isArray(payload && payload.days) && payload.days.some(offloadDayHasBackupOnly);
  }

  function backupOnlyTotalsForTeardown() {
    if (offloadState.status !== 'ready') return null;
    const payload = offloadState.payload || {};
    const backupOnly = payload.backup_only;
    if (!backupOnly || typeof backupOnly !== 'object' || Array.isArray(backupOnly)) return null;
    if (backupOnly.degraded !== false) return null;
    const days = backupOnly.total_days;
    const bytes = backupOnly.total_bytes;
    if (typeof days !== 'number' || !Number.isFinite(days) || days < 0) return null;
    if (typeof bytes !== 'number' || !Number.isFinite(bytes) || bytes < 0) return null;
    return { days, size: formatBytes(bytes), bytes };
  }

  function renderTeardownGate(totals) {
    const stakes = root.querySelector('[data-teardown-stakes]');
    if (!stakes) return;
    const restoreFirst = root.querySelector('[data-action="teardown-restore-first"]');
    if (totals === null) {
      stakes.textContent = managementCopy.teardown_gate_unavailable_lead || '';
      if (restoreFirst) restoreFirst.disabled = false;
      return;
    }
    if (totals.days === 0 && totals.bytes === 0) {
      stakes.textContent = managementCopy.teardown_gate_zero_lead || '';
      if (restoreFirst) restoreFirst.disabled = true;
      return;
    }
    stakes.textContent = (managementCopy.teardown_gate_lead || '')
      .replace('{days}', totals.days.toLocaleString())
      .replace('{size}', totals.size);
    if (restoreFirst) restoreFirst.disabled = false;
  }

  function showTeardownGate() {
    const gate = root.querySelector('[data-teardown-gate]');
    if (gate) gate.hidden = false;
    setElementHidden('[data-action="teardown-open"]', true);
    updateTeardownConfirmState();
  }

  function disarmTeardownConfirm() {
    const input = root.querySelector('[data-teardown-input]');
    if (input) input.value = '';
    updateTeardownConfirmState();
  }

  function resetTeardownGate() {
    const gate = root.querySelector('[data-teardown-gate]');
    if (gate) gate.hidden = true;
    setElementHidden('[data-action="teardown-open"]', false);
    disarmTeardownConfirm();
    showMessage('[data-teardown-status]', '');
  }

  // /app/backup/teardown remains unguarded server-side exactly as shipped today;
  // this owner-authenticated local app keeps the gate as a page-level honesty
  // surface and does not change the server contract.
  async function openTeardownGate() {
    try {
      await refreshOffloadStatus();
      const totals = backupOnlyTotalsForTeardown();
      renderTeardownGate(totals);
    } catch (err) {
      if (window.logError) {
        window.logError(err, { context: 'backup teardown offload status failed' });
      }
      renderOffloadUnavailable();
      renderTeardownGate(null);
    }
    showTeardownGate();
  }

  function offloadConfigBody() {
    const budgetField = root.querySelector('[data-offload-budget-input]') || {};
    const floorField = root.querySelector('[data-offload-floor-input]') || {};
    return {
      budget_bytes: gbToBytes(budgetField.value),
      floor_bytes: gbToBytes(floorField.value),
    };
  }

  function offloadLimitState() {
    const budgetField = root.querySelector('[data-offload-budget-input]') || {};
    const floorField = root.querySelector('[data-offload-floor-input]') || {};
    const budgetBytes = gbToBytes(budgetField.value);
    const floorBytes = gbToBytes(floorField.value);
    const budgetPositive = budgetBytes > 0;
    const floorPositive = floorBytes > 0;
    return {
      bothPositive: budgetPositive && floorPositive,
      exactlyOnePositive: budgetPositive !== floorPositive,
    };
  }

  function offloadDaysDegraded(days, payload) {
    return Boolean(
      (payload && payload.backup_only && payload.backup_only.degraded === true) ||
        (Array.isArray(days) && days.some((day) => day && day.degraded === true)),
    );
  }

  function renderOffloadDays(days) {
    const target = root.querySelector('[data-offload-days]');
    if (!target) return;
    const template = root.querySelector('[data-offload-day-template]');
    target.replaceChildren();
    const payload = offloadState.payload || {};
    const filtered = Array.isArray(days)
      ? days
          .filter(offloadDayHasBackupOnly)
          .slice()
          .sort((left, right) => String(right.day || '').localeCompare(String(left.day || '')))
      : [];
    if (filtered.length === 0) {
      if (offloadDaysDegraded(days, payload)) {
        const degraded = document.createElement('p');
        degraded.className = 'backup-warning';
        degraded.textContent = offloadMessages.degraded || '';
        target.append(degraded);
        return;
      }
      const empty = document.createElement('p');
      empty.className = 'backup-note';
      empty.textContent = offloadMessages.empty_days || '';
      target.append(empty);
      return;
    }
    const visible = offloadDaysExpanded ? filtered : filtered.slice(0, MAX_OFFLOAD_DAY_ROWS);
    for (const day of visible) {
      const clone = template.content.cloneNode(true);
      applyCopy(clone, copy);
      const row = clone.querySelector('.backup-offload-day');
      const details = clone.querySelector('[data-offload-day-detail]');
      const heading = clone.querySelector('strong[data-offload-day-value]');
      const displayDay = formatDisplayDay(day.day);
      if (heading) heading.textContent = displayDay;
      const raw = clone.querySelector('[data-offload-day-raw-bytes]');
      if (raw) raw.textContent = formatBytes(day.raw_media_bytes || 0);
      const rawRow = clone.querySelector('[data-offload-day-raw-row]');
      if (rawRow) rawRow.hidden = !(day.raw_media_bytes > 0);
      const backupOnly = clone.querySelector('[data-offload-day-backup-only-bytes]');
      if (backupOnly) backupOnly.textContent = formatBytes(day.backup_only_bytes || 0);

      if (day.degraded) {
        const degraded = document.createElement('p');
        degraded.className = 'backup-warning';
        degraded.textContent = offloadMessages.degraded || '';
        if (details) details.append(degraded);
      }

      const button = clone.querySelector('[data-offload-day-restore]');
      if (button) {
        button.setAttribute('data-offload-day-value', day.day || '');
        button.title = offloadRestoreExpectation(day.backup_only_bytes);
        button.setAttribute(
          'aria-label',
          [offloadCopy.actions && offloadCopy.actions.restore_day, displayDay]
            .filter(Boolean)
            .join(': '),
        );
        button.disabled = !day.backup_only_segments;
      }
      if (row) target.append(row);
    }
    const remaining = filtered.length - visible.length;
    if (remaining > 0) {
      const button = document.createElement('button');
      button.type = 'button';
      button.setAttribute('data-action', 'offload-show-all-days');
      button.textContent = (offloadMessages.show_all_days || '').replace(
        '{count}',
        filtered.length.toLocaleString(),
      );
      target.append(button);
    }
  }

  function renderOffload() {
    const section = root.querySelector('[data-offload-section]');
    if (!section) return;
    section.setAttribute('data-offload-state', offloadState.status);

    const ready = offloadReady();
    const loading = offloadState.status === 'loading';
    const unavailable = offloadState.status === 'unavailable';
    const payload = offloadState.payload || {};
    const offload = payload.offload || {};
    const enabled = ready && offload.enabled === true;
    const hasBackupOnlyMedia = hasBackupOnly(payload);
    const showControls = ready && !unavailable && !loading;
    const showUsage = ready && !unavailable && !loading;
    const showProof = ready && !unavailable && !loading && (enabled || hasBackupOnlyMedia);
    const unavailableElement = root.querySelector('[data-offload-unavailable]');
    if (unavailableElement) unavailableElement.hidden = !unavailable;

    const readiness = root.querySelector('[data-offload-readiness]');
    if (readiness) readiness.hidden = ready || unavailable || loading;
    const form = root.querySelector('[data-offload-enable-form]');
    const summary = root.querySelector('[data-offload-summary]');
    const proof = root.querySelector('[data-offload-proof]');
    const tiering = root.querySelector('.backup-offload-tiering');
    if (form) form.hidden = !showControls;
    if (summary) summary.hidden = !showUsage;
    if (proof) proof.hidden = !showProof;
    if (tiering) tiering.hidden = !showProof;

    const budget = offload.budget_bytes || (payload.suggested_defaults && payload.suggested_defaults.budget_bytes);
    const floor = offload.floor_bytes || (payload.suggested_defaults && payload.suggested_defaults.floor_bytes);
    const budgetField = root.querySelector('[data-offload-budget-input]');
    const floorField = root.querySelector('[data-offload-floor-input]');
    if (budgetField && offloadState.status === 'ready') budgetField.value = formatGbInput(budget);
    if (floorField && offloadState.status === 'ready') floorField.value = formatGbInput(floor);
    for (const field of [budgetField, floorField]) {
      if (field) field.disabled = !showControls;
    }

    const stakes = root.querySelector('[data-offload-stakes]');
    if (stakes) {
      stakes.hidden = !showControls;
      stakes.classList.toggle('backup-offload-stakes--warning', showControls && !enabled);
      stakes.classList.toggle('backup-offload-stakes--note', showControls && enabled);
    }
    setElementHidden('[data-offload-on-chip]', !enabled);
    const enableButton = root.querySelector('[data-action="offload-enable"]');
    if (enableButton) {
      enableButton.hidden = !showControls || enabled;
      enableButton.disabled = !showControls || enabled;
    }
    const saveButton = root.querySelector('[data-action="offload-save"]');
    if (saveButton) {
      saveButton.hidden = !showControls || !enabled;
      saveButton.disabled = !showControls || !enabled;
    }
    const disableButton = root.querySelector('[data-offload-disable]');
    if (disableButton) {
      disableButton.hidden = !enabled;
      disableButton.disabled = !enabled;
    }
    setElementHidden('[data-offload-disable-note]', !enabled);

    setText('[data-offload-raw-bytes]', formatBytes(payload.raw_media && payload.raw_media.total_bytes));
    setText('[data-offload-backup-only-bytes]', formatBytes(payload.backup_only && payload.backup_only.total_bytes));
    setText('[data-offload-device-free]', formatBytes(payload.device && payload.device.free_bytes));
    setText('[data-offload-device-total]', formatBytes(payload.device && payload.device.total_bytes));
    const budgetValid = typeof budget === 'number' && Number.isFinite(budget) && budget > 0;
    const floorValid = typeof floor === 'number' && Number.isFinite(floor) && floor > 0;
    setElementHidden('[data-offload-budget-row]', !budgetValid);
    setElementHidden('[data-offload-floor-row]', !floorValid);
    setText('[data-offload-budget]', budgetValid ? formatBytes(budget) : '');
    setText('[data-offload-floor]', floorValid ? formatBytes(floor) : '');
    setTextWithTitle(
      '[data-offload-last-run]',
      formatWorkingProofDisplay(payload.last_offload),
    );
    setTextWithTitle('[data-offload-last-verify]', formatWorkingProofDisplay(payload.last_verification));
    setTextWithTitle('[data-offload-last-restore]', formatRestoreResultDisplay(payload.last_restore));

    const stalled = payload.last_offload && payload.last_offload.status === 'stalled';
    const stallElement = root.querySelector('[data-offload-stall-reason]');
    if (stallElement) {
      stallElement.hidden = !(ready && !unavailable && stalled);
      if (!stallElement.hidden) {
        stallElement.textContent = [
          offloadCopy.stalled_lead || '',
          offloadStallReasonLabel(payload.last_offload.reason),
        ].filter(Boolean).join(' ');
      }
    }

    renderOffloadDays(showProof ? payload.days : []);
  }

  function renderOperation() {
    const operation = state.operation;
    const banner = root.querySelector('[data-operation-banner]');
    if (!banner) return;
    if (
      !operation ||
      operation.phase === 'needs_subscription' ||
      (hostedRestoreLaneSelected() && operation.kind === 'restore_hosted' && operation.phase === 'refused')
    ) {
      banner.hidden = true;
      return;
    }
    banner.hidden = false;
    setText('[data-operation-phase]', labelForPhase(operation.phase));
    const errorLabel =
      operation.kind === 'offload_restore'
        ? offloadRestoreReasonLabel(operation.reason_code)
        : reasonLabel(operation.reason_code);
    setText('[data-operation-error]', errorLabel);
  }

  function renderStatus() {
    root.setAttribute(
      'data-state',
      operationActive(state.operation) ? state.operation.phase : managedMode() ? 'done' : 'empty',
    );
    const lastBackup = state.last_backup || {};
    const lastVerification = state.last_verification || {};
    const selection = selectStatusCopy(lastBackup.status, lastVerification.status);
    const nodes = ensureBackupStatusNodes();
    if (nodes.backup && selection) {
      const display = statusSelectionDisplay(selection.backup);
      nodes.backup.textContent = display.text;
      if (display.title) {
        nodes.backup.title = display.title;
      } else {
        nodes.backup.removeAttribute('title');
      }
    }
    if (nodes.reason) {
      const reason = lastBackup.status === 'error'
        ? backupErrorReasonLine(lastBackup.error_reason)
        : null;
      nodes.reason.textContent = reason || '';
      nodes.reason.hidden = !reason;
    }
    if (nodes.verification && selection) {
      const display = statusSelectionDisplay(selection.verification);
      nodes.verification.textContent = display.text;
      if (display.title) {
        nodes.verification.title = display.title;
      } else {
        nodes.verification.removeAttribute('title');
      }
    }
    setTextWithTitle('[data-last-prune]', timestampDisplay(state.last_prune && state.last_prune.time));
    const retention = state.retention || {};
    for (const input of root.querySelectorAll('[data-retention-field]')) {
      const key = input.getAttribute('data-retention-field');
      if (key && retention[key] != null) input.value = retention[key];
    }
    renderOperation();
    renderHostedRestoreOperation();
    renderHostedLocation();
    renderOffload();
  }

  function applyPayload(payload) {
    if (!payload) return;
    const next = Object.assign({}, payload);
    delete next.success;
    state = Object.assign({}, state, next);
    renderStatus();
  }

  async function readJson(response) {
    const payload = await response.json();
    if (!response.ok) throw payload;
    return payload;
  }

  async function postJson(path, body) {
    const options = {
      method: 'POST',
      headers: { Accept: 'application/json' },
    };
    if (body) {
      options.headers['Content-Type'] = 'application/json';
      options.body = JSON.stringify(body);
    }
    return readJson(await fetch(path, options));
  }

  async function refreshStatus() {
    const payload = await readJson(
      await fetch('/app/backup/status', { headers: { Accept: 'application/json' } }),
    );
    applyPayload(payload);
    return payload;
  }

  function applyOffloadPayload(payload) {
    if (!validOffloadPayload(payload)) {
      const error = new Error('malformed backup offload status payload');
      if (window.logError) {
        window.logError(error, { context: 'backup offload status payload' });
      } else if (window.console && window.console.error) {
        window.console.error(error);
      }
      renderOffloadUnavailable();
      return false;
    }
    const next = Object.assign({}, payload || {});
    delete next.success;
    delete next.operation;
    offloadState = { status: 'ready', payload: next };
    renderOffload();
    return true;
  }

  function validOffloadPayload(payload) {
    return Boolean(
      payload &&
        typeof payload === 'object' &&
        !Array.isArray(payload) &&
        payload.offload &&
        typeof payload.offload === 'object' &&
        !Array.isArray(payload.offload) &&
        Array.isArray(payload.days),
    );
  }

  async function refreshOffloadStatus() {
    const payload = await readJson(
      await fetch('/app/backup/offload/status', { headers: { Accept: 'application/json' } }),
    );
    applyOffloadPayload(payload);
    return payload;
  }

  function renderOffloadUnavailable() {
    offloadState = { status: 'unavailable', payload: null };
    renderOffload();
  }

  function showMessage(selector, value) {
    const element = root.querySelector(selector);
    if (!element) return;
    element.textContent = value || '';
    element.hidden = !value;
  }

  function showError(selector, err) {
    showMessage(selector, reasonLabel(err && err.reason_code) || (err && err.error) || '');
  }

  function renderRecoveryGrid(display) {
    currentRecoveryDisplay = display || '';
    const grid = root.querySelector('[data-recovery-grid]');
    if (!grid) return;
    grid.replaceChildren();
    for (const group of currentRecoveryDisplay.split(/\s+/).filter(Boolean)) {
      const block = document.createElement('code');
      block.setAttribute('data-recovery-block', '');
      block.textContent = group;
      grid.append(block);
    }
  }

  async function generateRecoveryKey() {
    const payload = await postJson('/app/backup/keys/generate');
    renderRecoveryGrid(payload.recovery_key_display || '');
    return payload;
  }

  async function revealRecoveryKey() {
    const payload = await postJson('/app/backup/recovery-key/reveal');
    renderRecoveryGrid(payload.recovery_key_display || '');
    return payload;
  }

  async function copyRecoveryKey() {
    if (!currentRecoveryDisplay || !navigator.clipboard) return;
    await navigator.clipboard.writeText(currentRecoveryDisplay);
  }

  function syncBackendFields(prefix) {
    const select = root.querySelector(`[data-field="${prefix ? prefix + '_' : ''}backend"]`);
    const value = select ? select.value : 's3';
    const attr = prefix ? 'data-restore-backend-fields' : 'data-backend-fields';
    for (const group of root.querySelectorAll(`[${attr}]`)) {
      group.hidden = group.getAttribute(attr) !== value;
    }
  }

  function formValue(form, name) {
    const field = form.elements[name];
    return field && typeof field.value === 'string' ? field.value.trim() : '';
  }

  function destinationBody(form) {
    const backend = formValue(form, 'backend') || 's3';
    const credentials = {};
    if (backend === 's3') {
      credentials.access_key_id = formValue(form, 'access_key_id');
      credentials.secret_access_key = formValue(form, 'secret_access_key');
    } else {
      credentials.account_id = formValue(form, 'account_id');
      credentials.account_key = formValue(form, 'account_key');
    }
    return {
      repository: formValue(form, 'repository'),
      backend,
      credentials,
    };
  }

  function pollUntilTerminal() {
    if (pollTimer) window.clearTimeout(pollTimer);
    pollTimer = window.setTimeout(async function () {
      try {
        const payload = await refreshStatus();
        if (operationActive(payload.operation)) {
          pollUntilTerminal();
        } else if (payload.operation && payload.operation.kind === 'teardown') {
          resetTeardownGate();
        } else if (payload.operation && payload.operation.kind === 'offload_restore') {
          showMessage('[data-offload-restore-status]', '');
          try {
            await refreshOffloadStatus();
          } catch (err) {
            if (window.logError) {
              window.logError(err, { context: 'backup offload status after restore failed' });
            }
            renderOffloadUnavailable();
          }
        } else if (
          payload.operation &&
          (payload.operation.kind === 'enable_hosted' || payload.operation.kind === 'restore_hosted') &&
          payload.operation.phase === 'done'
        ) {
          showPanel('management');
        } else if (
          payload.operation &&
          payload.operation.kind === 'rotate' &&
          payload.operation.phase === 'done' &&
          payload.recovery_key_confirmed === false
        ) {
          await revealRecoveryKey();
          showPanel('display');
        }
      } catch (_err) {
        const current = state.operation || { kind: 'status' };
        state.operation = Object.assign({}, current, {
          phase: 'error',
          reason_code: 'failed',
          elapsed_ms: 0,
        });
        renderStatus();
      }
    }, 800);
  }

  async function startOperation(path, body) {
    const payload = await postJson(path, body);
    applyPayload(payload);
    if (operationActive(payload.operation)) pollUntilTerminal();
    return payload;
  }

  async function saveDestination(form, targetSelector) {
    const payload = await postJson('/app/backup/destination', destinationBody(form));
    applyPayload(payload);
    const status = payload.destination_status || {};
    showMessage(targetSelector, destinationLabels[status.reason_code] || status.message || '');
    return payload;
  }

  function bindIntro() {
    root.addEventListener('click', async function (event) {
      const button = event.target.closest('[data-action]');
      const action = button && button.getAttribute('data-action');
      if (!button || (button.disabled && action !== 'restore-hosted-unbound-start')) return;
      try {
        if (action === 'restore-hosted-unbound-start') {
          await beginHostedRestoreAttempt();
          return;
        }
        if (action === 'cancel-hosted-restore-attempt') {
          await cancelHostedRestoreAttempt({ showCancelled: true });
          return;
        }
        if (action === 'start') showPanel('educate');
        if (action === 'show-restore') showPanel('restore');
        if (action === 'understand') {
          await generateRecoveryKey();
          showPanel('display');
        }
        if (action === 'continue-confirm') showPanel('confirm');
        if (action === 'see-key-again') {
          await revealRecoveryKey();
          showPanel('display');
        }
        if (action === 'copy-key' || action === 'save-password-manager') {
          await copyRecoveryKey();
        }
        if (action === 'enable-backup') {
          await startOperation('/app/backup/enable');
          showPanel('management');
        }
        if (action === 'enable-hosted') {
          const payload = await startOperation('/app/backup/enable-hosted');
          maybeOpenPortal(payload);
        }
        if (action === 'backup-now') {
          applyPayload(await postJson('/app/backup/backup-now'));
        }
        if (action === 'view-key') {
          await revealRecoveryKey();
          showPanel('display');
        }
        if (action === 'rotate-key') await startOperation('/app/backup/recovery-key/rotate');
        if (action === 'teardown-open') await openTeardownGate();
        if (action === 'teardown-cancel') resetTeardownGate();
        if (action === 'teardown-confirm') {
          if (!teardownConfirmSatisfied()) return;
          disarmTeardownConfirm();
          const payload = await startOperation('/app/backup/teardown');
          if (payload.operation && payload.operation.kind === 'teardown' && !operationActive(payload.operation)) {
            resetTeardownGate();
          }
        }
        if (action === 'teardown-restore-first') {
          const totalBytes =
            offloadState.payload &&
            offloadState.payload.backup_only &&
            offloadState.payload.backup_only.total_bytes;
          showMessage('[data-offload-restore-status]', offloadRestoreExpectation(totalBytes));
          await startOperation('/app/backup/offload/restore', { all: true });
          resetTeardownGate();
        }
        if (action === 'cancel-restore') {
          if (hostedRestoreAttempt.stage === 'terminal' && hostedRestoreAttempt.refusedReason) {
            dismissHostedRestoreRefusal();
          }
          await cancelHostedRestoreAttempt();
          showPanel(managedMode() ? 'management' : 'intro');
        }
        if (action === 'offload-enable') {
          const limits = offloadLimitState();
          if (limits.exactlyOnePositive) {
            showMessage('[data-offload-config-status]', offloadCopy.invalid_limits || '');
            return;
          }
          if (limits.bothPositive) {
            if (!applyOffloadPayload(await postJson('/app/backup/offload/config', offloadConfigBody()))) {
              return;
            }
          }
          if (applyOffloadPayload(await postJson('/app/backup/offload/enable'))) {
            showMessage('[data-offload-config-status]', offloadMessages.saved || '');
          }
        }
        if (action === 'offload-save') {
          if (applyOffloadPayload(await postJson('/app/backup/offload/config', offloadConfigBody()))) {
            showMessage('[data-offload-config-status]', offloadMessages.saved || '');
          }
        }
        if (action === 'offload-disable') {
          if (applyOffloadPayload(await postJson('/app/backup/offload/disable'))) {
            showMessage('[data-offload-config-status]', offloadMessages.saved || '');
          }
        }
        if (action === 'offload-restore-day') {
          const day = button.getAttribute('data-offload-day-value');
          const entry =
            offloadState.payload &&
            Array.isArray(offloadState.payload.days) &&
            offloadState.payload.days.find((candidate) => candidate && candidate.day === day);
          showMessage(
            '[data-offload-restore-status]',
            offloadRestoreExpectation(entry && entry.backup_only_bytes),
          );
          await startOperation('/app/backup/offload/restore', { day });
        }
        if (action === 'offload-show-all-days') {
          offloadDaysExpanded = true;
          renderOffloadDays((offloadState.payload && offloadState.payload.days) || []);
        }
      } catch (err) {
        if (action === 'offload-restore-day' || action === 'teardown-restore-first') {
          showMessage('[data-offload-restore-status]', '');
        }
        if (action && action.startsWith('teardown-')) {
          showError('[data-teardown-status]', err);
        } else if (action && action.startsWith('offload-')) {
          showMessage('[data-offload-config-status]', offloadActionError(err));
        } else {
          showError('[data-operation-error]', err);
        }
      }
    });
  }

  function bindForms() {
    const confirmForm = root.querySelector('[data-confirm-form]');
    if (confirmForm) {
      confirmForm.addEventListener('submit', async function (event) {
        event.preventDefault();
        try {
          const entered = root.querySelector('[data-confirm-input]').value || '';
          const payload = await postJson('/app/backup/confirm', { recovery_key: entered });
          applyPayload(payload);
          showMessage('[data-confirm-error]', '');
          if (state.destination && state.destination.credentials_set) {
            await startOperation('/app/backup/enable');
            showPanel('management');
          } else {
            showPanel('destination');
          }
        } catch (err) {
          showError('[data-confirm-error]', err);
        }
      });
    }

    const destinationForm = root.querySelector('[data-destination-form]');
    if (destinationForm) {
      destinationForm.addEventListener('submit', async function (event) {
        event.preventDefault();
        try {
          await saveDestination(destinationForm, '[data-destination-status]');
        } catch (err) {
          showError('[data-destination-status]', err);
        }
      });
    }

    const retentionForm = root.querySelector('[data-retention-form]');
    if (retentionForm) {
      retentionForm.addEventListener('submit', async function (event) {
        event.preventDefault();
        const body = {};
        for (const input of retentionForm.querySelectorAll('[data-retention-field]')) {
          body[input.getAttribute('data-retention-field')] = input.value;
        }
        try {
          const payload = await postJson('/app/backup/retention', body);
          applyPayload(payload);
          showMessage('[data-retention-status]', phaseLabels.done || '');
        } catch (err) {
          showError('[data-retention-status]', err);
        }
      });
    }

    const restoreForm = root.querySelector('[data-restore-form]');
    if (restoreForm) {
      restoreForm.addEventListener('submit', async function (event) {
        event.preventDefault();
        const body = destinationBody(restoreForm);
        body.recovery_key = restoreForm.elements.recovery_key.value || '';
        try {
          await startOperation('/app/backup/restore', body);
          showMessage('[data-restore-status]', labelForPhase('restoring'));
        } catch (err) {
          showError('[data-restore-status]', err);
        }
      });
    }

    const teardownInput = root.querySelector('[data-teardown-input]');
    if (teardownInput) {
      teardownInput.addEventListener('input', updateTeardownConfirmState);
    }
  }

  function bindBackendSwitching() {
    const destinationBackend = root.querySelector('[data-field="backend"]');
    if (destinationBackend) {
      destinationBackend.addEventListener('change', function () {
        syncBackendFields('');
      });
    }
    const restoreBackend = root.querySelector('[data-field="restore_backend"]');
    if (restoreBackend) {
      restoreBackend.addEventListener('change', function () {
        syncBackendFields('restore');
      });
    }
    syncBackendFields('');
    syncBackendFields('restore');
  }

  function setMode(mode) {
    for (const button of root.querySelectorAll('.backup-mode')) {
      const selected = button.getAttribute('data-mode') === mode;
      button.classList.toggle('is-selected', selected);
      button.setAttribute('aria-checked', selected ? 'true' : 'false');
    }
    for (const item of root.querySelectorAll('[data-mode-panel]')) {
      item.hidden = item.getAttribute('data-mode-panel') !== mode;
    }
  }

  function bindModeSwitching() {
    for (const button of root.querySelectorAll('.backup-mode')) {
      button.addEventListener('click', function () {
        setMode(button.getAttribute('data-mode'));
      });
    }
  }

  function setRestoreLane(lane) {
    if (lane !== 'byo' && lane !== 'operated') return;
    if (
      restoreLane === 'operated' &&
      lane !== 'operated' &&
      hostedRestoreAttempt.stage === 'terminal' &&
      hostedRestoreAttempt.refusedReason
    ) {
      dismissHostedRestoreRefusal();
    }
    if (restoreLane !== lane && hostedRestoreAttemptInFlight()) {
      void cancelHostedRestoreAttempt();
    }
    restoreLane = lane;
    for (const button of root.querySelectorAll('.backup-restore-lane')) {
      const selected = button.getAttribute('data-restore-lane') === lane;
      button.classList.toggle('is-selected', selected);
      button.setAttribute('aria-checked', selected ? 'true' : 'false');
      button.setAttribute('tabindex', selected ? '0' : '-1');
    }
    for (const item of root.querySelectorAll('[data-restore-lane-panel]')) {
      item.hidden = item.getAttribute('data-restore-lane-panel') !== lane;
    }
    renderHostedRestoreOperation();
    renderHostedRestoreAttempt();
  }

  function bindRestoreLaneSwitching() {
    const lanes = Array.from(root.querySelectorAll('.backup-restore-lane'));
    for (const [index, button] of lanes.entries()) {
      button.addEventListener('click', function () {
        setRestoreLane(button.getAttribute('data-restore-lane'));
      });
      button.addEventListener('keydown', function (event) {
        if (!['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown'].includes(event.key)) return;
        event.preventDefault();
        const direction = event.key === 'ArrowLeft' || event.key === 'ArrowUp' ? -1 : 1;
        const next = lanes[(index + direction + lanes.length) % lanes.length];
        setRestoreLane(next.getAttribute('data-restore-lane'));
        next.focus();
      });
    }
    const { field } = hostedRestoreControls();
    if (field) {
      field.addEventListener('input', function () {
        clearHostedRestoreFieldError();
        if (hostedRestoreAttempt.message === (restoreHostedCopy.key_required || '')) {
          setHostedRestoreOutcome('', 'neutral');
        }
        renderHostedRestoreAttempt();
      });
    }
    renderHostedRestoreAttempt();
  }

  function initialPanel() {
    if (operationActive(state.operation)) {
      pollUntilTerminal();
      return managedMode() ? 'management' : 'destination';
    }
    if (managedMode()) return 'management';
    return 'intro';
  }

  async function bind() {
    applyCopy(root, copy);
    renderIntroBullets(root, copy);
    renderRetentionGrid(root, copy);
    bindIntro();
    bindForms();
    bindBackendSwitching();
    bindModeSwitching();
    bindRestoreLaneSwitching();
    try {
      await refreshStatus();
    } catch (err) {
      if (window.logError) {
        window.logError(err, { context: 'backup initial status failed' });
      }
      renderStatus();
    }
    try {
      await refreshOffloadStatus();
    } catch (err) {
      if (window.logError) {
        window.logError(err, { context: 'backup offload initial status failed' });
      }
      renderOffloadUnavailable();
    }
    showPanel(initialPanel());
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', bind);
  } else {
    bind();
  }
})();
