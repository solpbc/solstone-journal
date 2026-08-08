# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from dataclasses import dataclass


@dataclass(frozen=True)
class Reason:
    code: str
    message: str
    status: int = 400


# request boundary
HTTP_ERROR = Reason(
    "http_error",
    "I couldn't complete that request.",
    400,
)
INTERNAL_ERROR = Reason(
    "internal_error",
    "I couldn't complete that request.",
    500,
)

# auth
AUTH_REQUIRED = Reason("auth_required", "I couldn't verify this request.", 401)
AUTH_KEY_INVALID = Reason("auth_key_invalid", "I couldn't verify that key.", 401)
HOST_NOT_ALLOWED = Reason("host_not_allowed", "I couldn't verify this request.", 403)
CROSS_ORIGIN_BLOCKED = Reason(
    "cross_origin_blocked", "I couldn't verify this request.", 403
)
PL_REVOKED = Reason(
    "pl_revoked",
    "I couldn't use that paired device because it was revoked.",
    403,
)
PAIRED_DEVICE_NOT_FOUND = Reason(
    "paired_device_not_found",
    "I couldn't find that paired device.",
    404,
)

# pairing
PAIRING_REQUEST_INVALID = Reason(
    "pairing_request_invalid",
    "I couldn't use that pairing request.",
    400,
)
PAIRING_KEY_INVALID = Reason(
    "pairing_key_invalid",
    "I couldn't use that pairing key.",
    400,
)
PAIRING_RELAY_UNAVAILABLE = Reason(
    "pairing_relay_unavailable",
    "I couldn't open the pairing window with the relay right now. Try again in a moment.",
    503,
)

# input validation
INVALID_JSON_REQUEST = Reason(
    "invalid_json_request",
    "I couldn't read that JSON request.",
    400,
)
MISSING_REQUEST_BODY = Reason(
    "missing_request_body",
    "I couldn't find any data in that request.",
    400,
)
MISSING_REQUIRED_FIELD = Reason(
    "missing_required_field",
    "I couldn't find a required field.",
    400,
)
INVALID_REQUEST_VALUE = Reason(
    "invalid_request_value",
    "I couldn't use one of those values.",
    400,
)
INVALID_OPERATION_FOR_STATE = Reason(
    "invalid_operation_for_state",
    "I couldn't take that action in the current state.",
    400,
)
INVALID_DAY = Reason("invalid_day", "I couldn't use that day.", 400)
INVALID_MONTH = Reason("invalid_month", "I couldn't use that month.", 400)
TIMELINE_MONTH_NOT_FOUND = Reason(
    "timeline_month_not_found", "I couldn't find that timeline month.", 404
)
INVALID_PATH = Reason("invalid_path", "I couldn't use that path.", 400)
INVALID_SEGMENT_OR_STREAM = Reason(
    "invalid_segment_or_stream",
    "I couldn't use that segment or stream.",
    400,
)

# file/journal
FILE_NOT_FOUND = Reason("file_not_found", "I couldn't find that file.", 404)
FILE_READ_FAILED = Reason("file_read_failed", "I couldn't read that file.", 500)
RAW_MEDIA_NOT_AVAILABLE = Reason(
    "raw_media_not_available",
    "I couldn't run analysis because the raw media is no longer available.",
    400,
)
OPERATION_NO_LONGER_AVAILABLE = Reason(
    "operation_no_longer_available",
    "I couldn't finish because that action is no longer available.",
    410,
)

# config/settings
CORRUPT_CONFIG = Reason(
    "corrupt_config",
    "I couldn't read your settings.",
    500,
)
IDENTITY_NOT_LOCKED = Reason(
    "identity_not_locked",
    "I couldn't finish setup because the journal id is not locked.",
    400,
)
INVALID_CONFIG_VALUE = Reason(
    "invalid_config_value",
    "I couldn't save that setting because one value was invalid.",
    400,
)
SETTINGS_OPERATION_FAILED = Reason(
    "settings_operation_failed",
    "I couldn't save those settings.",
    500,
)
CONFIG_BUSY = Reason(
    "config_busy",
    "I couldn't save those settings right now because they were busy. Try again in a moment.",
    503,
)
CONVEY_OPERATION_FAILED = Reason(
    "convey_operation_failed",
    "I couldn't update the interface settings.",
    500,
)
CONVEY_BUSY = Reason(
    "convey_busy",
    "I couldn't update the interface settings right now because they were busy. Try again in a moment.",
    503,
)
SERVICE_BUSY = Reason(
    "service_busy",
    "The service operation is already running. Try again in a moment.",
    503,
)
UNKNOWN_SERVICE = Reason(
    "unknown_service",
    "I couldn't find that service.",
    404,
)
SERVICE_OPERATION_FAILED = Reason(
    "service_operation_failed",
    "The service operation could not be completed.",
    500,
)

