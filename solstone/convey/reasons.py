# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from dataclasses import dataclass


@dataclass(frozen=True)
class Reason:
    code: str
    message: str
    status: int = 400


# auth
AUTH_REQUIRED = Reason("auth_required", "I couldn't verify this request.", 401)
AUTH_KEY_INVALID = Reason("auth_key_invalid", "I couldn't verify that key.", 401)
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
CONVEY_OPERATION_FAILED = Reason(
    "convey_operation_failed",
    "I couldn't update the interface settings.",
    500,
)
NETWORK_SECURITY_REQUIRES_PASSWORD = Reason(
    "network_security_requires_password",
    "I couldn't change network access until a password is set.",
    400,
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

# facets/activities/todos
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
ACTIVITY_PROTECTED = Reason(
    "activity_protected",
    "I can't remove that always-on activity.",
    400,
)
TODO_OPERATION_FAILED = Reason(
    "todo_operation_failed",
    "I couldn't update that todo.",
    500,
)

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
    "I couldn't restart observer processing.",
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

# import / ingest
IMPORT_NOT_FOUND = Reason("import_not_found", "I couldn't find that import.", 404)
IMPORT_CONFLICT = Reason(
    "import_conflict",
    "I couldn't start that import because it already exists.",
    409,
)
IMPORT_METADATA_FAILED = Reason(
    "import_metadata_failed",
    "I couldn't update that import metadata.",
    500,
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

# reprocess
REPROCESS_PAST_ONLY = Reason(
    "reprocess_past_only",
    "you can only reprocess past days — today and future days aren't ready yet.",
    400,
)
REPROCESS_UNREACHABLE = Reason(
    "reprocess_unreachable",
    "solstone's background service isn't running. start it, then try again.",
    503,
)
# Success-payload reason: intentionally not routed through error_response.
# Copy is locked verbatim, so this deviates from the "I couldn't…" house style.
REPROCESS_ALREADY_COMPLETE = Reason(
    "reprocess_already_complete",
    "this day's already done. want to redo it from scratch?",
    200,
)
