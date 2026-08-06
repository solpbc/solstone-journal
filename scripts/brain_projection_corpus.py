#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Behavioural oracle for reading `health/brain.json`.

The vocabularies of the active-brain record are pinned in
`local_contract.json`. This is the other half: what the reference implementation
*decides* when it reads one.

Two functions carry that decision and neither is a lookup.
`validate_brain_state_record` accepts or refuses a persisted record against a
closed schema — an unknown field is rejected, never ignored — and its refusal
message names the offending path. `project_brain_state` then reduces four
evidence components, a checking lease, a runtime-failure marker, the live
configuration and the runtime's own health record into a single aggregate state
and one reason code.

⚠ **The projection is where a wrong port is invisible.** A misread record does
not crash and does not corrupt: it produces a different aggregate state, and the
journal either reports itself unhealthy when it is fine or ready when it is not.
The first costs the owner a working journal; the second sends their material to
a lane that cannot process it. Neither reddens a test that was written by
reading the new implementation.

So every case here records the reference's answer for an input, and a native
implementation is compared against the answer rather than against a restatement
of its own logic. The refusal messages are recorded verbatim for the same
reason: a validator that refuses the right records for the wrong reason is one
edit away from refusing the wrong ones.

⚠ **`runtime_health` is covered for the shapes the projection branches on**, not
for the runtime record's own lifecycle — that is the runtime's contract and is
pinned where the runtime is converted. The `local_runtime_*` reason codes below
are the projection's *reading* of a runtime record, never the runtime's own
state machine.

Determinism: `now` is a fixed instant and every timestamp is expressed relative
to it, so the corpus does not move when it is regenerated. The HMAC key is fixed
for the same reason; the real key is 32 random bytes per journal.