# backup
BACKUP_BUSY = Reason(
    "backup_busy",
    "I couldn't start that because another backup task is already running. Try again in a moment.",
    503,
)
BACKUP_NOT_CONFIRMED = Reason(
    "backup_not_confirmed",
    "I couldn't turn on backup until you confirm your recovery key.",
    400,
)
RECOVERY_KEY_MISMATCH = Reason(
    "recovery_key_mismatch",
    "I couldn't confirm that — it didn't match your recovery key.",
    400,
)
BACKUP_OPERATION_FAILED = Reason(
    "backup_operation_failed",
    "I couldn't finish that backup action.",
    500,
)
BACKUP_UNAVAILABLE = Reason(
    "backup_unavailable",
    "I couldn't start a backup because your journal's background service isn't running. Start it, then try again.",
    503,
)
LOCAL_REQUEST_ONLY = Reason(
    "local_request_only",
    "I couldn't register that observer because this request isn't authorized for "
    "that stream.",
    403,
)

# entities
ENTITY_NOT_FOUND = Reason("entity_not_found", "I couldn't find that entity.", 404)
ENTITY_ALREADY_EXISTS = Reason(
    "entity_already_exists",
    "I couldn't save that entity because it already exists.",
    409,
)
ENTITY_ALIAS_CONFLICT = Reason(
    "entity_alias_conflict",
    "I couldn't save that alias because it conflicts with another entity.",
    409,
)
ENTITY_BLOCKED = Reason(
    "entity_blocked",
    "I couldn't use that speaker because it's blocked.",
    400,
)
INVALID_ENTITY_TYPE = Reason(
    "invalid_entity_type",
    "I couldn't use that entity type.",
    400,
)
PRINCIPAL_ENTITY_PROTECTED = Reason(
    "principal_entity_protected",
    "I can't delete the principal entity.",
    400,
)
ENTITY_OPERATION_FAILED = Reason(
    "entity_operation_failed",
    "I couldn't finish that entity change.",
    500,
)
EDGE_INDEX_UNAVAILABLE = Reason(
    "edge_index_unavailable",
    "I couldn't read your connections because the index hasn't been built yet. Run `journal indexer --rebuild-edges` to build it.",
    503,
)

# facets/activities
FACET_NOT_FOUND = Reason("facet_not_found", "I couldn't find that facet.", 404)
FACET_ALREADY_EXISTS = Reason(
    "facet_already_exists",
    "I couldn't create that facet because it already exists.",
    409,
)
ACTIVITY_INVALID = Reason(
    "activity_invalid",
    "I couldn't use that activity setting.",
    400,
)
ACTIVITY_NOT_FOUND = Reason(
    "activity_not_found",
    "I couldn't find that activity in the facet.",
    404,
)
ACTIVITY_ALREADY_EXISTS = Reason(
    "activity_already_exists",
    "I couldn't create that activity because it already exists.",
    409,
)
ACTIVITY_PROTECTED = Reason(
    "activity_protected",
    "I can't remove that always-on activity.",
    400,
)

# search
SEARCH_FAILED = Reason("search_failed", "I couldn't search for that.", 400)

