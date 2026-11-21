// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const reportBackupReady = window.createViewReady('backup');

  const BACKUP_COPY = {
    "service_name": window.JournalI18n.render('journal.window.backup.copy.service.name'),
    "brand_lock": window.JournalI18n.renderEnglish('journal.window.backup.copy.brand.lock'),
    "intro": {
      "title": window.JournalI18n.render('journal.window.backup.copy.intro.title'),
      "subtitle": window.JournalI18n.renderEnglish('journal.window.backup.copy.intro.subtitle'),
      "bullets": [
        window.JournalI18n.render('journal.window.backup.copy.intro.bullets.item1'),
        window.JournalI18n.render('journal.window.backup.copy.intro.bullets.item2'),
        window.JournalI18n.render('journal.window.backup.copy.intro.bullets.item3')
      ],
      "optional": window.JournalI18n.render('journal.window.backup.copy.intro.optional'),
      "steps": window.JournalI18n.renderEnglish('journal.window.backup.copy.intro.steps')
    },
    "educate": {
      "stakes": window.JournalI18n.renderEnglish('journal.window.backup.copy.educate.stakes')
    },
    "key": {
      "theft_honesty": window.JournalI18n.renderEnglish('journal.window.backup.copy.key.theft.honesty'),
      "pm_caution": window.JournalI18n.renderEnglish('journal.window.backup.copy.key.pm.caution'),
      "save_key": window.JournalI18n.render('journal.window.backup.copy.key.save.key'),
      "save_failed": window.JournalI18n.render('journal.window.backup.copy.key.save.failed'),
      "copy_failed": window.JournalI18n.render('journal.window.backup.copy.key.copy.failed'),
      "copy_label": window.JournalI18n.render('journal.window.backup.copy.key.copy.label'),
      "continue": window.JournalI18n.render('journal.window.backup.copy.key.continue'),
      "clipboard_caveat": window.JournalI18n.renderEnglish('journal.window.backup.copy.key.clipboard.caveat')
    },
    "confirm": {
      "prompt": window.JournalI18n.render('journal.window.backup.copy.confirm.prompt'),
      "escape": window.JournalI18n.render('journal.window.backup.copy.confirm.escape')
    },
    "destination": {
      "repository_hint": window.JournalI18n.render('journal.window.backup.copy.destination.repository.hint'),
      "object_lock_warning": window.JournalI18n.render('journal.window.backup.copy.destination.object.lock.warning'),
      "object_lock_summary": window.JournalI18n.render('journal.window.backup.copy.destination.object.lock.summary'),
      "field_labels": {
        "repository": window.JournalI18n.render('journal.window.backup.copy.destination.field.labels.repository'),
        "backend": window.JournalI18n.render('journal.window.backup.copy.destination.field.labels.backend'),
        "s3": "S3",
        "b2": "B2",
        "access_key_id": window.JournalI18n.render('journal.window.backup.copy.destination.field.labels.access.key.id'),
        "secret_access_key": window.JournalI18n.render('journal.window.backup.copy.destination.field.labels.secret.access.key'),
        "b2_key_id": window.JournalI18n.render('journal.window.backup.copy.destination.field.labels.b2.key.id'),
        "b2_application_key": window.JournalI18n.render('journal.window.backup.copy.destination.field.labels.b2.application.key')
      },
      "reason_labels": {
        "repo_exists": window.JournalI18n.render('journal.window.backup.copy.destination.reason.labels.repo.exists'),
        "repo_missing": window.JournalI18n.render('journal.window.backup.copy.destination.reason.labels.repo.missing'),
        "auth_failed": window.JournalI18n.render('journal.window.backup.copy.destination.reason.labels.auth.failed'),
        "locked": window.JournalI18n.render('journal.window.backup.copy.destination.reason.labels.locked'),
        "timeout": window.JournalI18n.render('journal.window.backup.copy.destination.reason.labels.timeout'),
        "unreachable": window.JournalI18n.render('journal.window.backup.copy.destination.reason.labels.unreachable'),
        "bucket_not_found": window.JournalI18n.render('journal.window.backup.copy.destination.reason.labels.bucket.not.found')
      },
      "edit": {
        "lead": window.JournalI18n.render('journal.window.backup.copy.destination.edit.lead'),
        "keep_hint": window.JournalI18n.render('journal.window.backup.copy.destination.edit.keep.hint'),
        "partial_credentials": window.JournalI18n.render('journal.window.backup.copy.destination.edit.partial.credentials'),
        "checking": window.JournalI18n.render('journal.window.backup.copy.destination.edit.checking'),
        "no_change": window.JournalI18n.render('journal.window.backup.copy.destination.edit.no.change'),
        "saved_credentials": window.JournalI18n.render('journal.window.backup.copy.destination.edit.saved.credentials'),
        "saved_existing": window.JournalI18n.render('journal.window.backup.copy.destination.edit.saved.existing'),
        "saved_new": window.JournalI18n.render('journal.window.backup.copy.destination.edit.saved.new'),
        "confirm_title": window.JournalI18n.render('journal.window.backup.copy.destination.edit.confirm.title'),
        "confirm_lead": window.JournalI18n.renderEnglish('journal.window.backup.copy.destination.edit.confirm.lead'),
        "confirm_days_unavailable": window.JournalI18n.renderEnglish('journal.window.backup.copy.destination.edit.confirm.days.unavailable'),
        "confirm_action": window.JournalI18n.render('journal.window.backup.copy.destination.edit.confirm.action'),
        "refusals": {
          "auth_failed": window.JournalI18n.render('journal.window.backup.copy.destination.edit.refusals.auth.failed'),
          "bucket_not_found": window.JournalI18n.render('journal.window.backup.copy.destination.edit.refusals.bucket.not.found'),
          "repository_key_refused": window.JournalI18n.render('journal.window.backup.copy.destination.edit.refusals.repository.key.refused'),
          "unreachable": window.JournalI18n.render('journal.window.backup.copy.destination.edit.refusals.unreachable'),
          "timeout": window.JournalI18n.render('journal.window.backup.copy.destination.edit.refusals.timeout'),
          "locked": window.JournalI18n.render('journal.window.backup.copy.destination.edit.refusals.locked'),
          "destination_unchanged": window.JournalI18n.render('journal.window.backup.copy.destination.edit.refusals.destination.unchanged')
        }
      },
      "modes": {
        "byo": {
          "title": window.JournalI18n.render('journal.window.backup.copy.destination.modes.byo.title'),
          "desc": window.JournalI18n.render('journal.window.backup.copy.destination.modes.byo.desc'),
          "note": window.JournalI18n.renderEnglish('journal.window.backup.copy.destination.modes.byo.note')
        },
        "hosted": {
          "title": window.JournalI18n.render('journal.window.backup.copy.destination.modes.hosted.title'),
          "desc": window.JournalI18n.render('journal.window.backup.copy.destination.modes.hosted.desc'),
          "note": window.JournalI18n.renderEnglish('journal.window.backup.copy.destination.modes.hosted.note'),
          "cta": window.JournalI18n.render('journal.window.backup.copy.destination.modes.hosted.cta')
        }
      }
    },
    "hosted": {
      "setup_hint": window.JournalI18n.renderEnglish('journal.window.backup.copy.hosted.setup.hint'),
      "location_label": window.JournalI18n.render('journal.window.backup.copy.hosted.location.label'),
      "location_detail_summary": window.JournalI18n.render('journal.window.backup.copy.hosted.location.detail.summary'),
      "manage_label": window.JournalI18n.render('journal.window.backup.copy.hosted.manage.label'),
      "manage_url": "https://services.solstone.app/services/backup"
    },
    "management": {
      "destructive_action": window.JournalI18n.render('journal.window.backup.copy.management.destructive.action'),
      "destructive_caption": window.JournalI18n.renderEnglish('journal.window.backup.copy.management.destructive.caption'),
      "teardown_gate_unavailable_lead": window.JournalI18n.renderEnglish('journal.window.backup.copy.management.teardown.gate.unavailable.lead'),
      "teardown_gate_zero_lead": window.JournalI18n.renderEnglish('journal.window.backup.copy.management.teardown.gate.zero.lead'),
      "teardown_confirm_phrase": "delete",
      "teardown_confirm_prompt": window.JournalI18n.renderEnglish('journal.window.backup.copy.management.teardown.confirm.prompt'),
      "teardown_restore_first_action": window.JournalI18n.render('journal.window.backup.copy.management.teardown.restore.first.action'),
      "rotate_key_summary": window.JournalI18n.render('journal.window.backup.copy.management.rotate.key.summary'),
      "rotate_key_caption": window.JournalI18n.render('journal.window.backup.copy.management.rotate.key.caption'),
      "retention_hint": window.JournalI18n.render('journal.window.backup.copy.management.retention.hint'),
      "pause": {
        "title": window.JournalI18n.render('journal.window.backup.copy.management.pause.title'),
        "caption": window.JournalI18n.render('journal.window.backup.copy.management.pause.caption')
      },
      "paused": {
        "title": window.JournalI18n.render('journal.window.backup.copy.management.paused.title'),
        "body": window.JournalI18n.render('journal.window.backup.copy.management.paused.body'),
        "hosted_note": window.JournalI18n.render('journal.window.backup.copy.management.paused.hosted.note')
      },
      "status_labels": {
        "last_backup": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.last.backup'),
        "last_successful_backup": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.last.successful.backup'),
        "backup_attempt_unknown": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.backup.attempt.unknown'),
        "backup_history_unknown": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.backup.history.unknown'),
        "backup_history_note": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.backup.history.note'),
        "last_prune": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.last.prune'),
        "last_verification": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.last.verification'),
        "not_available": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.not.available'),
        "not_yet": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.not.yet'),
        "enabled": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.enabled'),
        "disabled": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.disabled'),
        "destination": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.destination'),
        "retention": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.retention'),
        "setup": window.JournalI18n.render('journal.window.backup.copy.management.status.labels.setup')
      },
      "retention_labels": {
        "hourly": window.JournalI18n.render('journal.window.backup.copy.management.retention.labels.hourly'),
        "daily": window.JournalI18n.render('journal.window.backup.copy.management.retention.labels.daily'),
        "weekly": window.JournalI18n.render('journal.window.backup.copy.management.retention.labels.weekly'),
        "monthly": window.JournalI18n.render('journal.window.backup.copy.management.retention.labels.monthly')
      }
    },
    "restore": {
      "expectation": window.JournalI18n.render('journal.window.backup.copy.restore.expectation'),
      "hosted": {
        "choose_lane": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.choose.lane'),
        "byo_desc": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.byo.desc'),
        "operated_desc": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.operated.desc'),
        "lane_title": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.lane.title'),
        "lane_intro": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.lane.intro'),
        "key_label": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.key.label'),
        "key_reassurance": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.key.reassurance'),
        "primary": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.primary'),
        "reopen": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.reopen'),
        "state_b": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.state.b'),
        "state_b_refused": {
          "no_hosted_backup": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.state.b.refused.no.hosted.backup'),
          "hosted_backup_expired": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.state.b.refused.hosted.backup.expired')
        },
        "status_retryable": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.status.retryable'),
        "errors": {
          "auth_failed": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.errors.auth.failed'),
          "cancelled": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.errors.cancelled')
        },
        "popup_preflight_failed": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.popup.preflight.failed'),
        "key_required": window.JournalI18n.render('journal.window.backup.copy.restore.hosted.key.required')
      }
    },
    "status": {
      "last_backup": {
        "never_run": window.JournalI18n.render('journal.window.backup.copy.status.last.backup.never.run'),
      },
    },
    "offload": {
      "title": window.JournalI18n.render('journal.window.backup.copy.offload.title'),
      "unsupported": window.JournalI18n.render('journal.window.backup.copy.offload.unsupported'),
      "stakes": window.JournalI18n.renderEnglish('journal.window.backup.copy.offload.stakes'),
      "stalled_lead": window.JournalI18n.render('journal.window.backup.copy.offload.stalled.lead'),
      "backup_only_label": window.JournalI18n.render('journal.window.backup.copy.offload.backup.only.label'),
      "restore_report": {
        "files": window.JournalI18n.render('journal.window.backup.copy.offload.restore.report.files'),
        "held": window.JournalI18n.render('journal.window.backup.copy.offload.restore.report.held'),
        "dispositions": {
          "published": window.JournalI18n.render('journal.window.backup.copy.offload.restore.report.dispositions.published'),
          "already_present": window.JournalI18n.render('journal.window.backup.copy.offload.restore.report.dispositions.already.present'),
          "conflict": window.JournalI18n.render('journal.window.backup.copy.offload.restore.report.dispositions.conflict'),
          "unverified": window.JournalI18n.render('journal.window.backup.copy.offload.restore.report.dispositions.unverified'),
          "unattempted": window.JournalI18n.render('journal.window.backup.copy.offload.restore.report.dispositions.unattempted')
        }
      },
      "disable_note": window.JournalI18n.render('journal.window.backup.copy.offload.disable.note'),
      "paused_note": window.JournalI18n.render('journal.window.backup.copy.offload.paused.note'),
      "paused_enable_note": window.JournalI18n.render('journal.window.backup.copy.offload.paused.enable.note'),
      "unavailable_lead": window.JournalI18n.render('journal.window.backup.copy.offload.unavailable.lead'),
      "action_error": window.JournalI18n.render('journal.window.backup.copy.offload.action.error'),
      "invalid_limits": window.JournalI18n.render('journal.window.backup.copy.offload.invalid.limits'),
      "enable_hint": window.JournalI18n.render('journal.window.backup.copy.offload.enable.hint'),
      "not_ready": window.JournalI18n.render('journal.window.backup.copy.offload.not.ready'),
      "labels": {
        "gb_suffix": "GiB",
        "budget_gb": window.JournalI18n.render('journal.window.backup.copy.offload.labels.budget.gb'),
        "floor_gb": window.JournalI18n.render('journal.window.backup.copy.offload.labels.floor.gb'),
        "budget_short": window.JournalI18n.render('journal.window.backup.copy.offload.labels.budget.short'),
        "floor_short": window.JournalI18n.render('journal.window.backup.copy.offload.labels.floor.short'),
        "raw_media": window.JournalI18n.render('journal.window.backup.copy.offload.labels.raw.media'),
        "proposed": window.JournalI18n.render('journal.window.backup.copy.offload.labels.proposed'),
        "eligible": window.JournalI18n.render('journal.window.backup.copy.offload.labels.eligible'),
        "projected": window.JournalI18n.render('journal.window.backup.copy.offload.labels.projected'),
        "known": window.JournalI18n.render('journal.window.backup.copy.offload.labels.known'),
        "device_free": window.JournalI18n.render('journal.window.backup.copy.offload.labels.device.free'),
        "device_total": window.JournalI18n.render('journal.window.backup.copy.offload.labels.device.total'),
        "last_offload": window.JournalI18n.render('journal.window.backup.copy.offload.labels.last.offload'),
        "last_verify": window.JournalI18n.render('journal.window.backup.copy.offload.labels.last.verify'),
        "last_restore": window.JournalI18n.render('journal.window.backup.copy.offload.labels.last.restore'),
        "last_completed": window.JournalI18n.render('journal.window.backup.copy.offload.labels.last.completed'),
        "days": window.JournalI18n.render('journal.window.backup.copy.offload.labels.days'),
      },
      "result_labels": {
        "ok": window.JournalI18n.render('journal.window.backup.copy.offload.result.labels.ok'),
        "skipped": window.JournalI18n.render('journal.window.backup.copy.offload.result.labels.skipped'),
        "stalled": window.JournalI18n.render('journal.window.backup.copy.offload.result.labels.stalled'),
        "partial": window.JournalI18n.render('journal.window.backup.copy.offload.result.labels.partial'),
        "error": window.JournalI18n.render('journal.window.backup.copy.offload.result.labels.error'),
        "unknown": window.JournalI18n.render('journal.window.backup.copy.offload.result.labels.unknown')
      },
      "reason_unavailable": window.JournalI18n.render('journal.window.backup.copy.offload.reason.unavailable'),
      "verification_integrity_failed": window.JournalI18n.render('journal.window.backup.copy.offload.verification.integrity.failed'),
      "actions": {
        "prepare": window.JournalI18n.render('journal.window.backup.copy.offload.actions.prepare'),
        "check_preparation": window.JournalI18n.render('journal.window.backup.copy.offload.actions.check.preparation'),
        "review_originals": window.JournalI18n.render('journal.window.backup.copy.offload.actions.review.originals'),
        "enable": window.JournalI18n.render('journal.window.backup.copy.offload.actions.enable'),
        "save": window.JournalI18n.render('journal.window.backup.copy.offload.actions.save'),
        "disable": window.JournalI18n.render('journal.window.backup.copy.offload.actions.disable'),
        "restore_day": window.JournalI18n.render('journal.window.backup.copy.offload.actions.restore.day')
      },
      "messages": {
        "saved": window.JournalI18n.render('journal.window.backup.copy.offload.messages.saved'),
        "empty_days": window.JournalI18n.render('journal.window.backup.copy.offload.messages.empty.days'),
        "incomplete": window.JournalI18n.render('journal.window.backup.copy.offload.messages.incomplete'),
        "degraded": window.JournalI18n.render('journal.window.backup.copy.offload.messages.degraded')
      },
      "prepare_hint": window.JournalI18n.render('journal.window.backup.copy.offload.prepare.hint'),
      "preparation": {
        "checking": window.JournalI18n.render('journal.window.backup.copy.offload.preparation.checking'),
        "pending": window.JournalI18n.render('journal.window.backup.copy.offload.preparation.pending'),
        "finished": window.JournalI18n.render('journal.window.backup.copy.offload.preparation.finished'),
        "failed": window.JournalI18n.render('journal.window.backup.copy.offload.preparation.failed'),
        "unavailable": window.JournalI18n.render('journal.window.backup.copy.offload.preparation.unavailable')
      },
      "stall_reason_labels": {
        "backup_not_ready": window.JournalI18n.render('journal.window.backup.copy.offload.stall.reason.labels.backup.not.ready'),
        "backup_failing": window.JournalI18n.render('journal.window.backup.copy.offload.stall.reason.labels.backup.failing'),
        "verification_missing": window.JournalI18n.render('journal.window.backup.copy.offload.stall.reason.labels.verification.missing'),
        "verification_overdue": window.JournalI18n.render('journal.window.backup.copy.offload.stall.reason.labels.verification.overdue'),
        "verification_failed": window.JournalI18n.render('journal.window.backup.copy.offload.stall.reason.labels.verification.failed'),
        "locked": window.JournalI18n.render('journal.window.backup.copy.offload.stall.reason.labels.locked'),
        "archive_failed": window.JournalI18n.render('journal.window.backup.copy.offload.stall.reason.labels.archive.failed'),
        "confirm_failed": window.JournalI18n.render('journal.window.backup.copy.offload.stall.reason.labels.confirm.failed'),
        "confirm_tool_failed": window.JournalI18n.render('journal.window.backup.copy.offload.stall.reason.labels.confirm.tool.failed'),
        "unexpected_error": window.JournalI18n.render('journal.window.backup.copy.offload.stall.reason.labels.unexpected.error')
      },
      "restore_reason_labels": {
        "archive_unbound": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.archive.unbound'),
        "auth_failed": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.auth.failed'),
        "backup_not_ready": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.backup.not.ready'),
        "failed": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.failed'),
        "insufficient_free_space": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.insufficient.free.space'),
        "ledger_degraded": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.ledger.degraded'),
        "legacy_read_failed": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.legacy.read.failed'),
        "legacy_review_held": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.legacy.review.held'),
        "legacy_verification_failed": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.legacy.verification.failed'),
        "locked": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.locked'),
        "missing_file_after_restore": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.missing.file.after.restore'),
        "nothing_to_restore": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.nothing.to.restore'),
        "repo_missing": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.repo.missing'),
        "restore_conflict": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.restore.conflict'),
        "restore_held": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.restore.held'),
        "restore_not_published": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.restore.not.published'),
        "segment_missing": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.segment.missing'),
        "timeout": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.timeout'),
        "verification_failed": window.JournalI18n.render('journal.window.backup.copy.offload.restore.reason.labels.verification.failed')
      }
    },
    "phase_labels": {
      "cleanup_pending": window.JournalI18n.render('journal.window.backup.copy.phase.labels.cleanup.pending'),
      "setting_up": window.JournalI18n.render('journal.window.backup.copy.phase.labels.setting.up'),
      "restoring": window.JournalI18n.render('journal.window.backup.copy.phase.labels.restoring'),
      "rotating": window.JournalI18n.render('journal.window.backup.copy.phase.labels.rotating'),
      "tearing_down": window.JournalI18n.render('journal.window.backup.copy.phase.labels.tearing.down'),
      "done": window.JournalI18n.render('journal.window.backup.copy.phase.labels.done'),
      "degraded": window.JournalI18n.render('journal.window.backup.copy.phase.labels.degraded'),
      "refused": window.JournalI18n.render('journal.window.backup.copy.phase.labels.refused'),
      "error": window.JournalI18n.render('journal.window.backup.copy.phase.labels.error'),
      "loading": window.JournalI18n.render('journal.window.backup.copy.phase.labels.loading'),
      "empty": window.JournalI18n.render('journal.window.backup.copy.phase.labels.empty')
    },
    "operation_reason_labels": {
      "backup_paused": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.backup.paused'),
      "cleanup_pending": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.cleanup.pending'),
      "backup_busy": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.backup.busy'),
      "backup_not_confirmed": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.backup.not.confirmed'),
      "backup_operation_failed": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.backup.operation.failed'),
      "backup_unavailable": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.backup.unavailable'),
      "invalid_key": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.invalid.key'),
      "invalid_config_value": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.invalid.config.value'),
      "invalid_operation_for_state": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.invalid.operation.for.state'),
      "invalid_request_value": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.invalid.request.value'),
      "repository_unsupported": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.repository.unsupported'),
      "repo_missing": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.repo.missing'),
      "auth_failed": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.auth.failed'),
      "locked": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.locked'),
      "timeout": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.timeout'),
      "failed": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.failed'),
      "incomplete": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.incomplete'),
      "body_rebuild_failed": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.body.rebuild.failed'),
      "integrity_failed": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.integrity.failed'),
      "integrity_unverified": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.integrity.unverified'),
      "missing_required_field": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.missing.required.field'),
      "recovery_key_mismatch": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.recovery.key.mismatch'),
      "expired": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.expired'),
      "restore_prepare_expired": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.restore.prepare.expired'),
      "malformed": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.malformed'),
      "network_error": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.network.error'),
      "broker_unreachable": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.broker.unreachable'),
      "broker_error": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.broker.error'),
      "hosted_entitlement_inactive": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.hosted.entitlement.inactive'),
      "backup_only_held": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.backup.only.held'),
      "destination_unsupported": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.destination.unsupported'),
      "teardown_tool_unavailable": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.teardown.tool.unavailable'),
      "storage_unreached": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.storage.unreached'),
      "backup_only_unplaced": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.backup.only.unplaced'),
      "offload_record_unavailable": window.JournalI18n.render('journal.window.backup.copy.operation.reason.labels.offload.record.unavailable')
    },
    "action_labels": {
      "finish_cleanup": window.JournalI18n.render('journal.window.backup.copy.action.labels.finish.cleanup'),
      "finishing_cleanup": window.JournalI18n.render('journal.window.backup.copy.action.labels.finishing.cleanup'),
      "cleanup_waiting": window.JournalI18n.render('journal.window.backup.copy.action.labels.cleanup.waiting'),
      "cleanup_blocked": window.JournalI18n.render('journal.window.backup.copy.action.labels.cleanup.blocked'),
      "finish_task": window.JournalI18n.render('journal.window.backup.copy.action.labels.finish.task'),
      "finishing_task": window.JournalI18n.render('journal.window.backup.copy.action.labels.finishing.task'),
      "cleanup_checking": window.JournalI18n.render('journal.window.backup.copy.action.labels.cleanup.checking'),
      "cleanup_contended": window.JournalI18n.render('journal.window.backup.copy.action.labels.cleanup.contended'),
      "check_cleanup": window.JournalI18n.render('journal.window.backup.copy.action.labels.check.cleanup'),
      "start": window.JournalI18n.render('journal.window.backup.copy.action.labels.start'),
      "understand": window.JournalI18n.renderEnglish('journal.window.backup.copy.action.labels.understand'),
      "save_destination": window.JournalI18n.render('journal.window.backup.copy.action.labels.save.destination'),
      "change_destination": window.JournalI18n.render('journal.window.backup.copy.action.labels.change.destination'),
      "pause": window.JournalI18n.render('journal.window.backup.copy.action.labels.pause'),
      "resume": window.JournalI18n.render('journal.window.backup.copy.action.labels.resume'),
      "enable": window.JournalI18n.render('journal.window.backup.copy.action.labels.enable'),
      "backup_now": window.JournalI18n.render('journal.window.backup.copy.action.labels.backup.now'),
      "view_key": window.JournalI18n.render('journal.window.backup.copy.action.labels.view.key'),
      "rotate_key": window.JournalI18n.render('journal.window.backup.copy.action.labels.rotate.key'),
      "teardown": window.JournalI18n.render('journal.window.backup.copy.action.labels.teardown'),
      "save_retention": window.JournalI18n.render('journal.window.backup.copy.action.labels.save.retention'),
      "restore": window.JournalI18n.render('journal.window.backup.copy.action.labels.restore'),
      "try_again": window.JournalI18n.render('journal.window.backup.copy.action.labels.try.again'),
      "cancel": window.JournalI18n.render('journal.window.backup.copy.action.labels.cancel')
    },
    "error_intro": window.JournalI18n.render('journal.window.backup.copy.error.intro')
  };
  const STATUS_SELECTION_TABLE = {
    "null|null": { "backup": { "copy_key": "status.last_backup.never_run", "duration_source": null }, "verification": { "copy_key": "management.status_labels.not_yet", "duration_source": null } },
    "null|ok": { "backup": { "copy_key": "status.last_backup.never_run", "duration_source": null }, "verification": { "copy_key": "management.status_labels.not_yet", "duration_source": null } },
    "null|skipped": { "backup": { "copy_key": "status.last_backup.never_run", "duration_source": null }, "verification": { "copy_key": "management.status_labels.not_yet", "duration_source": null } },
    "null|error": { "backup": { "copy_key": "status.last_backup.never_run", "duration_source": null }, "verification": { "copy_key": "management.status_labels.not_yet", "duration_source": null } },
    "ok|null": { "backup": { "message_family": "journal.window.backup.age.elapsed", "duration_source": "last_backup.time" }, "verification": { "copy_key": "management.status_labels.not_yet", "duration_source": null } },
    "ok|ok": { "backup": { "message_family": "journal.window.backup.age.elapsed", "duration_source": "last_backup.time" }, "verification": { "message_family": "journal.window.backup.age.elapsed", "duration_source": "last_verification.time" } },
    "ok|skipped": { "backup": { "message_family": "journal.window.backup.age.elapsed", "duration_source": "last_backup.time" }, "verification": { "message_family": "journal.window.backup.age.check.skipped", "duration_source": "last_verification.time" } },
    "ok|error": { "backup": { "message_family": "journal.window.backup.age.elapsed", "duration_source": "last_backup.time" }, "verification": { "message_family": "journal.window.backup.age.check.failed", "duration_source": "last_verification.time" } },
    "error|null": { "backup": { "message_family": "journal.window.backup.age.backup.failed", "duration_source": "last_backup.time" }, "verification": { "copy_key": "management.status_labels.not_yet", "duration_source": null } },
    "error|ok": { "backup": { "message_family": "journal.window.backup.age.backup.failed", "duration_source": "last_backup.time" }, "verification": { "message_family": "journal.window.backup.age.elapsed", "duration_source": "last_verification.time" } },
    "error|skipped": { "backup": { "message_family": "journal.window.backup.age.backup.failed", "duration_source": "last_backup.time" }, "verification": { "message_family": "journal.window.backup.age.check.skipped", "duration_source": "last_verification.time" } },
    "error|error": { "backup": { "message_family": "journal.window.backup.age.backup.failed", "duration_source": "last_backup.time" }, "verification": { "message_family": "journal.window.backup.age.check.failed", "duration_source": "last_verification.time" } }
  };
  const copy = BACKUP_COPY;

  function backupCount(name, value, display = String(value)) {
    return [[name + '_quantity', window.JournalI18n.cardinalQuantity(value)], [name, display]];
  }
  const BYTES_PER_GB = 1073741824;
  const BYTES_PER_MB = 1048576;
  const MAX_OFFLOAD_DAY_ROWS = 21;
  let state = {};
  let backupStatusCurrent = false;
  let offloadState = { status: 'loading', payload: null };
  const preparationPath = '/api/journal/maintenance/backup:offload';
  let preparation = { loaded: false, request: null, busy: false, error: false };
  let preparationRead = null;
  let currentRecoveryDisplay = '';
  let offloadDaysExpanded = false;
  let displayedRestoreOperation = null;
  let displayedRestoreReport = null;
  let pollTimer = null;
  let statusRead = null;
  let statusReadAgain = false;
  let offloadRead = null;
  let offloadReadAgain = false;
  let initialStatusShown = false;
  let disposed = false;
  const realtimeCleanup = [];
  let cleanupRequestRunning = false;
  const cleanupEligibility = new WeakMap();
  const cleanupMutationActions = new Set([
    'understand', 'enable-backup', 'enable-hosted', 'backup-now', 'rotate-key',
    'teardown-confirm', 'teardown-restore-first', 'restore-hosted-unbound-start',
    'offload-enable', 'offload-save', 'offload-disable', 'offload-restore-day',
    'offload-prepare',
    'pause', 'resume', 'destination-edit', 'switch-confirm', 'switch-restore-first',
  ]);
  let destinationEditing = false;
  let restoreLane = null;
  let hostedRestoreAttempt = {
    stage: 'idle',
    capability: null,
    popup: null,
    message: '',
    tone: 'neutral',
    refusedReason: null,
    dismissedRefusal: false,
    fieldError: false,
    pendingPrepare: null,
    superseded: false,
  };
  let hostedRestoreStartGateOwner = null;

  function releaseHostedRestoreGate(owner) {
    if (hostedRestoreStartGateOwner === owner) {
      hostedRestoreStartGateOwner = null;
    }
  }

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
  const editCopy = (copy.destination && copy.destination.edit) || {};
  const fieldLabels = (copy.destination && copy.destination.field_labels) || {};
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
  const BackupStatus = window.BackupStatus;

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
    return BackupStatus.operationActive(operation);
  }

  function cleanupRequired() {
    return BackupStatus.cleanupRequired(state);
  }

  function conflictsWithCleanup(button) {
    if (cleanupMutationActions.has(button.getAttribute('data-action'))) return true;
    return button.getAttribute('type') === 'submit'
      && Boolean(button.closest('[data-confirm-form], [data-destination-form], [data-retention-form], [data-restore-form]'));
  }

  function setButtonDisabled(button, disabled) {
    if (cleanupRequired() && conflictsWithCleanup(button)) {
      cleanupEligibility.set(button, Boolean(disabled));
      button.disabled = true;
    } else {
      cleanupEligibility.delete(button);
      button.disabled = Boolean(disabled);
    }
  }

  function applyCleanupFence() {
    const pending = cleanupRequired();
    for (const button of root.querySelectorAll('button')) {
      if (!conflictsWithCleanup(button)) continue;
      if (pending) {
        if (!cleanupEligibility.has(button)) cleanupEligibility.set(button, button.disabled);
        button.disabled = true;
      } else if (cleanupEligibility.has(button)) {
        button.disabled = cleanupEligibility.get(button);
        cleanupEligibility.delete(button);
      }
    }
  }

  function shouldPoll() {
    return operationActive(state.operation) || cleanupRequired();
  }

  function managedMode() {
    return BackupStatus.managedMode(state);
  }

  function labelForPhase(phase) {
    return phaseLabels[phase] || phase || '';
  }

  function reasonLabel(reason) {
    return operationLabels[reason] || destinationLabels[reason] || copy.error_intro || '';
  }

  function backupErrorReasonLine(errorReason) {
    const labels = window.BackupStatus.errorReasons;
    if (errorReason === null || errorReason === undefined) {
      return labels._missing || null;
    }
    return Object.prototype.hasOwnProperty.call(labels, errorReason) ? labels[errorReason] : (labels._missing || null);
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
      window.solstoneOutside.openDelayed(operation.portal_url);
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
      reopen: root.querySelector('[data-hosted-restore-reopen]'),
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
    const { field, hint, keyControl, keyReassurance, primary, attemptCancel, reopen, outcome } = hostedRestoreControls();
    if (!field || !primary || !outcome) return;
    const refused = hostedRestoreAttempt.stage === 'terminal' && hostedRestoreAttempt.refusedReason;
    if (hint) hint.hidden = Boolean(refused);
    if (keyControl) keyControl.hidden = Boolean(refused);
    if (keyReassurance) keyReassurance.hidden = Boolean(refused);
    primary.hidden = Boolean(refused);
    setButtonDisabled(primary, Boolean(refused) || hostedRestoreAttemptInFlight() || field.value.trim() === '');
    if (attemptCancel) attemptCancel.hidden = !hostedRestoreAttempt.capability || Boolean(refused);
    if (reopen) {
      const operation = state.operation;
      const eligible = !hostedRestoreAttempt.superseded && operation && operation.kind === 'restore_hosted'
        && !['done', 'error', 'degraded', 'needs_subscription', 'refused'].includes(operation.phase)
        && operation.phase !== 'cleanup_pending'
        && operation.reason_code !== 'expired'
        && operation.reason_code !== 'restore_prepare_expired';
      const validatedUrl = eligible && operation.portal_url ? validHostedRestorePortal(operation.portal_url) : null;
      if (validatedUrl) {
        reopen.hidden = false;
        reopen.href = validatedUrl;
      } else {
        reopen.hidden = true;
        reopen.href = '';
      }
    }
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

  function closeHostedRestorePopup(attempt) {
    const target = attempt || hostedRestoreAttempt;
    const popup = target.popup;
    target.popup = null;
    if (!popup || popup.closed || typeof popup.close !== 'function') return;
    try {
      popup.close();
    } catch (_err) {
      // A cross-origin popup can become uncontrollable after navigation.
    }
  }

  function resetHostedRestoreAttempt() {
    releaseHostedRestoreGate(hostedRestoreAttempt);
    hostedRestoreAttempt.superseded = true;
    if (hostedRestoreAttempt.pendingPrepare) hostedRestoreAttempt.pendingPrepare.cancelled = true;
    closeHostedRestorePopup(hostedRestoreAttempt);
    hostedRestoreAttempt = {
      stage: 'idle',
      capability: null,
      popup: null,
      message: '',
      tone: 'neutral',
      refusedReason: null,
      dismissedRefusal: false,
      fieldError: false,
      pendingPrepare: null,
      superseded: false,
    };
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

  async function cancelStaleCapability(capability, attempt) {
    if (!capability || typeof capability !== 'string') return;
    if (attempt && attempt.cancelPromise) {
      try { await attempt.cancelPromise; } catch (_err) {}
      return;
    }
    try {
      const cancellation = postJson('/app/backup/restore-hosted/cancel', { capability });
      if (attempt) attempt.cancelPromise = cancellation;
      await cancellation;
    } catch (cancelErr) {
      const released = ['restore_prepare_invalid_capability', 'restore_prepare_expired',
        'restore_prepare_generation_changed'].includes(cancelErr && cancelErr.reason_code);
      if (attempt === hostedRestoreAttempt && ((cancelErr && cancelErr.reason_code === 'backup_busy')
        || (Object.hasOwn(state, 'cleanup_admission') && !released))) {
        pollUntilTerminal();
      }
    }
  }

  async function cancelHostedRestoreAttempt(options) {
    const settings = options || {};
    const attempt = hostedRestoreAttempt;
    releaseHostedRestoreGate(attempt);
    attempt.superseded = true;
    closeHostedRestorePopup(attempt);
    renderHostedRestoreAttempt();
    const capability = attempt.capability;
    if (!capability && attempt.stage === 'preparing' && attempt.pendingPrepare) {
      attempt.pendingPrepare.cancelled = true;
    }
    if (capability) {
      try {
        if (!attempt.cancelPromise) {
          attempt.cancelPromise = postJson('/app/backup/restore-hosted/cancel', { capability });
        }
        const payload = await attempt.cancelPromise;
        if (attempt !== hostedRestoreAttempt) return false;
        applyPayload(payload);
      } catch (err) {
        if (attempt !== hostedRestoreAttempt) return false;
        const released = ['restore_prepare_invalid_capability', 'restore_prepare_expired',
          'restore_prepare_generation_changed'].includes(err && err.reason_code);
        if ((err && err.reason_code === 'backup_busy')
          || (Object.hasOwn(state, 'cleanup_admission') && !released)) {
          // A refusal or an unanswered Windows cancellation can still have a
          // running worker. Recover server state before clearing the attempt.
          pollUntilTerminal();
          return false;
        }
        // A resolved or expired lease no longer owns a pending operation.
      }
    }
    resetHostedRestoreAttempt();
    if (settings.showCancelled) {
      setHostedRestoreOutcome((restoreHostedCopy.errors && restoreHostedCopy.errors.cancelled) || '', 'neutral');
    } else if (!settings.preserveOutcome) {
      setHostedRestoreOutcome('', 'neutral');
    }
    renderHostedRestoreAttempt();
    return hostedRestoreAttempt;
  }

  async function failHostedRestoreAttempt(err, options, failedAttempt) {
    const attempt = failedAttempt || hostedRestoreAttempt;
    const isCurrent = attempt === hostedRestoreAttempt && !attempt.superseded;
    closeHostedRestorePopup(attempt);
    const reason = err && err.reason_code;

    if (attempt.capability) {
      if (isCurrent) {
        const cleared = await cancelHostedRestoreAttempt({ preserveOutcome: true });
        if (!cleared || cleared !== hostedRestoreAttempt) return;
      } else {
        await cancelStaleCapability(attempt.capability, attempt);
        return;
      }
    } else if (isCurrent) {
      resetHostedRestoreAttempt();
    } else {
      return;
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
      } else if (operation.reason_code === 'restore_prepare_expired') {
        setHostedRestoreOutcome(operationLabels.restore_prepare_expired || '', 'error');
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
    if (hostedRestoreStartGateOwner) return;

    const attempt = {
      stage: 'idle',
      capability: null,
      popup: null,
      message: '',
      tone: 'neutral',
      refusedReason: null,
      dismissedRefusal: false,
      fieldError: false,
      pendingPrepare: null,
      superseded: false,
    };
    hostedRestoreAttempt = attempt;
    hostedRestoreStartGateOwner = attempt;

    clearHostedRestoreFieldError();
    attempt.dismissedRefusal = false;
    setHostedRestoreOutcome('', 'neutral');

    if (state.hosted && state.hosted.bound === true) {
      attempt.stage = 'polling';
      setHostedRestoreOutcome(restoreHostedCopy.state_b || '', 'active');
      renderHostedRestoreAttempt();
      try {
        const payload = await postJson('/app/backup/restore-hosted', { recovery_key: field.value });
        if (attempt !== hostedRestoreAttempt || attempt.superseded) return;
        applyPayload(payload);
        if (attempt === hostedRestoreAttempt && !attempt.superseded && operationActive(payload.operation)) pollUntilTerminal();
      } catch (err) {
        await failHostedRestoreAttempt(err, null, attempt);
      } finally {
        releaseHostedRestoreGate(attempt);
      }
      return;
    }

    const reservation = window.solstoneOutside.reserveBlank();

    if (!reservation || reservation.kind === 'blocked') {
      releaseHostedRestoreGate(attempt);
      await failHostedRestoreAttempt(null, { popupPreflight: true }, attempt);
      return;
    }

    if (reservation.kind === 'popup') {
      attempt.popup = reservation.popup;
    }
    attempt.stage = 'popup_opened';
    setHostedRestoreOutcome('', 'neutral');
    renderHostedRestoreAttempt();

    let pendingPrepare = null;
    try {
      attempt.stage = 'verifying_popup';
      if (attempt.popup && attempt.popup.closed) {
        await failHostedRestoreAttempt(null, { popupPreflight: true }, attempt);
        return;
      }
      attempt.stage = 'preparing';
      pendingPrepare = { cancelled: false };
      attempt.pendingPrepare = pendingPrepare;
      setHostedRestoreOutcome(restoreHostedCopy.state_b || '', 'active');
      renderHostedRestoreAttempt();

      const prepared = await postJson('/app/backup/restore-hosted/prepare');

      if (attempt !== hostedRestoreAttempt || attempt.superseded) {
        if (prepared && typeof prepared.capability === 'string' && prepared.capability !== '') {
          await cancelStaleCapability(prepared.capability, attempt);
        }
        return;
      }

      if (!prepared || typeof prepared.capability !== 'string' || prepared.capability === '') {
        throw { reason_code: 'restore_prepare_invalid_capability' };
      }
      if (pendingPrepare.cancelled || attempt.stage !== 'preparing') {
        if (attempt.pendingPrepare === pendingPrepare) {
          attempt.pendingPrepare = null;
        }
        await cancelStaleCapability(prepared.capability, attempt);
        return;
      }

      if (attempt.pendingPrepare === pendingPrepare) {
        attempt.pendingPrepare = null;
      }
      attempt.capability = prepared.capability;
      renderHostedRestoreAttempt();

      const keyed = await postJson('/app/backup/restore-hosted/key', {
        capability: attempt.capability,
        recovery_key: field.value,
      });

      if (attempt !== hostedRestoreAttempt || attempt.superseded || attempt.stage !== 'preparing') {
        await cancelStaleCapability(attempt.capability, attempt);
        return;
      }

      const portalUrl = validHostedRestorePortal(keyed && keyed.portal_url);
      if (!portalUrl) {
        throw { reason_code: 'restore_prepare_invalid_portal' };
      }
      attempt.stage = 'key_submitted';
      renderHostedRestoreAttempt();

      await new Promise((resolve) => window.setTimeout(resolve, 0));
      if (attempt !== hostedRestoreAttempt || attempt.superseded || attempt.stage !== 'key_submitted') {
        await cancelStaleCapability(attempt.capability, attempt);
        return;
      }

      attempt.stage = 'navigating';
      setHostedRestoreOutcome(restoreHostedCopy.state_b || '', 'active');
      renderHostedRestoreAttempt();

      const navigated = window.solstoneOutside.navigateReserved(reservation, portalUrl, 'replace');
      if (!navigated) {
        throw { reason_code: 'restore_prepare_navigation_failed' };
      }

      attempt.stage = 'arming';
      renderHostedRestoreAttempt();
      const armed = await postJson('/app/backup/restore-hosted/arm', {
        capability: attempt.capability,
      });

      if (attempt !== hostedRestoreAttempt || attempt.superseded || attempt.stage !== 'arming') {
        await cancelStaleCapability(attempt.capability, attempt);
        return;
      }
      applyPayload(armed);
      if (attempt !== hostedRestoreAttempt || attempt.superseded) return;

      attempt.stage = 'activating';
      renderHostedRestoreAttempt();
      const activated = await postJson('/app/backup/restore-hosted/activate', {
        capability: attempt.capability,
      });

      if (attempt !== hostedRestoreAttempt || attempt.superseded || attempt.stage !== 'activating') {
        await cancelStaleCapability(attempt.capability, attempt);
        return;
      }
      attempt.stage = 'polling';
      setHostedRestoreOutcome(restoreHostedCopy.state_b || '', 'active');
      applyPayload(activated);
      if (attempt === hostedRestoreAttempt && !attempt.superseded) pollUntilTerminal();
    } catch (err) {
      if (attempt.pendingPrepare === pendingPrepare) {
        attempt.pendingPrepare = null;
      }
      if (attempt.stage !== 'idle') await failHostedRestoreAttempt(err, null, attempt);
    } finally {
      releaseHostedRestoreGate(attempt);
    }
  }

  function renderHostedLocation() {
    const section = root.querySelector('[data-hosted-location-section]');
    if (!section) return;
    const hosted = state.hosted || {};
    const operated = state.mode === 'operated' && hosted.bound;
    section.hidden = !operated;
    const detail = root.querySelector('[data-hosted-location-detail]');
    if (!operated) {
      if (detail) detail.hidden = true;
      return;
    }
    setText('[data-hosted-location]', hostedCopy.location_label || '');
    // The heading asks where the backup lives, so the exact storage location is
    // available (G3-214) — as an exact id, it lives inside the disclosure.
    const bucket = typeof hosted.bucket === 'string' ? hosted.bucket.trim() : '';
    const prefix = typeof hosted.prefix === 'string' ? hosted.prefix.trim() : '';
    const exact = [bucket, prefix].filter(Boolean).join('/');
    setText('[data-hosted-location-exact]', exact);
    if (detail) detail.hidden = !exact;
  }

  // A pause is the owner's choice. It keeps every setting and stops only the
  // work that reaches the destination; the server enforces the same flag.
  function backupPaused() {
    return state.enabled === true && Number.isInteger(state.paused_at);
  }

  function renderPaused() {
    const paused = backupPaused();
    setElementHidden('[data-paused-section]', !paused);
    setElementHidden('[data-paused-hosted-note]', !(paused && state.mode === 'operated'));
    setElementHidden('[data-pause-section]', paused || state.enabled !== true);
    setElementHidden('[data-action="backup-now"]', paused);
  }

  // Removing erased things from the backups: while it runs it holds the
  // backup's lock, so the page says a backup waits; while backup is paused,
  // the paused panel says what waits. Read from the trash's own answer.
  const BACKUP_ERASE_COPY = {
    note: window.JournalI18n.render('journal.window.backup.window.explanation.a.backup.waits.until.this.finishes'),
  };
  let backupErase = null;
  let backupEraseReading = false;
  async function readBackupErase() {
    if (backupEraseReading) return;
    backupEraseReading = true;
    try {
      backupErase = await window.apiJson('/app/settings/api/trash/backups');
    } catch (_) {
      backupErase = null;
    } finally {
      backupEraseReading = false;
    }
    renderBackupErase(false);
  }
  function renderBackupErase(read = true) {
    if (read) readBackupErase();
    const C = BACKUP_ERASE_COPY;
    const n = backupErase && Number(backupErase.pending) || 0;
    const banner = root.querySelector('[data-backup-erase-banner]');
    const running = Boolean(backupErase && backupErase.state === 'running' && n > 0);
    if (banner) {
      banner.hidden = !running;
      if (running) {
        setText('[data-backup-erase-phase]', window.JournalI18n.render('journal.window.backup.erasure.running.count', backupCount('things', n)));
        setText('[data-backup-erase-note]', C.note);
      }
    }
    const note = root.querySelector('[data-paused-erase-note]');
    if (note) {
      const show = backupPaused() && n > 0;
      note.hidden = !show;
      if (show) note.textContent = window.JournalI18n.render('journal.window.backup.erasure.paused.count', backupCount('things', n));
    }
  }

  function renderByoLocation() {
    const section = root.querySelector('[data-byo-location-section]');
    if (!section) return;
    const destination = state.destination || {};
    const repository = typeof destination.repository === 'string' ? destination.repository : '';
    const show = state.enabled === true && state.mode === 'byo' && repository !== '';
    section.hidden = !show;
    if (!show) return;
    const backend = destination.backend === 'b2' ? fieldLabels.b2 : fieldLabels.s3;
    setText('[data-byo-location-meta]', window.JournalI18n.render('journal.window.backup.storage.user.backend', [['storage_backend', backend || '']]));
    setText('[data-byo-location-repository]', repository);
  }

  function formatTime(value) {
    if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
      return statusLabels.not_yet || '';
    }
    try {
      if (window.JournalClock?.formatInstant) {
        return window.JournalClock.formatInstant(value * 1000);
      }
      return new Date(value * 1000).toLocaleString(window.JournalI18n.formatLocale);
    } catch (_err) {
      return statusLabels.not_yet || '';
    }
  }

  function validTimestamp(value) {
    return typeof value === 'number' && Number.isFinite(value) && value > 0;
  }

  function timestampRelativeLabel(value, family) {
    const elapsed = Date.now() - value * 1000;
    if (Number.isFinite(elapsed) && elapsed >= 0 && typeof relativeTime === 'function') {
      return relativeTime(elapsed, family);
    }
    return null;
  }

  function timestampDisplay(value) {
    if (!validTimestamp(value)) {
      return { text: statusLabels.not_yet || '', title: '' };
    }
    const title = formatTime(value);
    const text = timestampRelativeLabel(value, 'journal.window.backup.age.elapsed');
    return { text: title && text !== null ? text : title, title };
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
    if (!selection.duration_source) {
      return { text: resolveCopyPath(copy, selection.copy_key) || '', title: '' };
    }
    const value = stateValue(selection.duration_source);
    if (!validTimestamp(value)) return timestampDisplay(value);
    const title = formatTime(value);
    const text = timestampRelativeLabel(value, selection.message_family)
      || window.JournalI18n.render(`${selection.message_family}.clock`, [['recorded_at', title]]);
    return { text, title };
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
    // G3-314: verification reads one slice of the backup data, and the tile
    // that answers 'when' has to keep the scope of what was read. The scope sits
    // under the age rather than inside it, so the tile keeps its siblings' shape.
    let subset = document.querySelector('[data-last-verification-subset]');
    if (verification && !subset) {
      subset = document.createElement('p');
      subset.setAttribute('data-last-verification-subset', '');
      subset.className = 'backup-status-subset';
      subset.hidden = true;
      verification.insertAdjacentElement('afterend', subset);
    }
    return { backup, reason, verification, subset };
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
    // Keep configured bytes exact when an unchanged field is saved or enabled.
    return String(bytes / BYTES_PER_GB);
  }

  function applyOffloadSuggestion(field, hintSelector, suggestedBytes) {
    const suggestion = formatGbInput(suggestedBytes);
    // The input carries the owner's answer and nothing else. A placeholder here
    // reads as a set value at a glance (G3-203); the caption below is the only
    // place the suggestion appears.
    field.removeAttribute('placeholder');
    const hint = root.querySelector(hintSelector);
    if (!hint) return;
    hint.textContent = suggestion
      ? window.JournalI18n.render('journal.window.backup.offload.capacity.suggested', [['suggested_size', formatBytes(suggestedBytes)]])
      : '';
    hint.hidden = !hint.textContent;
  }

  function formatBytes(bytes, { sourceEnglish = false } = {}) {
    const render = sourceEnglish ? window.JournalI18n.renderEnglish : window.JournalI18n.render;
    const locale = sourceEnglish ? undefined : window.JournalI18n.formatLocale;
    if (typeof bytes !== 'number' || !Number.isFinite(bytes) || bytes < 0) {
      return render('journal.window.backup.copy.management.status.labels.not.available');
    }
    if (bytes === 0) return render('journal.window.backup.capacity.gib', [['amount', '0']]);
    if (bytes < BYTES_PER_MB) return render('journal.window.backup.copy.offload.labels.under.one.mib');
    const isGb = bytes >= BYTES_PER_GB;
    const value = bytes / (isGb ? BYTES_PER_GB : BYTES_PER_MB);
    const rounded = value >= 10 ? Math.round(value) : Math.round(value * 10) / 10;
    const id = isGb ? 'journal.window.backup.capacity.gib' : 'journal.window.backup.capacity.mib';
    return render(id, [['amount', rounded.toLocaleString(locale)]]);
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
    const label = typeof reason === 'string' && Object.prototype.hasOwnProperty.call(labels, reason)
      ? labels[reason] : '';
    if (!label) logMissingCopy(`${path}.${reason}`);
    return label || offloadCopy.reason_unavailable || '';
  }

  function offloadStallReasonLabel(reason) {
    if (!reason) return '';
    return Object.prototype.hasOwnProperty.call(offloadStallLabels, reason)
      ? offloadStallLabels[reason] : offloadCopy.reason_unavailable || '';
  }

  function offloadRestoreReasonLabel(reason) {
    return reasonFromOffloadMap(offloadRestoreLabels, 'offload.restore_reason_labels', reason);
  }

  function workingVerificationReasonLabel(reason) {
    if (reason === 'integrity_failed' || reason === 'read_data_mismatch') {
      return offloadCopy.verification_integrity_failed || '';
    }
    const labels = window.BackupStatus.errorReasons;
    return Object.prototype.hasOwnProperty.call(labels, reason)
      ? labels[reason] : offloadCopy.reason_unavailable || '';
  }

  function renderWorkingProof(selector, historySelector, result, reasonLabel) {
    const element = root.querySelector(selector);
    if (!element) return;
    const labels = offloadCopy.result_labels || {};
    const hasResult = Boolean(result && typeof result.status === 'string' && result.status);
    const knownStatus = hasResult && Object.prototype.hasOwnProperty.call(labels, result.status)
      && result.status !== 'unknown';
    const status = hasResult ? (knownStatus ? result.status : 'unknown') : 'missing';
    element.setAttribute('data-result-status', status);
    const display = hasResult ? timestampDisplay(result.time) : { text: statusLabels.not_yet || '', title: '' };
    const reason = hasResult && result.status !== 'ok' && result.reason ? reasonLabel(result.reason) : '';
    setTextWithTitle(selector, {
      text: hasResult ? [labels[status] || labels.unknown || '', display.text, reason].filter(Boolean).join(' · ') : display.text,
      title: display.title,
    });
    const history = root.querySelector(historySelector);
    if (history) {
      const available = Boolean(result && validTimestamp(result.last_ok_time));
      history.hidden = !available;
      setTextWithTitle(`${historySelector} [data-offload-success-time]`, available
        ? timestampDisplay(result.last_ok_time) : { text: '', title: '' });
    }
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
    return window.JournalI18n.render('journal.window.backup.offload.restore.capacity.expectation', [['restore_size', formatBytes(bytes)]]);
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
    if (button) setButtonDisabled(button, !teardownConfirmSatisfied());
  }

  function offloadDayUncertain(day) {
    return Boolean(day && (day.backup_only_complete === false || day.raw_media_complete === false ||
      day.unbound_files > 0 || day.unknown_files > 0 || day.degraded === true));
  }

  function offloadDayHasBackupOnly(day) {
    return Boolean(
      day &&
        (day.backup_only_bytes > 0 ||
          day.backup_only_segments > 0 ||
          offloadDayUncertain(day)),
    );
  }

  function hasBackupOnly(payload) {
    const backupOnly = payload && payload.backup_only;
    if (backupOnly && typeof backupOnly === 'object' && !Array.isArray(backupOnly)) {
      if (
        backupOnly.total_bytes > 0 ||
        backupOnly.total_segments > 0 ||
        backupOnly.total_days > 0 ||
        backupOnly.degraded === true ||
        backupOnly.complete === false ||
        backupOnly.unbound_files > 0 ||
        backupOnly.unknown_files > 0
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
    return { days, bytes };
  }

  function renderTeardownGate(totals) {
    const stakes = root.querySelector('[data-teardown-stakes]');
    if (!stakes) return;
    const restoreFirst = root.querySelector('[data-action="teardown-restore-first"]');
    if (totals === null) {
      stakes.textContent = managementCopy.teardown_gate_unavailable_lead || '';
      if (restoreFirst) setButtonDisabled(restoreFirst, false);
      return;
    }
    if (totals.days === 0 && totals.bytes === 0) {
      stakes.textContent = managementCopy.teardown_gate_zero_lead || '';
      if (restoreFirst) setButtonDisabled(restoreFirst, true);
      return;
    }
    stakes.textContent = window.JournalI18n.renderEnglish('journal.window.backup.consent.teardown.backup.only.days', [
      ...backupCount('days', totals.days, totals.days.toLocaleString()),
      ['backup_size', formatBytes(totals.bytes, { sourceEnglish: true })],
    ]);
    if (restoreFirst) setButtonDisabled(restoreFirst, false);
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

  // These metadata totals explain the confirmation. The server owns teardown
  // admission and fresh checks; this page does not grant deletion authority.
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

  function offloadCoverageUncertain(days, payload) {
    return Boolean(
      (payload && payload.raw_media && payload.raw_media.complete === false) ||
      (payload && payload.backup_only && (payload.backup_only.degraded === true ||
        payload.backup_only.complete === false || payload.backup_only.unbound_files > 0 ||
        payload.backup_only.unknown_files > 0)) ||
        (Array.isArray(days) && days.some(offloadDayUncertain)),
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
      if (offloadCoverageUncertain(days, payload)) {
        const degraded = document.createElement('p');
        degraded.className = 'backup-warning';
        degraded.textContent = offloadMessages.incomplete || '';
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
      if (raw) raw.textContent = formatBytes(day.raw_media_bytes);
      const rawRow = clone.querySelector('[data-offload-day-raw-row]');
      if (rawRow) rawRow.hidden = day.raw_media_bytes === 0;
      const backupOnly = clone.querySelector('[data-offload-day-backup-only-bytes]');
      if (backupOnly) backupOnly.textContent = formatBytes(day.backup_only_bytes);

      if (offloadDayUncertain(day)) {
        const degraded = document.createElement('p');
        degraded.className = 'backup-warning';
        degraded.textContent = offloadMessages.incomplete || '';
        if (details) details.append(degraded);
      }

      const button = clone.querySelector('[data-offload-day-restore]');
      if (button) {
        button.setAttribute('data-offload-day-value', day.day || '');
        button.title = offloadRestoreExpectation(day.backup_only_bytes);
        button.setAttribute(
          'aria-label',
          displayDay
            ? window.JournalI18n.render('journal.window.backup.offload.restore.day.accessible.name', [['day', displayDay]])
            : window.JournalI18n.render('journal.window.backup.copy.offload.actions.restore.day'),
        );
        setButtonDisabled(button, !day.backup_only_segments);
      }
      if (row) target.append(row);
    }
    const remaining = filtered.length - visible.length;
    if (remaining > 0) {
      const button = document.createElement('button');
      button.type = 'button';
      button.setAttribute('data-action', 'offload-show-all-days');
      button.textContent = window.JournalI18n.render('journal.window.backup.offload.days.show.all',
        backupCount('days', filtered.length, filtered.length.toLocaleString(window.JournalI18n.formatLocale)));
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
    const supported = payload.enable_supported !== false;
    const offload = payload.offload || {};
    const enabled = ready && offload.enabled === true;
    const hasBackupOnlyMedia = hasBackupOnly(payload);
    const showControls = supported && ready && !unavailable && !loading;
    const showUsage = ready && !unavailable && !loading;
    const showProof = ready && !unavailable && !loading && (enabled || hasBackupOnlyMedia);
    const unavailableElement = root.querySelector('[data-offload-unavailable]');
    if (unavailableElement) unavailableElement.hidden = !unavailable;
    const unsupportedElement = root.querySelector('[data-offload-unsupported]');
    if (unsupportedElement) unsupportedElement.hidden = supported || unavailable || loading;

    const readiness = root.querySelector('[data-offload-readiness]');
    if (readiness) readiness.hidden = !supported || ready || unavailable || loading;
    const form = root.querySelector('[data-offload-enable-form]');
    const summary = root.querySelector('[data-offload-summary]');
    const proof = root.querySelector('[data-offload-proof]');
    const tiering = root.querySelector('.backup-offload-tiering');
    if (form) form.hidden = !showControls;
    if (summary) summary.hidden = !showUsage;
    if (proof) proof.hidden = !showProof;
    if (tiering) tiering.hidden = !showProof;

    // A suggestion is never shown as if it were the setting: only a configured
    // limit fills the input or captions a tile. The suggestion lives in the hint
    // line alone, and the server still applies it when enable posts no config.
    const suggestedDefaults = payload.suggested_defaults || {};
    const budget = offload.budget_bytes;
    const floor = offload.floor_bytes;
    const budgetField = root.querySelector('[data-offload-budget-input]');
    const floorField = root.querySelector('[data-offload-floor-input]');
    if (budgetField && offloadState.status === 'ready') {
      budgetField.value = formatGbInput(budget);
      applyOffloadSuggestion(budgetField, '[data-offload-budget-hint]', suggestedDefaults.budget_bytes);
    }
    if (floorField && offloadState.status === 'ready') {
      floorField.value = formatGbInput(floor);
      applyOffloadSuggestion(floorField, '[data-offload-floor-hint]', suggestedDefaults.floor_bytes);
    }
    for (const field of [budgetField, floorField]) {
      if (field) field.disabled = !showControls;
    }

    // Paused: offload runs with backup, so it reads as paused too, and turning
    // it on waits for resume.
    const paused = backupPaused();
    const pausedNote = root.querySelector('[data-offload-paused-note]');
    if (pausedNote) {
      pausedNote.textContent = paused
        ? (enabled ? offloadCopy.paused_note : offloadCopy.paused_enable_note) || '' : '';
      pausedNote.hidden = !paused || (!enabled && !showControls);
    }
    if (form && paused && !enabled) form.hidden = true;

    const stakes = root.querySelector('[data-offload-stakes]');
    if (stakes) {
      stakes.hidden = !showControls || (paused && !enabled);
      stakes.classList.toggle('backup-offload-stakes--warning', showControls && !enabled);
      stakes.classList.toggle('backup-offload-stakes--note', showControls && enabled);
    }
    setElementHidden('[data-offload-on-chip]', !enabled);
    const enableButton = root.querySelector('[data-action="offload-enable"]');
    if (enableButton) {
      enableButton.hidden = !showControls || enabled;
      setButtonDisabled(enableButton, !showControls || enabled);
    }
    const saveButton = root.querySelector('[data-action="offload-save"]');
    if (saveButton) {
      saveButton.hidden = !showControls || !enabled;
      setButtonDisabled(saveButton, !showControls || !enabled);
    }
    const disableButton = root.querySelector('[data-offload-disable]');
    if (disableButton) {
      disableButton.hidden = !enabled;
      setButtonDisabled(disableButton, !enabled);
    }
    setElementHidden('[data-offload-disable-note]', !enabled);

    const rawMedia = payload.raw_media || {};
    const backupOnly = payload.backup_only || {};
    const proposed = payload.proposed_release || {};
    setText('[data-offload-raw-bytes]', formatBytes(rawMedia.complete === true ? rawMedia.total_bytes : null));
    setText('[data-offload-backup-only-bytes]', formatBytes(backupOnly.complete === true ? backupOnly.total_bytes : null));
    setText('[data-offload-proposed-bytes]', formatBytes(proposed.structural_bytes));
    setText('[data-offload-eligible-bytes]', formatBytes(proposed.available === true ? proposed.eligible_bytes : null));
    setText('[data-offload-projected-bytes]', formatBytes(proposed.available === true && rawMedia.complete === true ? proposed.projected_local_bytes : null));
    for (const [name, population] of [['raw', rawMedia], ['backup-only', backupOnly]]) {
      const showKnown = population.complete === false && Number.isFinite(population.known_bytes);
      setElementHidden('[data-offload-' + name + '-known-row]', !showKnown);
      setText('[data-offload-' + name + '-known-bytes]', showKnown ? formatBytes(population.known_bytes) : '');
    }
    const uncertain = offloadCoverageUncertain(payload.days, payload);
    setElementHidden('[data-offload-coverage-note]', !uncertain);
    setText('[data-offload-coverage-note]', uncertain ? offloadMessages.incomplete : '');
    setText('[data-offload-device-free]', formatBytes(payload.device && payload.device.free_bytes));
    setText('[data-offload-device-total]', formatBytes(payload.device && payload.device.total_bytes));
    const budgetValid = typeof budget === 'number' && Number.isFinite(budget) && budget > 0;
    const floorValid = typeof floor === 'number' && Number.isFinite(floor) && floor > 0;
    setElementHidden('[data-offload-budget-row]', !budgetValid);
    setElementHidden('[data-offload-floor-row]', !floorValid);
    setText('[data-offload-budget]', budgetValid ? formatBytes(budget) : '');
    setText('[data-offload-floor]', floorValid ? formatBytes(floor) : '');
    renderWorkingProof(
      '[data-offload-last-run]',
      '[data-offload-run-history]', payload.last_offload, offloadStallReasonLabel,
    );
    renderWorkingProof('[data-offload-last-verify]', '[data-offload-verify-history]', payload.last_verification, workingVerificationReasonLabel);
    setTextWithTitle('[data-offload-last-restore]', formatRestoreResultDisplay(payload.last_restore));

    // Historical summaries do not describe the current owning operation.
    const stallElement = root.querySelector('[data-offload-stall-reason]');
    if (stallElement) {
      stallElement.hidden = true;
      stallElement.textContent = '';
    }

    renderOffloadDays(showProof ? payload.days : []);
    renderPreparation();
  }

  function preparationRequest(value) {
    if (!value || typeof value !== 'object' || Array.isArray(value)
        || !/^[A-Za-z0-9_-]{1,64}$/.test(value.request_id || '')
        || typeof value.terminal !== 'boolean'
        || value.terminal && !Number.isInteger(value.exit_code)) {
      throw new Error('preparation receipt unavailable');
    }
    return value;
  }

  function renderPreparation() {
    const supported = offloadState.status === 'ready'
      && offloadState.payload?.prepare_supported === true;
    setElementHidden('[data-offload-prepare]', !supported);
    const request = preparation.request;
    const pending = request && !request.terminal;
    const prepare = root.querySelector('[data-action="offload-prepare"]');
    if (prepare) setButtonDisabled(prepare, !supported || !offloadReady()
      || offloadState.payload?.offload?.enabled !== true || backupPaused()
      || operationActive(state.operation)
      || !preparation.loaded || preparation.busy || preparation.error || pending);
    const check = root.querySelector('[data-action="offload-prepare-check"]');
    if (check) check.disabled = preparation.busy;
    const labels = offloadCopy.preparation;
    const message = preparation.busy ? labels.checking
      : preparation.error ? labels.unavailable
      : pending ? labels.pending
      : request ? request.exit_code === 0 ? labels.finished : labels.failed : '';
    setText('[data-offload-prepare-status]', message);
  }

  function refreshPreparation() {
    if (!workspaceCurrent() || offloadState.payload?.prepare_supported !== true) {
      return Promise.resolve();
    }
    if (preparationRead) return preparationRead;
    preparation.busy = true;
    renderPreparation();
    const id = preparation.request?.request_id;
    const path = preparationPath + '/requests' + (id ? '/' + encodeURIComponent(id) : '');
    preparationRead = (async function () {
      try {
        const payload = await readJson(await fetch(path, { headers: { Accept: 'application/json' } }));
        const value = id ? preparationRequest(payload)
          : payload?.request === null ? null : preparationRequest(payload?.request);
        if (id && value.request_id !== id) throw new Error('preparation identity changed');
        if (!workspaceCurrent()) return;
        preparation.request = value;
        preparation.loaded = true;
        preparation.error = false;
      } catch (_) {
        if (workspaceCurrent()) preparation.error = true;
      } finally {
        preparationRead = null;
        preparation.busy = false;
        if (workspaceCurrent()) renderPreparation();
      }
    })();
    return preparationRead;
  }

  async function prepareCopies() {
    if (!preparation.loaded || preparation.busy || preparation.error
        || preparation.request && !preparation.request.terminal) return;
    // Retain this identity before dispatch. An uncertain reply only offers a
    // read of this receipt; it never automatically sends another mutation.
    if (typeof window.crypto?.getRandomValues !== 'function') {
      preparation.error = true;
      renderPreparation();
      return;
    }
    const bytes = window.crypto.getRandomValues(new Uint8Array(16));
    const id = 'ui-' + Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
    preparation.request = { request_id: id, terminal: false };
    preparation.busy = true;
    renderPreparation();
    try {
      const value = preparationRequest(await postJson(preparationPath, { request_id: id }));
      if (value.request_id !== id) throw new Error('preparation identity changed');
      if (workspaceCurrent()) preparation.request = value;
    } catch (_) {
      if (workspaceCurrent()) preparation.error = true;
    } finally {
      preparation.busy = false;
      if (workspaceCurrent()) renderPreparation();
    }
  }

  function restoreEffectText(value) {
    const validCount = (count) => Number.isSafeInteger(count) && count >= 0;
    const files = validCount(value.files_restored)
      ? value.files_restored.toLocaleString(window.JournalI18n.formatLocale) : statusLabels.not_available || '';
    const size = formatBytes(validCount(value.bytes_restored) ? value.bytes_restored : null);
    return window.JournalI18n.render('journal.window.backup.offload.restore.effects', [
      ...backupCount('files', value.files_restored, files), ['size', size],
    ]);
  }

  function renderRestoreReport(operation) {
    const container = root.querySelector('[data-operation-restore-report]');
    const summary = root.querySelector('[data-operation-restore-summary]');
    const details = root.querySelector('[data-operation-restore-details]');
    const rows = root.querySelector('[data-operation-restore-files]');
    if (!container || !summary || !details || !rows) return;
    const report = operation && operation.kind === 'offload_restore' && operation.report;
    if (!report || typeof report !== 'object' || Array.isArray(report)) {
      container.hidden = true;
      summary.textContent = '';
      rows.replaceChildren();
      details.hidden = true;
      details.removeAttribute('open');
      displayedRestoreOperation = null;
      displayedRestoreReport = null;
      return;
    }
    container.hidden = false;
    if (operation === displayedRestoreOperation && report === displayedRestoreReport) return;
    displayedRestoreOperation = operation;
    displayedRestoreReport = report;
    summary.textContent = restoreEffectText(report);
    rows.replaceChildren();
    details.removeAttribute('open');
    const files = Array.isArray(report.files) ? report.files : [];
    details.hidden = files.length === 0;
    const labels = offloadCopy.restore_report || {};
    const dispositions = labels.dispositions || {};
    const unavailable = (offloadCopy.result_labels || {}).unknown || '';
    for (const file of files) {
      const row = document.createElement('li');
      if (!file || typeof file !== 'object' || Array.isArray(file)) {
        row.setAttribute('data-restore-disposition', 'unknown');
        row.textContent = unavailable;
      } else {
        const known = typeof file.disposition === 'string'
          && Object.prototype.hasOwnProperty.call(dispositions, file.disposition);
        row.setAttribute('data-restore-disposition', known ? file.disposition : 'unknown');
        const location = [file.day, file.stream, file.segment, file.name]
          .filter((part) => typeof part === 'string' && part.length > 0).join('/');
        const held = file.evidence && typeof file.evidence === 'object'
          && !Array.isArray(file.evidence) && file.evidence.hold != null;
        row.textContent = [location || unavailable,
          known ? dispositions[file.disposition] : unavailable,
          restoreEffectText(file), held ? labels.held : ''].filter(Boolean).join(' · ');
      }
      rows.appendChild(row);
    }
  }

  function renderOperation() {
    const operation = state.operation;
    renderRestoreReport(operation);
    const banner = root.querySelector('[data-operation-banner]');
    if (!banner) return;
    if (
      !operation ||
      operation.phase === 'needs_subscription' ||
      operation.phase === 'cleanup_pending' ||
      (hostedRestoreLaneSelected() && operation.kind === 'restore_hosted' && operation.phase === 'refused')
    ) {
      banner.hidden = true;
      return;
    }
    banner.hidden = false;
    setText('[data-operation-phase]', labelForPhase(operation.phase));
    const spinner = banner.querySelector('.backup-spinner');
    if (spinner) spinner.hidden = !operationActive(operation);
    const defaultErrorLabel =
      operation.kind === 'offload_restore'
        ? offloadRestoreReasonLabel(operation.reason_code)
        : reasonLabel(operation.reason_code);
    const hasGuidance = typeof operation.guidance === 'string' && operation.guidance.length > 0;
    const errorLabel = hasGuidance ? operation.guidance : defaultErrorLabel;
    setText('[data-operation-error]', hasGuidance || operation.reason_code ? errorLabel : '');
  }

  function renderCleanup() {
    const banner = root.querySelector('[data-cleanup-banner]');
    const button = root.querySelector('[data-action="finish-cleanup"]');
    if (!banner || !button) return;
    const own = state.operation?.phase === 'cleanup_pending';
    const contended = !own && state.cleanup_admission === 'contended';
    banner.hidden = !cleanupRequired();
    button.hidden = banner.hidden;
    button.disabled = cleanupRequestRunning;
    setText('[data-cleanup-heading]', own ? phaseLabels.cleanup_pending
      : contended ? actionLabels.cleanup_checking : actionLabels.cleanup_waiting);
    setText('[data-cleanup-reason]', own ? reasonLabel('cleanup_pending')
      : contended ? actionLabels.cleanup_contended : actionLabels.cleanup_blocked);
    button.textContent = cleanupRequestRunning
      ? (own ? actionLabels.finishing_cleanup : actionLabels.finishing_task)
      : own ? actionLabels.finish_cleanup
      : contended ? actionLabels.check_cleanup : actionLabels.finish_task;
  }

  function renderStatus() {
    window.BackupStatus.renderSummary(root.querySelector('[data-backup-summary]'), backupStatusCurrent ? state : null);
    root.setAttribute(
      'data-state',
      operationActive(state.operation) ? state.operation.phase : managedMode() ? 'done' : 'empty',
    );
    const lastBackup = state.last_backup || {};
    const lastVerification = state.last_verification || {};
    const selection = selectStatusCopy(lastBackup.status, lastVerification.status);
    const nodes = ensureBackupStatusNodes();
    if (nodes.backup) {
      const display = selection ? statusSelectionDisplay(selection.backup)
        : { text: statusLabels.backup_attempt_unknown || '', title: '' };
      nodes.backup.textContent = display.text;
      if (display.title) {
        nodes.backup.title = display.title;
      } else {
        nodes.backup.removeAttribute('title');
      }
    }
    if (nodes.reason) {
      // A backup that stored a copy but couldn't read everything (restic's exit 3)
      // is recorded as ok with the reason incomplete.
      const reason = lastBackup.status === 'error'
          || (lastBackup.status === 'ok' && lastBackup.error_reason === 'incomplete')
        ? backupErrorReasonLine(lastBackup.error_reason)
        : null;
      nodes.reason.textContent = reason || '';
      nodes.reason.hidden = !reason;
    }
    // An absent history field can still have an intact successful attempt.
    // Explicit null means success history is unknown, not "never backed up".
    const success = Object.hasOwn(lastBackup, 'last_success')
      ? lastBackup.last_success : lastBackup.status === 'ok' ? lastBackup : null;
    const knownSuccess = success?.status === 'ok' && validTimestamp(success.time)
      && Number.isFinite(new Date(success.time * 1000).getTime());
    setTextWithTitle('[data-last-successful-backup]', knownSuccess
      ? timestampDisplay(success.time)
      : { text: statusLabels.backup_history_unknown || '', title: '' });
    const successReason = root.querySelector('[data-last-successful-backup-reason]');
    if (successReason) {
      const reason = success?.status === 'ok' && success.error_reason === 'incomplete'
        ? backupErrorReasonLine(success.error_reason) : '';
      successReason.textContent = reason || '';
      successReason.hidden = !reason;
    }
    const historyNote = root.querySelector('[data-backup-history-note]');
    if (historyNote) historyNote.hidden = success?.status !== 'ok';
    if (nodes.verification && selection) {
      const display = statusSelectionDisplay(selection.verification);
      nodes.verification.textContent = display.text;
      if (display.title) {
        nodes.verification.title = display.title;
      } else {
        nodes.verification.removeAttribute('title');
      }
    }
    if (nodes.subset) {
      const line = window.BackupStatus.verificationSubsetLine(lastVerification.checked_subset);
      nodes.subset.textContent = line;
      nodes.subset.hidden = !line;
    }
    setTextWithTitle('[data-last-prune]', timestampDisplay(state.last_prune && state.last_prune.time));
    const retention = state.retention || {};
    for (const input of root.querySelectorAll('[data-retention-field]')) {
      const key = input.getAttribute('data-retention-field');
      if (key && retention[key] != null) input.value = retention[key];
    }
    renderOperation();
    renderCleanup();
    renderHostedRestoreOperation();
    renderHostedLocation();
    renderByoLocation();
    renderPaused();
    renderBackupErase();
    renderOffload();
    applyCleanupFence();
    const heading = root.querySelector('h1[data-copy="intro.title"]') || document.querySelector('h1');
    if (heading && window.JournalClock?.placeNote) {
      window.JournalClock.placeNote(heading, { inside: false });
    }
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

  function workspaceCurrent() {
    return !disposed && root.isConnected !== false;
  }

  function refreshStatus() {
    if (!workspaceCurrent()) return Promise.resolve(null);
    if (statusRead) {
      statusReadAgain = true;
      return statusRead;
    }
    statusRead = (async function () {
      let payload = null;
      let failure = null;
      do {
        statusReadAgain = false;
        try {
          payload = await readJson(
            await fetch('/app/backup/status', { headers: { Accept: 'application/json' } }),
          );
          if (!workspaceCurrent()) return null;
          if (payload?.success !== true) throw new Error('backup status unavailable');
          backupStatusCurrent = Boolean(window.BackupStatus.select(payload).snapshot);
          applyPayload(payload);
          failure = null;
        } catch (error) {
          if (!workspaceCurrent()) return null;
          backupStatusCurrent = false;
          window.BackupStatus.renderSummary(root.querySelector('[data-backup-summary]'), null);
          failure = error;
        }
      } while (statusReadAgain && workspaceCurrent());
      if (failure) throw failure;
      return payload;
    })().finally(function () { statusRead = null; });
    return statusRead;
  }

  function revealInitialStatus() {
    if (initialStatusShown || !workspaceCurrent()) return;
    initialStatusShown = true;
    root.querySelector('[data-backup-loading]').hidden = true;
    showPanel(initialPanel());
    refreshOffloadStatus().catch(function (error) {
      if (window.logError) window.logError(error, { context: 'backup offload initial status failed' });
      if (workspaceCurrent()) renderOffloadUnavailable();
    });
  }

  function refreshChangedStatus() {
    return refreshStatus().then(function (payload) {
      if (!payload || !workspaceCurrent()) return;
      revealInitialStatus();
      if (shouldPoll()) pollUntilTerminal();
    }).catch(function (error) {
      if (window.logError) window.logError(error, { context: 'backup result refresh failed' });
    });
  }

  function refreshChangedOffloadStatus() {
    return refreshOffloadStatus().catch(function (error) {
      if (window.logError) window.logError(error, { context: 'backup offload result refresh failed' });
      if (workspaceCurrent()) renderOffloadUnavailable();
    });
  }

  function disposeRealtime() {
    if (disposed) return;
    disposed = true;
    for (const cleanup of realtimeCleanup) cleanup();
    if (pollTimer) window.clearTimeout(pollTimer);
    document.removeEventListener?.('workspace:mounted', workspaceMounted);
  }

  function workspaceMounted() {
    if (!workspaceCurrent()) disposeRealtime();
  }

  function wireRealtime() {
    if (!window.appEvents) return;
    realtimeCleanup.push(window.appEvents.listen('supervisor', function (message) {
      if (!workspaceCurrent()) { disposeRealtime(); return; }
      if (message.event !== 'stopped') return;
      const routines = ['backup:run', 'backup:verify', 'backup:prune', 'backup:offload'];
      const relevant = message.service === 'backup' && routines.includes(message.routine)
        || routines.some(function (routine) { return message.service === 'maintenance:' + routine; });
      if (relevant) refreshChangedStatus();
      const offloadFinished = message.service === 'backup' && message.routine === 'backup:offload'
        || message.service === 'maintenance:backup:offload';
      if (offloadFinished) refreshChangedOffloadStatus();
    }));
    let reconnectNeeded = false;
    if (window.appEvents.onConnectionState) {
      realtimeCleanup.push(window.appEvents.onConnectionState(function (connection) {
        if (!workspaceCurrent()) { disposeRealtime(); return; }
        if (!connection.connected) reconnectNeeded = true;
        else if (reconnectNeeded) {
          reconnectNeeded = false;
          refreshChangedStatus();
          refreshChangedOffloadStatus();
        }
      }));
    }
    document.addEventListener('workspace:mounted', workspaceMounted);
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
    applyCleanupFence();
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

  function refreshOffloadStatus() {
    if (!workspaceCurrent()) return Promise.resolve(null);
    if (offloadRead) {
      offloadReadAgain = true;
      return offloadRead;
    }
    offloadRead = Promise.resolve().then(async function () {
      try {
        let payload = null;
        let failure = null;
        do {
          offloadReadAgain = false;
          try {
            payload = await readJson(
              await fetch('/app/backup/offload/status', { headers: { Accept: 'application/json' } }),
            );
            if (!workspaceCurrent()) return null;
            applyOffloadPayload(payload);
            if (payload.prepare_supported === true) await refreshPreparation();
            failure = null;
          } catch (error) {
            if (!workspaceCurrent()) return null;
            failure = error;
          }
        } while (offloadReadAgain && workspaceCurrent());
        if (failure) throw failure;
        return payload;
      } finally {
        offloadRead = null;
      }
    });
    return offloadRead;
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
    if (!currentRecoveryDisplay) return;
    try {
      if (!navigator.clipboard) throw new Error('clipboard unavailable');
      await navigator.clipboard.writeText(currentRecoveryDisplay);
    } catch (_) {
      throw { error: copy.key.copy_failed };
    }
  }

  async function saveRecoveryKey() {
    if (!currentRecoveryDisplay) return;
    const file = new File([currentRecoveryDisplay + '\n'], 'journal-recovery-key.txt', { type: 'text/plain' });
    if (navigator.canShare && navigator.canShare({ files: [file] })) {
      try {
        await navigator.share({ files: [file] });
      } catch (error) {
        if (error.name !== 'AbortError') throw { error: copy.key.save_failed };
      }
      return;
    }
    const url = URL.createObjectURL(file);
    const link = document.createElement('a');
    link.href = url;
    link.download = file.name;
    document.body.appendChild(link);
    link.click();
    link.remove();
    // Let the browser accept the download before retiring its source.
    setTimeout(() => URL.revokeObjectURL(url), 1000);
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
    if (!workspaceCurrent()) return;
    if (pollTimer) window.clearTimeout(pollTimer);
    pollTimer = window.setTimeout(async function () {
      try {
        const payload = await refreshStatus();
        if (!payload || !workspaceCurrent()) return;
        if (shouldPoll()) {
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
        if (!workspaceCurrent()) return;
        const current = state.operation || { kind: 'status' };
        if (cleanupRequired() || (Object.hasOwn(state, 'cleanup_admission') && operationActive(current))) {
          renderStatus();
          pollUntilTerminal();
          return;
        }
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

  function destinationMessage(text, tone) {
    const element = root.querySelector('[data-destination-status]');
    if (!element) return;
    element.textContent = text || '';
    element.hidden = !text;
    if (tone) element.setAttribute('data-tone', tone);
    else element.removeAttribute('data-tone');
  }

  function syncKeepHint() {
    const form = root.querySelector('[data-destination-form]');
    const current = (state.destination || {}).backend;
    setElementHidden('[data-destination-keep-hint]',
      !(destinationEditing && form && formValue(form, 'backend') === current));
  }

  function hideSwitchGate() {
    setElementHidden('[data-switch-gate]', true);
    setElementHidden('[data-destination-actions]', false);
  }

  function setDestinationEditMode(editing) {
    destinationEditing = editing;
    setElementHidden('[data-destination-setup-heading]', editing);
    setElementHidden('[data-destination-edit-heading]', !editing);
    setElementHidden('[data-destination-edit-lead]', !editing);
    setElementHidden('.backup-modes', editing);
    setElementHidden('[data-action="enable-backup"]', editing);
    setElementHidden('[data-action="destination-edit-cancel"]', !editing);
    root.querySelector('[data-destination-save]')?.classList.toggle('primary', editing);
    hideSwitchGate();
    syncKeepHint();
  }

  // Secrets never come back to the page, so the credential fields start empty;
  // empty with the same backend keeps the current ones.
  function openDestinationEdit() {
    const form = root.querySelector('[data-destination-form]');
    if (!form) return;
    const destination = state.destination || {};
    form.elements.repository.value = destination.repository || '';
    form.elements.backend.value = destination.backend === 'b2' ? 'b2' : 's3';
    for (const name of ['access_key_id', 'secret_access_key', 'account_id', 'account_key']) {
      if (form.elements[name]) form.elements[name].value = '';
    }
    setMode('byo');
    syncBackendFields('');
    setDestinationEditMode(true);
    destinationMessage('', null);
    showMessage('[data-byo-location-status]', '');
    showPanel('destination');
  }

  function closeDestinationEdit(savedMessage) {
    setDestinationEditMode(false);
    destinationMessage('', null);
    showPanel('management');
    showMessage('[data-byo-location-status]', savedMessage || '');
  }

  function editRefusal(err) {
    const code = err && err.reason_code;
    return (editCopy.refusals || {})[code] || reasonLabel(code) || (err && err.error) || '';
  }

  function credentialsFilled(body) {
    const values = Object.values(body.credentials);
    const filled = values.filter(Boolean).length;
    return filled === 0 ? 'none' : filled === values.length ? 'all' : 'some';
  }

  // These totals explain the confirmation; the server still asks for it.
  async function openSwitchGate() {
    try {
      await refreshOffloadStatus();
    } catch (err) {
      if (window.logError) window.logError(err, { context: 'backup switch offload status failed' });
      renderOffloadUnavailable();
    }
    const totals = backupOnlyTotalsForTeardown();
    const kept = Boolean(totals && (totals.days > 0 || totals.bytes > 0));
    let line = '';
    if (totals === null) line = editCopy.confirm_days_unavailable || '';
    else if (kept) {
      line = window.JournalI18n.renderEnglish('journal.window.backup.consent.destination.backup.only.days', [
        ...backupCount('days', totals.days, totals.days.toLocaleString()),
        ['backup_size', formatBytes(totals.bytes, { sourceEnglish: true })],
      ]);
    }
    showMessage('[data-switch-days]', line);
    setElementHidden('[data-action="switch-restore-first"]', !kept);
    destinationMessage('', null);
    setElementHidden('[data-destination-actions]', true);
    setElementHidden('[data-switch-gate]', false);
  }

  async function submitDestinationEdit(form, confirmed) {
    const body = destinationBody(form);
    if (credentialsFilled(body) === 'some') {
      destinationMessage(editCopy.partial_credentials, 'refused');
      return;
    }
    const current = state.destination || {};
    if (!confirmed && (body.repository !== current.repository || body.backend !== current.backend)) {
      await openSwitchGate();
      return;
    }
    if (confirmed) body.confirm_new_repository = true;
    hideSwitchGate();
    destinationMessage(editCopy.checking, null);
    const save = root.querySelector('[data-destination-save]');
    if (save) save.disabled = true;
    try {
      const payload = await postJson('/app/backup/destination', body);
      applyPayload(payload);
      const status = payload.destination_status || {};
      closeDestinationEdit(status.change === 'credentials' ? editCopy.saved_credentials
        : status.reason_code === 'repo_missing' ? editCopy.saved_new : editCopy.saved_existing);
    } catch (err) {
      destinationMessage(editRefusal(err), 'refused');
    } finally {
      if (save) save.disabled = false;
    }
  }

  function bindIntro() {
    root.addEventListener('click', async function (event) {
      const button = event.target.closest('[data-action]');
      const action = button && button.getAttribute('data-action');
      if (!button || (button.disabled && action !== 'restore-hosted-unbound-start')) return;
      if (cleanupRequired() && cleanupMutationActions.has(action)) {
        applyCleanupFence();
        return;
      }
      try {
        if (action === 'offload-prepare') {
          await prepareCopies();
          return;
        }
        if (action === 'offload-prepare-check') {
          await refreshStatus();
          await refreshPreparation();
          await refreshChangedOffloadStatus();
          return;
        }
        if (action === 'finish-cleanup') {
          if (cleanupRequestRunning || !cleanupRequired()) return;
          cleanupRequestRunning = true;
          renderCleanup();
          try {
            applyPayload(await postJson('/app/backup/api/cleanup/retry'));
          } finally {
            cleanupRequestRunning = false;
            renderCleanup();
            if (shouldPoll()) pollUntilTerminal();
          }
          return;
        }
        if (action === 'restore-hosted-unbound-start') {
          await beginHostedRestoreAttempt();
          return;
        }
        if (action === 'cancel-hosted-restore-attempt') {
          await cancelHostedRestoreAttempt({ showCancelled: true });
          return;
        }
        if (action === 'destination-edit') openDestinationEdit();
        if (action === 'destination-edit-cancel') closeDestinationEdit('');
        if (action === 'switch-cancel') hideSwitchGate();
        if (action === 'switch-confirm') {
          await submitDestinationEdit(root.querySelector('[data-destination-form]'), true);
        }
        if (action === 'pause') applyPayload(await postJson('/app/backup/pause'));
        if (action === 'resume') applyPayload(await postJson('/app/backup/resume'));
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
        if (action === 'copy-key') {
          showMessage('[data-recovery-error]', '');
          await copyRecoveryKey();
        }
        if (action === 'save-key') {
          showMessage('[data-recovery-error]', '');
          await saveRecoveryKey();
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
          // Acceptance returns an earlier status snapshot, not this run's result.
          await postJson('/app/backup/backup-now');
          await refreshChangedStatus();
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
        if (action === 'switch-restore-first') {
          closeDestinationEdit('');
          const totalBytes = offloadState.payload?.backup_only?.total_bytes;
          showMessage('[data-offload-restore-status]', offloadRestoreExpectation(totalBytes));
          await startOperation('/app/backup/offload/restore', { all: true });
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
          if (!await cancelHostedRestoreAttempt()) return;
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
        if (['offload-restore-day', 'teardown-restore-first', 'switch-restore-first'].includes(action)) {
          showMessage('[data-offload-restore-status]', '');
        }
        if (['copy-key', 'save-key'].includes(action)) {
          showError('[data-recovery-error]', err);
        } else if (action && action.startsWith('teardown-')) {
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
        if (cleanupRequired()) return;
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
        if (cleanupRequired()) return;
        if (destinationEditing) {
          await submitDestinationEdit(destinationForm, false);
          return;
        }
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
        if (cleanupRequired()) return;
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
        if (cleanupRequired()) return;
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
        syncKeepHint();
        // A phone's late blur as the menu closes goes back to the select.
        window.SolFocus.keepAfterPick(destinationBackend);
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
    if (shouldPoll()) pollUntilTerminal();
    return BackupStatus.initialPanel(state);
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
    wireRealtime();
    try {
      await refreshStatus();
    } catch (err) {
      if (window.logError) {
        window.logError(err, { context: 'backup initial status failed' });
      }
      if (workspaceCurrent()) {
        const loading = root.querySelector('[data-backup-loading]');
        loading.textContent = window.JournalI18n.render('journal.window.backup.visible.text.couldn.t.load.backup.settings.reload');
        reportBackupReady('error');
      }
      return;
    }
    revealInitialStatus();
    reportBackupReady('content');
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', bind);
  } else {
    bind();
  }
})();