⚠ Regenerating requires a runnable reference tree. This is a frozen record.
"""

from __future__ import annotations

import hashlib
import json
from datetime import datetime, timedelta, timezone
from typing import Any

from local_contract_corpus import REFERENCE_SOURCES, reference_commit
from solstone.think.providers import brain_state as bs

FIXTURE_VERSION = 1

NOW = datetime(2026, 8, 6, 12, 0, 0, tzinfo=timezone.utc)
CORPUS_HMAC_KEY = bytes(range(32))

FINGERPRINT_A = "a" * 64
FINGERPRINT_B = "b" * 64
# The bundled runtime's own artifact digest. On the local lane the runtime
# record carries this as its `desired_fingerprint_sha256`, and the projection
# INJECTS it into the config fingerprint rather than probing the installed
# artifact — so a runtime record advertising a different digest changes the
# journal's fingerprint and reads as `brain_config_changed`, not as a runtime
# problem. Pinning one value here is what lets the corpus reach the branches
# past that check.
BUNDLED_RUNTIME_SHA = "c" * 64


def _iso(offset_seconds: float) -> str:
    return (NOW + timedelta(seconds=offset_seconds)).isoformat().replace("+00:00", "Z")


def _component(
    status: str,
    *,
    observed: float = -60.0,
    reason: str | None = None,
    expires: float | None = None,
    diagnostic: dict[str, Any] | None = None,
) -> dict[str, Any]:
    component: dict[str, Any] = {"status": status, "observed_at": _iso(observed)}
    if reason is not None:
        component["reason_code"] = reason
    if expires is not None:
        component["expires_at"] = _iso(expires)
    if diagnostic is not None:
        component["diagnostic"] = diagnostic
    return component


def _evidence(**components: Any) -> dict[str, Any]:
    record = {name: None for name in bs.COMPONENT_ORDER}
    record.update(components)
    return record


def _record(
    *,
    lane: str,
    provider: str | None = "local",
    model: str | None = "local/qwen3.5-4b",
    fingerprint: str | None = FINGERPRINT_A,
    checking: dict[str, Any] | None = None,
    evidence: dict[str, Any] | None = None,
    marker: dict[str, Any] | None = None,
    diagnostic: dict[str, Any] | None = None,
    revision: int = 3,
    updated: float = 0.0,
) -> dict[str, Any]:
    """Build a record the way the reference writer builds one.

    🔴 The validator is a CROSS-FIELD consistency check, not a shape check: it
    recomputes the aggregate and reason from the evidence and refuses a record
    whose stored pair disagrees, refuses a checking record whose marker does not
    match, and refuses a lane whose applicable evidence is absent. Hand-authoring
    the aggregate is therefore a way to author records the product could never
    have written — seven of the first draft's "valid" records were refused for
    exactly that reason. Routing through `_record_from_evidence` makes every
    corpus record one the reference itself would emit.
    """
    return bs._record_from_evidence(
        evidence=evidence if evidence is not None else _evidence(),
        fingerprint={
            "ok": True,
            "fingerprint_sha256": fingerprint,
            "active_lane": lane,
            "active_provider": provider,
            "active_model": model,
            "reason_code": None,
            "diagnostic": diagnostic or {},
        },
        revision=revision,
        now=NOW + timedelta(seconds=updated),
        checking=checking,
        runtime_failure_marker=marker,
    )


def _checking(
    *,
    expires: float,
    fingerprint: str | None = FINGERPRINT_A,
    revision: int = 3,
    marker_seen: str | None = None,
) -> dict[str, Any]:
    return {
        "run_id": "0123456789abcdef",
        "started_at": _iso(expires - bs.CHECKING_TTL.total_seconds()),
        "expires_at": _iso(expires),
        "fingerprint_sha256": fingerprint,
        "checking_revision": revision,
        "runtime_failure_marker_seen": marker_seen,
    }


def _marker(reason: str, *, revision: int = 3, recorded: float = -10.0) -> dict[str, Any]:
    return {
        "marker_id": "fedcba9876543210",
        "revision": revision,
        "recorded_at": _iso(recorded),
        "reason_code": reason,
    }


def _ready_evidence(lane: str) -> dict[str, Any]:
    ttl = bs.DEFAULT_READY_EVIDENCE_TTL.total_seconds()
    return _evidence(
        **{
            name: _component("ok", expires=ttl - 3600.0)
            for name in bs.LANE_COMPONENTS[lane]
        }
    )


# --------------------------------------------------------------------------
# configurations — one per lane, plus the two ways a config fails to name one
# --------------------------------------------------------------------------

ENDPOINT_URL = "http://127.0.0.1:9099"
ENDPOINT_MODEL = "served-model"
ENDPOINT_CREDENTIAL = "endpoint-credential"
ENDPOINT_CREDENTIAL_SHA = hashlib.sha256(
    ENDPOINT_CREDENTIAL.encode("utf-8")
).hexdigest()

# 🔴 The lane is derived from the config, and the derivation is not obvious.
# `local` is FOUR lanes depending on what sits beside it: no endpoint at all is
# `bundled`; a complete endpoint is `byo-endpoint`; a complete endpoint whose
# confidential provenance block matches it exactly is `spp`; and a half-written
# endpoint — one of the two required keys — resolves to NO lane and projects
# `configuration_invalid` rather than falling back to bundled.
#
# ⚠ A first draft of this corpus used plausible-looking key names (`base_url`,
# `api_key`) instead of the real ones. Every config still produced a valid
# fingerprint, so nothing failed — but `lane_byo_endpoint` silently resolved to
# `bundled`, and `spp` and `byo-endpoint` were reached by ZERO cases while the
# corpus claimed five-lane coverage.
CONFIGS: dict[str, dict[str, Any]] = {
    "lane_bundled": {"providers": {"active": {"provider": "local"}}},
    "lane_none": {"providers": {"active": {"provider": "none"}}},
    "lane_byo_cloud": {
        "providers": {"active": {"provider": "anthropic"}},
        "env": {"ANTHROPIC_API_KEY": "sk-test"},
    },
    "lane_byo_endpoint": {
        "providers": {
            "active": {"provider": "local", "model": ENDPOINT_MODEL},
            "local": {
                "endpoint_url": ENDPOINT_URL,
                "served_model_id": ENDPOINT_MODEL,
                "credential": ENDPOINT_CREDENTIAL,
            },
        }
    },
    "lane_spp": {
        "providers": {
            "active": {"provider": "local", "model": ENDPOINT_MODEL},
            "local": {
                "endpoint_url": ENDPOINT_URL,
                "served_model_id": ENDPOINT_MODEL,
                "credential": ENDPOINT_CREDENTIAL,
            },
        },
        "services": {
            "confidential": {
                "endpoint_url": ENDPOINT_URL,
                "served_model_id": ENDPOINT_MODEL,
                "credential_fingerprint_sha256": ENDPOINT_CREDENTIAL_SHA,
                # A restore-only field: present in the provenance block and
                # deliberately excluded from the fingerprint, so a corpus that
                # included it would pin the wrong digest.
                "prior_active": {"provider": "local"},
            }
        },
    },
    # A confidential block that does not match the live endpoint resolves to no
    # lane at all rather than degrading to `byo-endpoint` — the journal refuses
    # to guess whether the owner meant confidential processing.
    "config_confidential_unmatched": {
        "providers": {
            "active": {"provider": "local", "model": ENDPOINT_MODEL},
            "local": {
                "endpoint_url": ENDPOINT_URL,
                "served_model_id": ENDPOINT_MODEL,
                "credential": ENDPOINT_CREDENTIAL,
            },
        },
        "services": {
            "confidential": {
                "endpoint_url": "https://somewhere-else.example",
                "served_model_id": ENDPOINT_MODEL,
                "credential_fingerprint_sha256": ENDPOINT_CREDENTIAL_SHA,
            }
        },
    },
    "config_endpoint_partial": {
        "providers": {
            "active": {"provider": "local"},
            "local": {"endpoint_url": ENDPOINT_URL},
        }
    },
    "config_missing_provider": {"providers": {"active": {}}},
    "config_unknown_provider": {"providers": {"active": {"provider": "nonesuch"}}},
}


# --------------------------------------------------------------------------
# runtime health — only the shapes the projection actually branches on
# --------------------------------------------------------------------------


def _runtime(status: str, record: Any) -> dict[str, Any]:
    return {
        "status": status,
        "provider": "local",
        "record_kind": "health",
        "path": "health/providers/runtime/local.json",
        "record": record,
        "reason_code": None,
        "error": None,
    }


def _health(
    phase: str, *, reason: str | None = None, desired: str | None = BUNDLED_RUNTIME_SHA
) -> dict[str, Any]:
    return {
        "schema_version": 1,
        "provider": "local",
        "revision": 5,
        "phase": phase,
        "reason_code": reason,
        "detail": {},
        "desired_fingerprint_sha256": desired,
        "incarnation": "inc",
        "generation": 1,
        "attempt": 0,
        "process": None,
        "updated_at": _iso(-15.0),
        "display_deadline_at": None,
        "owner": None,
    }


RUNTIMES: dict[str, Any] = {
    "absent": None,
    "ok_ready": _runtime("ok", _health("ready")),
    "ok_ready_fingerprint_b": _runtime("ok", _health("ready", desired=FINGERPRINT_B)),
    "ok_ready_no_desired": _runtime("ok", _health("ready", desired=None)),
    "ok_starting": _runtime("ok", _health("starting")),
    "ok_failed": _runtime("ok", _health("failed", reason="local-server-unhealthy")),
    "ok_host_blocked": _runtime("ok", _health("host-blocked", reason="gpu-unavailable")),
    "ok_install_in_progress": _runtime("ok", _health("observing", reason="install-in-progress")),
    "ok_unknown_phase": _runtime("ok", _health("phase-that-does-not-exist")),
    "ok_record_not_a_mapping": _runtime("ok", "not-a-record"),
    "corrupt": _runtime("corrupt", None),
    "unavailable": _runtime("unavailable", None),
}

# One incoherent (phase, reason) pair straight out of the reference table, so a
# native implementation cannot pass by ignoring the coherence check entirely.
if bs.INCOHERENT_RUNTIME_PHASE_REASON_CODES:
    _incoherent_phase, _incoherent_reason = sorted(
        bs.INCOHERENT_RUNTIME_PHASE_REASON_CODES
    )[0]
    RUNTIMES["ok_incoherent_phase_reason"] = _runtime(
        "ok", _health(_incoherent_phase, reason=_incoherent_reason)
    )


# --------------------------------------------------------------------------
# records
# --------------------------------------------------------------------------


def _live_fingerprint(config: dict[str, Any]) -> dict[str, Any]:
    """The fingerprint the reference computes for this config, right now.

    🔴 Load-bearing for the corpus, not a detail. A record whose fingerprint does
    not equal this value never reaches any evidence branch: the projection
    answers `brain_config_changed` and stops. A first draft of this corpus used a
    placeholder digest and produced 302 `brain_config_changed` answers and **zero
    `ready`** — every case green, the entire success path unexercised.

    The bundled runtime's own digest is passed in rather than probed so the
    corpus does not depend on what happens to be installed on the machine that
    regenerates it.
    """
    return bs.build_active_brain_fingerprint(
        config,
        hmac_key=CORPUS_HMAC_KEY,
        bundled_runtime_fingerprint_sha256=BUNDLED_RUNTIME_SHA,
    )


def _lane_records(
    lane: str, provider: str | None, model: str | None, fingerprint: str | None
) -> dict[str, Any]:
    """Every record variant for one lane, at that lane's live fingerprint."""
    ttl = bs.DEFAULT_READY_EVIDENCE_TTL.total_seconds()
    components = bs.LANE_COMPONENTS[lane]
    common = {"lane": lane, "provider": provider, "model": model, "fingerprint": fingerprint}

    records: dict[str, Any] = {
        "ready": _record(**common, evidence=_ready_evidence(lane)),
        "ready_expiring_within_the_hour": _record(
            **common,
            evidence=_evidence(
                **{name: _component("ok", expires=1800.0) for name in components}
            ),
        ),
        "evidence_expired": _record(
            **common,
            evidence=_evidence(
                **{
                    name: _component("ok", observed=-ttl - 60.0, expires=-60.0)
                    for name in components
                }
            ),
        ),
        "checking_fresh": _record(
            **common, checking=_checking(expires=300.0, fingerprint=fingerprint),
            evidence=_ready_evidence(lane),
        ),
        # 🔴 The crashed-refresh record, and the reason it is written this way.
        # A checking block is only writable while it is still fresh AT WRITE
        # TIME — the validator re-reduces the evidence against the record's own
        # `updated_at`, not against the reader's clock. So the on-disk shape a
        # killed refresh leaves behind is a record written 15 minutes ago whose
        # lease expired 5 minutes ago: valid to validate, expired to project.
        # Authoring it "expired at write time" instead produces a record the
        # product could never emit, which is what the first draft did.
        "checking_expired_after_crash": _record(
            **common,
            updated=-900.0,
            checking=_checking(expires=-300.0, fingerprint=fingerprint),
            evidence=_ready_evidence(lane),
        ),
        "checking_other_fingerprint": _record(
            **common, checking=_checking(expires=300.0, fingerprint=FINGERPRINT_B),
            evidence=_ready_evidence(lane),
        ),
        "fingerprint_other": _record(
            **{**common, "fingerprint": FINGERPRINT_B}, evidence=_ready_evidence(lane)
        ),
        "fingerprint_absent": _record(
            **{**common, "fingerprint": None}, evidence=_ready_evidence(lane)
        ),
        "updated_at_in_the_future": _record(
            **common, updated=86400.0, evidence=_ready_evidence(lane)
        ),
    }

    if lane == "none":
        return records

    # Reasons are PARTITIONED per component — 32 evidence-recordable reasons
    # split across four components, plus 10 the projection may produce and no
    # record may carry. A reason on the wrong component is refused, so these
    # pick from each component's own set rather than from the vocabulary.
    prereq_reason = "local_runtime_not_ready" if lane == "bundled" else "provider_key_missing"
    records |= {
        "prerequisites_blocked": _record(
            **common,
            evidence=_evidence(
                configuration=_component("ok", expires=ttl - 3600.0),
                lane_prerequisites=_component("blocked", reason=prereq_reason),
                generate=_component("not_attempted", reason=prereq_reason),
                cogitate=_component("not_attempted", reason=prereq_reason),
            ),
        ),
        "generate_failed": _record(
            **common,
            evidence=_evidence(
                configuration=_component("ok", expires=ttl - 3600.0),
                lane_prerequisites=_component("ok", expires=ttl - 3600.0),
                generate=_component("failed", reason="provider_unavailable"),
                cogitate=_component("ok", expires=ttl - 3600.0),
            ),
        ),
        "cogitate_failed_generate_ok": _record(
            **common,
            evidence=_evidence(
                configuration=_component("ok", expires=ttl - 3600.0),
                lane_prerequisites=_component("ok", expires=ttl - 3600.0),
                generate=_component("ok", expires=ttl - 3600.0),
                cogitate=_component("failed", reason="cogitate_terminal_error"),
            ),
        ),
        "marker_current": _record(
            **common,
            evidence=_ready_evidence(lane),
            marker=_marker("provider_unavailable"),
        ),
        "marker_superseded": _record(
            **common,
            revision=9,
            evidence=_ready_evidence(lane),
            marker=_marker("provider_unavailable", revision=3),
        ),
    }
    return records