# agent/talent
AGENT_UNAVAILABLE = Reason(
    "agent_unavailable",
    "I couldn't reach the agent service.",
    503,
)
CHAT_QUEUE_FULL = Reason("chat_queue_full", "Chat queue full", 429)
TALENT_RUN_PENDING = Reason(
    "talent_run_pending",
    "I'm still working on that talent run.",
    202,
)
TALENT_NOT_FOUND = Reason(
    "talent_not_found",
    "I couldn't find that talent run.",
    404,
)
TALENT_RUN_MALFORMED = Reason(
    "talent_run_malformed",
    "I couldn't read that talent run.",
    500,
)
TALENT_OPERATION_FAILED = Reason(
    "talent_operation_failed",
    "I couldn't load that talent data.",
    500,
)

# health
HEALTH_REPORT_FAILED = Reason(
    "health_report_failed",
    "I couldn't build your journal health report.",
    500,
)

# voice / push / support
FEATURE_UNAVAILABLE = Reason(
    "feature_unavailable",
    "I couldn't use that feature because it isn't enabled.",
    403,
)
PROVIDER_KEY_MISSING = Reason(
    "provider_key_missing",
    "I couldn't start because that provider key is missing.",
    503,
)
VOICE_UNAVAILABLE = Reason(
    "voice_unavailable",
    "I couldn't start voice right now.",
    503,
)
OBSERVER_RESTART_FAILED = Reason(
    "observer_restart_failed",
    "i couldn't restart sol's processing.",
    503,
)
PUSH_REQUEST_INVALID = Reason(
    "push_request_invalid",
    "I couldn't use that push request.",
    400,
)
SUPPORT_PORTAL_FAILED = Reason(
    "support_portal_failed",
    "I couldn't reach support right now.",
    500,
)
OPERATION_IN_PROGRESS = Reason(
    "operation_in_progress",
    "That operation is already in progress.",
    409,
)
IDEMPOTENCY_CONFLICT = Reason(
    "idempotency_conflict",
    "That operation conflicts with an earlier attempt.",
    409,
)
SUPPORT_INVALID_STATE = Reason(
    "invalid_state",
    "That operation isn't available in the current state.",
    409,
)
SUPPORT_TOS_CHANGED = Reason(
    "tos_changed",
    "Support terms changed and require re-consent.",
    401,
)
OPERATION_RETIRED = Reason(
    "operation_retired",
    "That operation is no longer available.",
    410,
)
OPERATION_ERASED = Reason(
    "operation_erased",
    "That operation was erased.",
    410,
)

# import / ingest
IMPORT_NOT_FOUND = Reason("import_not_found", "I couldn't find that import.", 404)
IMPORT_CONFLICT = Reason(
    "import_conflict",
    "I couldn't start that import because it already exists.",
    409,
)
IMPORT_CLIENT_ID_CONFLICT = Reason(
    "import_client_id_conflict",
    "That client_item_id is already staged for different content.",
    409,
)
IMPORT_METADATA_FAILED = Reason(
    "import_metadata_failed",
    "I couldn't update that import metadata.",
    500,
)
IMPORT_QUEUE_UNREACHABLE = Reason(
    "import_queue_unreachable",
    "your journal's background service isn't running. start it, then try again.",
    503,
)
JOURNAL_SOURCE_PROBLEM = Reason(
    "journal_source_problem",
    "I couldn't use that journal source.",
    400,
)
INGEST_NO_FILES = Reason(
    "ingest_no_files",
    "I couldn't find any files to bring in.",
    400,
)
INGEST_STORAGE_FAILED = Reason(
    "ingest_storage_failed",
    "I couldn't store those files.",
    500,
)
INGEST_CONTRACT_INVALID = Reason(
    "ingest_contract_invalid",
    "I couldn't use those observer files because they don't match the journal contract.",
    422,
)
INGEST_SIDECAR_CONFLICT = Reason(
    "ingest_sidecar_conflict",
    "I couldn't bring in those observer sidecars because they conflict with files already held.",
    409,
)

