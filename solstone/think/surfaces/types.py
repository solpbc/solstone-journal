# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from dataclasses import dataclass


@dataclass(frozen=True)
class ActivitySourceRef:
    facet: str
    day: str
    activity_id: str
    field: str
    created_at: int


@dataclass(frozen=True)
class LedgerItem:
    id: str
    state: str
    owner: str
    owner_entity_id: str | None
    counterparty: str | None
    counterparty_entity_id: str | None
    action: str
    summary: str  # summary == action verbatim; CLI composes "owner: action → counterparty" at render time if it wants.
    when: str | None
    context: str
    opened_at: int
    closed_at: int | None
    age_days: int
    sources: tuple[ActivitySourceRef, ...]


@dataclass(frozen=True)
class Decision:
    id: str
    owner: str
    owner_entity_id: str | None
    action: str
    context: str
    day: str
    created_at: int
    source: ActivitySourceRef


@dataclass(frozen=True)
class Cadence:
    recent_interactions_count_30d: int
    last_seen: str | None
    avg_interval_days: float | None
    gone_quiet_since: int | None


@dataclass(frozen=True)
class ProfileBrief:
    entity_id: str
    name: str
    type: str
    description: str | None
    last_seen: str | None
    open_loop_count: int
    decisions_count_30d: int


@dataclass(frozen=True)
class Profile:
    entity_id: str
    name: str
    type: str
    aka: tuple[str, ...]
    is_self: bool
    facets: tuple[str, ...]
    description: str | None
    cadence: Cadence
    open_with_them: tuple[LedgerItem, ...]
    closed_with_them_30d: tuple[LedgerItem, ...]
    decisions_involving_them: tuple[Decision, ...]
    sources: tuple[ActivitySourceRef, ...]
    generated_at: int


@dataclass(frozen=True)
class CaptureHealth:
    hours_with_capture: int
    hours_total: int
    coverage_ratio: float | None
    facets_with_recent_capture: tuple[str, ...]
    facets_silent_24h: tuple[str, ...]
    last_segment_at: int | None


@dataclass(frozen=True)
class SynthesisHealth:
    activities_count: int
    activities_with_participation: int
    activities_with_story: int
    activities_user_edited: int
    activities_anticipated_unfilled: int
    talent_run_failures_24h: int | None
    indexer_last_rebuild_at: int | None


@dataclass(frozen=True)
class ConsumerSignalHealth:
    ledger_open_items_total: int
    ledger_stale_items_count: int
    profile_entities_total: int


@dataclass(frozen=True)
class HealthNote:
    severity: str
    category: str
    message: str
    detected_at: int
    detail_pointer: str | None


@dataclass(frozen=True)
class SegmentBacklogHealth:
    not_thought: int
    days_with_backlog: int
    errors: tuple[str, ...]


@dataclass(frozen=True)
class HealthReport:
    generated_at: int
    range: tuple[str, str]
    facets: tuple[str, ...]
    capture_health: CaptureHealth
    synthesis_health: SynthesisHealth
    consumer_signal: ConsumerSignalHealth
    segment_backlog: SegmentBacklogHealth
    notes: tuple[HealthNote, ...]