def _records_by_config() -> dict[str, dict[str, Any]]:
    """Record variants keyed by the config whose fingerprint they carry."""
    by_config: dict[str, dict[str, Any]] = {}
    for config_name, config in sorted(CONFIGS.items()):
        fingerprint = _live_fingerprint(config)
        if not fingerprint["ok"] or fingerprint["active_lane"] is None:
            # A config that names no lane has no records of its own; it still
            # appears in the projection matrix as a context.
            continue
        by_config[config_name] = _lane_records(
            fingerprint["active_lane"],
            fingerprint["active_provider"],
            fingerprint["active_model"],
            fingerprint["fingerprint_sha256"],
        )
    return by_config


# --------------------------------------------------------------------------
# malformed records — the refusal path, message included
# --------------------------------------------------------------------------


def _malformed() -> list[tuple[str, Any, str]]:
    base = _record(
        lane="bundled",
        fingerprint=_live_fingerprint(CONFIGS["lane_bundled"])["fingerprint_sha256"],
        evidence=_ready_evidence("bundled"),
    )

    def mutate(**changes: Any) -> dict[str, Any]:
        copy = json.loads(json.dumps(base))
        copy.update(changes)
        return copy

    def drop(field: str) -> dict[str, Any]:
        copy = json.loads(json.dumps(base))
        copy.pop(field)
        return copy

    cases: list[tuple[str, Any, str]] = [
        ("not_a_mapping", ["nope"], "a record must be an object"),
        ("unknown_top_level_field", mutate(extra=1), "an unknown field is refused, not ignored"),
        ("missing_schema_version", drop("schema_version"), "a required field is absent"),
        ("wrong_schema_version", mutate(schema_version=99), "a future schema is refused"),
        ("unknown_lane", mutate(active_lane="lane-that-does-not-exist"), "the lane vocabulary is closed"),
        ("unknown_aggregate", mutate(aggregate_state="fine"), "the aggregate vocabulary is closed"),
        ("unknown_reason", mutate(reason_code="reason-that-does-not-exist"), "the reason vocabulary is closed"),
        ("fingerprint_not_hex", mutate(fingerprint_sha256="zz"), "the fingerprint is validated as hex"),
        ("fingerprint_wrong_length", mutate(fingerprint_sha256="ab"), "length is part of the hex rule"),
        ("revision_not_int", mutate(revision="3"), "a numeric field is not coerced from text"),
        ("revision_negative", mutate(revision=-1), "revisions do not go backwards past zero"),
        ("updated_at_naive", mutate(updated_at="2026-08-06T12:00:00"), "a timestamp must carry a zone"),
        ("updated_at_garbage", mutate(updated_at="yesterday"), "a timestamp must parse"),
        ("evidence_not_a_mapping", mutate(evidence=[]), "evidence is an object"),
        (
            "evidence_unknown_component",
            mutate(evidence={**base["evidence"], "invent": None}),
            "the component set is closed",
        ),
        (
            "evidence_component_unknown_field",
            mutate(
                evidence={
                    **base["evidence"],
                    "configuration": {**_component("ok"), "surprise": 1},
                }
            ),
            "a component's field set is closed too",
        ),
        (
            "evidence_component_unknown_status",
            mutate(evidence={**base["evidence"], "configuration": _component("great")}),
            "the component-status vocabulary is closed",
        ),
        (
            "evidence_ok_without_expiry",
            mutate(evidence={**base["evidence"], "configuration": _component("ok")}),
            "an ok component MUST carry expires_at — freshness is not optional, "
            "so there is no such thing as evidence that never goes stale",
        ),
        (
            "checking_unknown_field",
            mutate(checking={**_checking(expires=300.0), "extra": 1}),
            "the checking field set is closed",
        ),
        (
            "checking_missing_run_id",
            mutate(checking={k: v for k, v in _checking(expires=300.0).items() if k != "run_id"}),
            "a required checking field is absent",
        ),
        (
            "marker_unknown_field",
            mutate(runtime_failure_marker={**_marker("local_server_unhealthy"), "extra": 1}),
            "the marker field set is closed",
        ),
        (
            "marker_unrecordable_reason",
            mutate(runtime_failure_marker=_marker("brain_check_in_progress")),
            "not every reason is recordable as a runtime failure — the ten "
            "projection-only reasons are the projection's answers, never a "
            "record's content",
        ),
        (
            "empty_evidence",
            mutate(evidence={name: None for name in bs.COMPONENT_ORDER}),
            "🔴 a record with no lane-applicable evidence is REFUSED, not read "
            "as unknown. A native writer that emits an empty evidence block on "
            "any path produces a record the next reader treats as corrupt",
        ),
        (
            "checking_present_without_checking_aggregate",
            mutate(
                runtime_failure_marker=_marker("provider_unavailable"),
                checking={
                    **_checking(expires=300.0),
                    "runtime_failure_marker_seen": "fedcba9876543210",
                },
            ),
            "a checking block and a checking aggregate are one fact in two "
            "fields: a current runtime-failure marker outranks the lease, so "
            "the reduced aggregate stops being `checking` and the record no "
            "longer validates",
        ),
        ("diagnostic_not_a_mapping", mutate(diagnostic=[]), "diagnostic is an object"),
        (
            "diagnostic_nested_value",
            mutate(diagnostic={"field": {"nested": 1}}),
            "diagnostic values are scalars",
        ),
    ]
    return cases