# speakers
SPEAKER_OWNER_VOICE_TOO_CLOSE = Reason(
    "speaker_owner_voice_too_close",
    "I couldn't save that voice because it sounds too much like yours.",
    400,
)
SPEAKER_REVIEW_UNAVAILABLE = Reason(
    "speaker_review_unavailable",
    "I couldn't load that speaker review.",
    404,
)
SPEAKER_SENTENCE_MISSING = Reason(
    "speaker_sentence_missing",
    "I couldn't find that sentence. Try refreshing the page.",
    404,
)
SPEAKER_ATTRIBUTION_STATE_INVALID = Reason(
    "speaker_attribution_state_invalid",
    "I couldn't apply that change because the sentence isn't in the right state.",
    400,
)
SPEAKER_NOT_FOUND = Reason(
    "speaker_not_found",
    "I couldn't find that speaker. They may have been removed — try refreshing the page.",
    404,
)
SPEAKER_OWNER_IDENTITY_REQUIRED = Reason(
    "speaker_owner_identity_required",
    "Set your journal identity before tagging your voice.",
    400,
)
SPEAKER_VOICEPRINT_BUSY = Reason(
    "speaker_voiceprint_busy",
    "I couldn't update that voice right now because it was busy. Try again in a moment.",
    503,
)
SPEAKER_LABELS_BUSY = Reason(
    "speaker_labels_busy",
    "I couldn't update those speaker attributions right now because they were busy. Try again in a moment.",
    503,
)
SPEAKER_OWNER_CENTROID_REQUIRED = Reason(
    "speaker_owner_centroid_required",
    "I couldn't run that speaker command until your owner voice is set up.",
    409,
)
SPEAKER_COMMAND_FAILED = Reason(
    "speaker_command_failed",
    "I couldn't finish that speaker command.",
    400,
)
SPEAKER_DISCOVERY_FAILED = Reason(
    "speaker_discovery_failed",
    "i couldn't look for new voices right now.",
)
SPEAKER_IDENTIFY_RECOVERABLE = Reason(
    "speaker_identify_recoverable",
    "I couldn't finish that speaker identify operation, but it can be retried.",
    409,
)
SPEAKER_IDENTIFY_REPAIR_REQUIRED = Reason(
    "speaker_identify_repair_required",
    "I couldn't safely finish that speaker identify operation without repair.",
    409,
)
SPEAKER_IDENTIFY_CONFLICT = Reason(
    "speaker_identify_conflict",
    "I couldn't run that speaker identify operation because it conflicts with existing state.",
    409,
)
SPEAKER_IDENTIFY_OPERATION_NOT_FOUND = Reason(
    "speaker_identify_operation_not_found",
    "I couldn't find that speaker identify operation.",
    404,
)
AWARENESS_BUSY = Reason(
    "awareness_busy",
    "I couldn't update what I know right now because it was busy. Try again in a moment.",
    503,
)
AWARENESS_SECTION_NOT_FOUND = Reason(
    "awareness_section_not_found",
    "I couldn't find that part of what I know.",
    404,
)
ENTITY_BUSY = Reason(
    "entity_busy",
    "I couldn't update that entity right now because it was busy. Try again in a moment.",
    503,
)
ACTIVITIES_BUSY = Reason(
    "activities_busy",
    "I couldn't update activities right now because they were busy. Try again in a moment.",
    503,
)

# ledger
LEDGER_ITEM_NOT_FOUND = Reason(
    "ledger_item_not_found",
    "I couldn't find that ledger item.",
    404,
)

# identity
IDENTITY_BUSY = Reason(
    "identity_busy",
    "I couldn't update my identity right now because it was busy. Try again in a moment.",
    503,
)

# reprocess
REPROCESS_PAST_ONLY = Reason(
    "reprocess_past_only",
    "you can only reprocess past days — today and future days aren't ready yet.",
    400,
)
REPROCESS_UNREACHABLE = Reason(
    "reprocess_unreachable",
    "your journal's background service isn't running. start it, then try again.",
    503,
)
# Success-payload reason: intentionally not routed through error_response.
# Copy is locked verbatim, so this deviates from the "I couldn't…" house style.
REPROCESS_ALREADY_COMPLETE = Reason(
    "reprocess_already_complete",
    "this day's already done. want to redo it from scratch?",
    200,
)
# Success-payload reason like REPROCESS_ALREADY_COMPLETE.
REPROCESS_HELD_BY_BACKOFF = Reason(
    "reprocess_held_by_backoff",
    "sol's not retrying this day until {when}. to start it over right now, use redo from scratch.",
    200,
)
