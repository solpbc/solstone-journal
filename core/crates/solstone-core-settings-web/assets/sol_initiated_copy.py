# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Locked literals for sol-initiated chat."""

KIND_OWNER_CHAT_DISMISSED = "owner_chat_dismissed"
KIND_OWNER_CHAT_OPEN = "owner_chat_open"
KIND_OWNER_MESSAGE = "owner_message"
KIND_SOL_CHAT_REQUEST = "sol_chat_request"
KIND_SOL_CHAT_REQUEST_SUPERSEDED = "sol_chat_request_superseded"
SURFACE_CONVEY = "convey"
SOL_PINGED_OFFLINE_TOOLTIP = "sol-pinged but offline — refresh"

TRIGGER_LABEL_SOL_INITIATED = "sol_initiated"

THROTTLE_MUTE_WINDOW = "mute-window"
THROTTLE_RATE_FLOOR = "rate-floor"
THROTTLE_CATEGORY_SELF_MUTE = "category-self-mute"
THROTTLE_CATEGORY_CAP = "category-cap"
THROTTLE_DAILY_CAP = "daily-cap"

CATEGORIES = ("briefing", "pattern", "commitment", "error", "arrival", "notice")
CATEGORY_CAP_DEFAULTS = {
    "briefing": 3,
    "pattern": 2,
    "commitment": 2,
    "error": 2,
    "arrival": 3,
    "notice": 2,
}

# Chat origin tag, settings UI, push category.
APNS_CATEGORY_SOL_CHAT_REQUEST = "SOLSTONE_SOL_CHAT_REQUEST"
SOL_VOICE_SETTINGS_CATEGORY_CAP_AUTO_MUTED_FORMAT = "auto-muted until {date}"
SOL_VOICE_SETTINGS_CATEGORY_CAP_CLEAR_BUTTON = "clear auto-mute"
SOL_VOICE_SETTINGS_CATEGORY_CAPS_LABEL = "per-category caps"
SOL_VOICE_SETTINGS_DAILY_CAP_LABEL = "daily cap"
SOL_VOICE_SETTINGS_DEBUG_HEADING = "debug"
SOL_VOICE_SETTINGS_DEBUG_SHOW_THROTTLED_LABEL = "show throttled chat starts"
SOL_VOICE_SETTINGS_HEADING = "sol-initiated chat"
SOL_VOICE_SETTINGS_QUIET_HOURS_END_LABEL = "end (24h)"
SOL_VOICE_SETTINGS_QUIET_HOURS_HEADING = "quiet hours"
SOL_VOICE_SETTINGS_QUIET_HOURS_START_LABEL = "start (24h)"
SOL_VOICE_SETTINGS_RATE_FLOOR_LABEL = "minimum gap between starts (minutes)"
SOL_VOICE_SETTINGS_SYSTEM_NOTIFICATIONS_HEADING = "system notifications"
SOL_VOICE_SETTINGS_SYSTEM_NOTIFICATIONS_LINUX_LABEL = "linux"
SOL_VOICE_SETTINGS_SYSTEM_NOTIFICATIONS_MACOS_LABEL = "macos"
SOL_VOICE_THROTTLED_LOG_EMPTY = "no throttled starts in recent history"
SOL_VOICE_THROTTLED_LOG_ERROR = "unable to load throttled log"
SOL_VOICE_THROTTLED_LOG_HEADER_CATEGORY = "category"
SOL_VOICE_THROTTLED_LOG_HEADER_DEDUPE_KEY = "dedupe key"
SOL_VOICE_THROTTLED_LOG_HEADER_OUTCOME = "outcome"
SOL_VOICE_THROTTLED_LOG_HEADER_WHEN = "when"