# --------------------------------------------------------------------------
# build
# --------------------------------------------------------------------------


def _project(
    record: Any,
    config: dict[str, Any],
    runtime: Any,
    permit: bool,
    *,
    hmac_key: bytes | None = CORPUS_HMAC_KEY,
) -> tuple[Any, str | None]:
    try:
        return (
            bs.project_brain_state(
                record,
                NOW,
                config=config,
                hmac_key=hmac_key,
                refresh_permit_active=permit,
                runtime_health=runtime,
            ),
            None,
        )
    except Exception as exc:  # recorded, never swallowed
        return None, f"{type(exc).__name__}: {exc}"


def _projection_cases() -> list[dict[str, Any]]:
    by_config = _records_by_config()
    cases: list[dict[str, Any]] = []

    # 1. Every record against its OWN config — the matched-fingerprint path,
    #    which is the only one that can reach `ready`. The runtime record is
    #    read on the bundled lane only, so it is crossed there and pinned once
    #    elsewhere.
    for config_name, records in sorted(by_config.items()):
        config = CONFIGS[config_name]
        runtimes = (
            RUNTIMES if config_name == "lane_bundled" else {"absent": RUNTIMES["absent"]}
        )
        for record_name, record in sorted(records.items()):
            for runtime_name, runtime in sorted(runtimes.items()):
                for permit in (False, True):
                    projection, raised = _project(record, config, runtime, permit)
                    cases.append(
                        {
                            "record": f"{config_name}/{record_name}",
                            "config": config_name,
                            "runtime_health": runtime_name,
                            "refresh_permit_active": permit,
                            "hmac_key_present": True,
                            "projection": projection,
                            "raised": raised,
                        }
                    )

    # 2. An absent record against every config — the missing-record path.
    for config_name, config in sorted(CONFIGS.items()):
        projection, raised = _project(None, config, None, False)
        cases.append(
            {
                "record": "absent",
                "config": config_name,
                "runtime_health": "absent",
                "refresh_permit_active": False,
                "hmac_key_present": True,
                "projection": projection,
                "raised": raised,
            }
        )

    # 3. Each lane's ready record against every OTHER config — the owner changed
    #    provider. This is the most common real transition and the reason
    #    `brain_config_changed` exists; it is also the path that must never
    #    answer `ready` from a record written for a different lane.
    for record_config, records in sorted(by_config.items()):
        for config_name, config in sorted(CONFIGS.items()):
            if config_name == record_config:
                continue
            projection, raised = _project(records["ready"], config, None, False)
            cases.append(
                {
                    "record": f"{record_config}/ready",
                    "config": config_name,
                    "runtime_health": "absent",
                    "refresh_permit_active": False,
                    "hmac_key_present": True,
                    "projection": projection,
                    "raised": raised,
                }
            )

    # 4. No fingerprint key. `health/brain-fingerprint.key` is 32 random bytes
    #    written once per journal, and the read path loads it WITHOUT creating
    #    it — so an absent or wrong-length key is a state a reader meets, not an
    #    error it causes. The answer is `fingerprint_key_unavailable`, and it is
    #    reachable by no other case here because every case above supplies a key.
    for config_name, records in sorted(by_config.items()):
        for record_name in ("ready", "checking_fresh"):
            projection, raised = _project(
                records[record_name],
                CONFIGS[config_name],
                None,
                False,
                hmac_key=None,
            )
            cases.append(
                {
                    "record": f"{config_name}/{record_name}",
                    "config": config_name,
                    "runtime_health": "absent",
                    "refresh_permit_active": False,
                    "hmac_key_present": False,
                    "projection": projection,
                    "raised": raised,
                }
            )
    return cases


def _validation_cases() -> list[dict[str, Any]]:
    cases: list[dict[str, Any]] = []
    for config_name, records in sorted(_records_by_config().items()):
        for name, record in sorted(records.items()):
            try:
                bs.validate_brain_state_record(record)
                cases.append(
                    {
                        "name": f"{config_name}/{name}",
                        "accepted": True,
                        "error": None,
                        "note": None,
                    }
                )
            except Exception as exc:
                cases.append(
                    {
                        "name": f"{config_name}/{name}",
                        "accepted": False,
                        "error": f"{type(exc).__name__}: {exc}",
                        "note": "a corpus record the validator refuses is a corpus defect",
                    }
                )
    for name, record, note in _malformed():
        try:
            bs.validate_brain_state_record(record)
            cases.append(
                {
                    "name": name,
                    "accepted": True,
                    "error": None,
                    "note": f"{note} — ACCEPTED, which is the finding",
                }
            )
        except Exception as exc:
            cases.append(
                {
                    "name": name,
                    "accepted": False,
                    "error": f"{type(exc).__name__}: {exc}",
                    "note": note,
                }
            )
    return cases


def build_brain_projection_fixture() -> dict[str, Any]:
    by_config = _records_by_config()
    return {
        "fixture": "solstone-brain-projection",
        "fixture_version": FIXTURE_VERSION,
        "generated_by": "make core-fixtures",
        "generator": "scripts/brain_projection_corpus.py",
        "reference_sources": list(REFERENCE_SOURCES),
        "reference_commit": reference_commit(),
        "now": _iso(0.0),
        "hmac_key_hex": CORPUS_HMAC_KEY.hex(),
        "bundled_runtime_fingerprint_sha256": BUNDLED_RUNTIME_SHA,
        "unrelated_fingerprint": FINGERPRINT_B,
        "configs": CONFIGS,
        "config_fingerprints": {
            name: _live_fingerprint(config) for name, config in sorted(CONFIGS.items())
        },
        "runtime_health": RUNTIMES,
        "records": {
            f"{config_name}/{name}": record
            for config_name, records in sorted(by_config.items())
            for name, record in sorted(records.items())
        },
        "malformed_records": {name: record for name, record, _ in _malformed()},
        "validation": _validation_cases(),
        "projection": _projection_cases(),
    }


if __name__ == "__main__":  # pragma: no cover - manual regeneration aid
    print(json.dumps(build_brain_projection_fixture(), indent=2, ensure_ascii=False))
