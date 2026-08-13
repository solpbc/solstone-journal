#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Conformance oracle for the settings READ surface.

Drives the reference Flask settings app and records what it actually answers, so
the native port's tests assert against captured reference output rather than
against a re-reading of the reference's source. Nothing here restates an
expectation.

⚠ Why the phases are shaped the way they are -- each one exists because a
narrower capture certified something false:

* `established` / `rich` -- `rich` seeds the sections a real journal carries,
  plus an UNKNOWN section (`some_future_section`). The reference's config
  projection is a deep copy with subtractions, not an allow-list, so it hands
  that section back verbatim. A port modelling the config as a typed struct
  drops it silently, and an owner's valid configuration stops round-tripping
  after an upgrade. That is the single most important fact in this file.
* `populated` -- created by DRIVING the write routes, then snapshotting the
  resulting on-disk journal into `_journal_tree`. Captured against an empty
  journal, `api/facets`, `api/facets?all=true` and `api/facets/muted` returned
  byte-identical bodies: three different questions with one answer, so a port
  that confused them matched. It also seeds three days of logs on BOTH log
  routes, because the paging contract had no captured case at all and any
  belief about it would have been recorded as though measured.
* `corrupt` -- a config that exists and will not parse. The reference answers
  500 in the owner's voice ("Your settings were NOT changed"), never the
  first-run wizard. A port written `unwrap_or_default()` tells an owner their
  journal was never set up, over an existing journal.
* `tokened` -- every other phase carries an EMPTY service token, so the `true`
  branch of the env mask was never measured and a port hardcoding `false`
  matched four of five phases.

⛔ CORRECTION, and it is why `summary.*` is not normalized. It was. `summary` is
the entire output of the retention walk, so a handler answering
`{total_segments: 0, raw_media_bytes: 0, ...}` matched every phase while an
owner's storage page would have read "0 segments, 0 bytes". A NORMALIZED FIELD
HAS BEEN DELETED FROM THE CONTRACT. The fix was not a better assertion -- it was
to stop normalizing and seed a real chronicle, because byte counts over a fixed
tree are deterministic. ⚠ A segment directory is named `HHMMSS_LEN` and nothing
else; an ISO-stamped name is silently not a segment and produced an all-zero
summary that read exactly like the surface reporting zero.

⚠ Normalization is BY FIELD PATH, never by value shape. A shape rule is what ate
another corpus's coverage window in this repository: a `^\\d{8}$` rule matched every `day` value,
so a port returning the wrong window would have matched. Each case records which
paths it normalized, and the digest is over the NORMALIZED body.

⛔ This repository is PUBLIC. Run this twice and diff before committing: the
capture is reproducible byte-for-byte, and anything that differs between two runs
is either volatile (normalize it by path) or host-identifying (do not commit it).
"""
from __future__ import annotations

import hashlib
import json
import os
import platform
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_OUTPUT = REPO_ROOT / "core" / "fixtures" / "convey_settings_corpus.json"


def _rev() -> str:
    """Name the rev this capture was taken at.

    A corpus that does not name its rev cannot be re-derived, and a prior capture in this
    repository measured a tree 291 commits stale without noticing.
    """
    override = os.environ.get("ORACLE_REV")
    if override:
        return override
    return subprocess.run(
        ["git", "-C", str(REPO_ROOT), "rev-parse", "HEAD"],
        capture_output=True, text=True, check=True,
    ).stdout.strip()


REV = _rev()

# Field paths whose value is volatile or host-dependent. The KEY is the exact
# dotted path; `*` matches one path segment. Value is the placeholder.
NORMALIZE = {
    "runtime_label": "<HOST:runtime_label>",
    "parakeet_uses_cpp": "<HOST:parakeet_uses_cpp>",
    "resource.*": "<HOST:resource>",
    "backends.*.available": "<HOST:backend_available>",
    "api_keys.*": "<HOST:api_key_present>",
    "runtime_env.*": "<HOST:runtime_env>",
    "identity.timezone": "<HOST:timezone>",
    "dashboard_url": "<HOST:dashboard_url>",
    "status_text": "<HOST:status_text>",
    # ⛔ `summary.*` is deliberately NOT normalized. It was, and that was the
    # single worst hole in this corpus: `summary` is the entire output of the
    # retention walk and the stream listing, so a port answering
    # {total_segments: 0, raw_media_bytes: 0, ...} matched every phase while an
    # owner's storage page read "0 segments, 0 bytes". Byte counts over a fixed
    # seeded chronicle are deterministic, so there is nothing volatile to erase.
    # `warnings` stays normalized: it reads real free disk space.
    "warnings": "<VOLATILE:storage_warnings>",
    "key_validation.*.timestamp": "<VOLATILE:timestamp>",
    # Action-log rows carry a wall-clock stamp. Enumerated as an exact path,
    # not as a `*.timestamp` suffix rule: a suffix rule is a shape rule wearing
    # a path rule's clothes, and shape rules are what ate another capture's coverage
    # window.
    "entries.*.timestamp": "<VOLATILE:log_timestamp>",
    "category_mute_state.*": "<VOLATILE:mute_state>",
}


def _match(path: str, pattern: str) -> bool:
    p, q = path.split("."), pattern.split(".")
    if len(p) != len(q):
        return False
    return all(b == "*" or a == b for a, b in zip(p, q))


JOURNAL_ROOT_PLACEHOLDER = "<JOURNAL_ROOT>"


def normalize(value, path="", hits=None):
    if hits is None:
        hits = []
    if isinstance(value, str):
        root = os.environ.get("SOLSTONE_JOURNAL")
        if root and root in value:
            hits.append(f"{path}#journal_root")
            return value.replace(root, JOURNAL_ROOT_PLACEHOLDER), hits
    for pattern, placeholder in NORMALIZE.items():
        if path and _match(path, pattern):
            hits.append(path)
            return placeholder, hits
    if isinstance(value, dict):
        out = {}
        for key in sorted(value):
            out[key], hits = normalize(value[key], f"{path}.{key}" if path else key, hits)
        return out, hits
    if isinstance(value, list):
        out = []
        for index, item in enumerate(value):
            normalized, hits = normalize(item, f"{path}.*" if path else "*", hits)
            out.append(normalized)
        return out, hits
    return value, hits


def digest(payload) -> str:
    return hashlib.sha256(
        json.dumps(payload, sort_keys=True, ensure_ascii=False).encode()
    ).hexdigest()[:16]


ESTABLISHED = {"setup": {"completed_at": 1700000000000}}

RICH = {
    "setup": {"completed_at": 1700000000000},
    "identity": {
        "name": "Ada Lovelace",
        "preferred": "Ada",
        "bio": "first programmer",
        "pronouns": {"subject": "she", "object": "her", "possessive": "her",
                     "reflexive": "herself"},
        "aliases": ["ada", "AAL"],
        "email_addresses": ["ada@example.org"],
        "timezone": "Europe/London",
    },
    "journal": {"name": "Analytical Engine"},
    "agent": {"name": "sol", "name_status": "named", "named_date": "2026-01-02"},
    "support": {"enabled": True, "proactive": False, "anonymous_feedback": True,
                "portal_url": "https://support.example.org"},
    "env": {"PLAUD_ACCESS_TOKEN": ""},
    "transcribe": {
        "backend": "parakeet",
        "preserve_all": True,
        "confidential_audio": False,
        "parakeet": {"model_version": "v3", "device": "auto", "timeout_sec": 120.0},
        # A retired backend subtree and an unknown scalar. The reference's
        # transcribe projection is a genuine allow-list, and with nothing
        # outside it seeded, a pure pass-through matched the corpus exactly --
        # so the one subtraction singled out as needing narrowness had zero
        # falsifying evidence behind it.
        "whisper": {"model": "large-v3"},
        "not_a_real_key": "should not survive projection",
    },
    "observe": {"tmux": {"enabled": False, "capture_interval": 17}},
    "describe": {
        "max_extractions": 42,
        "redact": ["password", "secret"],
        "categories": {},
    },
    "retention": {
        "raw_media": "days",
        "raw_media_days": 90,
        "per_stream": {"tmux": {"raw_media": "processed"}},
        "journal_logs": {"enabled": True, "days": 14},
    },
    "processing": {},
    "convey": {"secret": "MUST-NOT-LEAK", "password_hash": "MUST-NOT-LEAK",
               "password": "MUST-NOT-LEAK", "bind": "127.0.0.1"},
    "providers": {"active": {"provider": "unused"}},
    "service_key_validation": {"plaud": {"valid": False, "timestamp": "2026-01-01T00:00:00Z"},
                               "bogus": {"valid": True}},
    # An UNKNOWN section an older/newer journal could legitimately carry. The
    # reference must still hand it back; a port that drops it has narrowed the
    # contract and an owner loses settings on upgrade.
    "some_future_section": {"kept": True, "n": 7},
}

# Every GET on the surface, in route order.
READ_CASES = [
    ("api/state", "/app/settings/api/state"),
    ("api/config", "/app/settings/api/config"),
    ("api/convey/status", "/app/settings/api/convey/status"),
    ("api/transcribe", "/app/settings/api/transcribe"),
    ("api/processing", "/app/settings/api/processing"),
    ("api/sol_voice", "/app/settings/api/sol_voice"),
    ("api/sol_voice/throttled", "/app/settings/api/sol_voice/throttled"),
    ("api/chat", "/app/settings/api/chat"),
    ("api/validate-keys", "/app/settings/api/validate-keys"),
    ("api/vision", "/app/settings/api/vision"),
    ("api/observe", "/app/settings/api/observe"),
    ("api/facets", "/app/settings/api/facets"),
    ("api/facets?all=true", "/app/settings/api/facets?all=true"),
    ("api/facets/muted", "/app/settings/api/facets/muted"),
    ("api/icons?q=sett", "/app/settings/api/icons?q=sett&limit=5"),
    ("api/logs", "/app/settings/api/logs"),
    ("api/activities/defaults", "/app/settings/api/activities/defaults"),
    ("api/sync", "/app/settings/api/sync"),
    ("api/storage", "/app/settings/api/storage"),
    ("api/facet/absent", "/app/settings/api/facet/no-such-facet"),
    ("api/facet/absent/logs", "/app/settings/api/facet/no-such-facet/logs"),
    ("api/facet/absent/activities", "/app/settings/api/facet/no-such-facet/activities"),
]

# Refusal cases: these pin the ERROR contract, which is the half a port most
# often narrows. Each is (name, method, url, json-body-or-None).
REFUSAL_CASES = [
    ("config.no-body", "POST", "/app/settings/api/config", None),
    ("config.no-section", "POST", "/app/settings/api/config", {"data": {}}),
    ("config.unknown-section", "POST", "/app/settings/api/config",
     {"section": "nope", "data": {}}),
    ("config.empty-journal-name", "POST", "/app/settings/api/config",
     {"section": "journal", "data": {"name": "   "}}),
    ("config.bad-backend", "POST", "/app/settings/api/config",
     {"section": "transcribe", "data": {"backend": "not-a-backend"}}),
    ("config.non-bool-preserve", "POST", "/app/settings/api/config",
     {"section": "transcribe", "data": {"preserve_all": "yes"}}),
    ("observe.non-object-tmux", "POST", "/app/settings/api/observe", {"tmux": 5}),
    ("observe.interval-out-of-range", "POST", "/app/settings/api/observe",
     {"tmux": {"capture_interval": 999}}),
    ("observe.no-body", "POST", "/app/settings/api/observe", None),
    ("vision.max-extractions-low", "PUT", "/app/settings/api/vision",
     {"max_extractions": 1}),
    ("vision.redact-not-list", "PUT", "/app/settings/api/vision", {"redact": "x"}),
    ("vision.unknown-category", "PUT", "/app/settings/api/vision",
     {"categories": {"no_such_category": {"importance": "high"}}}),
    ("vision.bad-importance", "PUT", "/app/settings/api/vision",
     {"categories": {"__REAL_CATEGORY__": {"importance": "urgent"}}}),
    ("storage.bad-mode", "PUT", "/app/settings/api/storage", {"raw_media": "sometimes"}),
    ("storage.bad-days", "PUT", "/app/settings/api/storage", {"raw_media_days": 0}),
    ("storage.logs-bad-days", "PUT", "/app/settings/api/storage",
     {"journal_logs": {"days": True}}),
    ("sync.non-object", "PUT", "/app/settings/api/sync", {"plaud": 1}),
    ("sync.non-bool", "PUT", "/app/settings/api/sync", {"plaud": {"enabled": "yes"}}),
    ("facet.no-title", "POST", "/app/settings/api/facet", {"emoji": "x"}),
    ("facet.numeric-title", "POST", "/app/settings/api/facet", {"title": "123"}),
    ("facet.absent-update", "PUT", "/app/settings/api/facet/no-such", {"title": "x"}),
    ("facet.delete-no-consent", "DELETE", "/app/settings/api/facet/no-such", {}),
    ("facet.delete-false-consent", "DELETE", "/app/settings/api/facet/no-such",
     {"consent": False}),
    ("facet.rename-no-name", "POST", "/app/settings/api/facet/no-such/rename", {}),
    ("chat.bad-thinking-surfaces", "PUT", "/app/settings/api/chat",
     {"thinking_surfaces": "sideways"}),
    ("sol_voice.not-object", "PUT", "/app/settings/api/sol_voice", ["nope"]),
    ("prune-logs.bad-days", "POST", "/app/settings/api/storage/prune-logs", {"days": 0}),
]


def seed_content(client) -> list:
    """Populate facets, activities and action logs by DRIVING the surface.

    An empty journal proves almost nothing for a read surface.
    Captured with no facets, `api/facets`, `api/facets?all=true` and
    `api/facets/muted` returned byte-identical bodies -- three different
    questions with one answer, so a port that confused them would have matched.
    """
    seeded = []
    for title, emoji, icon in (("Work Life", "\U0001f4bc", "briefcase"),
                               ("Zeta Project", "\U0001f9ea", "flask-conical"),
                               ("Muted Thing", "\U0001f507", "")):
        response = client.post("/app/settings/api/facet",
                               json={"title": title, "emoji": emoji, "icon": icon,
                                     "color": "#334455", "description": f"{title} desc",
                                     "consent": True})
        seeded.append((title, response.status_code, response.get_json(silent=True)))
    client.put("/app/settings/api/facet/muted-thing", json={"muted": True})
    client.post("/app/settings/api/facet/work-life/activities",
                json={"name": "Deep Work", "description": "focus block",
                      "priority": "high", "icon": "target"})
    client.post("/app/settings/api/facet/work-life/activities",
                json={"name": "Standup", "priority": "low", "emoji": "\U0001f5e3"})
    # An ordinary config write, so the journal action log has real rows.
    client.post("/app/settings/api/config",
                json={"section": "identity", "data": {"preferred": "Countess"}})
    return seeded


def seed_multiday_logs(root: Path) -> None:
    """Write three days of action logs so the cursor contract is measurable.

    The reference's paging behaviour -- newest day first, entries reversed
    within a day, `?cursor=` loading the day STRICTLY before -- had no captured
    case at all: the populated phase produced exactly one day with one entry, so
    any belief about paging would have been recorded as if it were measured.
    """
    for actions in (root / "config" / "actions",
                    root / "facets" / "work-life" / "logs"):
        _write_day_logs(actions)


def _write_day_logs(actions: Path) -> None:
    actions.mkdir(parents=True, exist_ok=True)
    for day, count in (("20260810", 2), ("20260811", 3), ("20260812", 1)):
        rows = [
            {"timestamp": f"{day[:4]}-{day[4:6]}-{day[6:]}T0{index}:00:00Z",
             "actor": "settings", "action": f"probe_{index}", "source": "app",
             "params": {"seq": index}}
            for index in range(count)
        ]
        (actions / f"{day}.jsonl").write_text(
            "\n".join(json.dumps(row) for row in rows) + "\n", encoding="utf-8"
        )
    # A non-.jsonl sibling and a non-8-digit name: neither may be paged into.
    (actions / "notes.txt").write_text("ignore me\n", encoding="utf-8")
    (actions / "2026081.jsonl").write_text("{}\n", encoding="utf-8")


def seed_chronicle(root: Path) -> None:
    """Write real segment content so `api/storage` has something to count.

    Without this, every phase answered `summary: {total_segments: 0, ...}` and a
    handler that returned zeros unconditionally matched the whole corpus. Sizes
    are fixed byte counts so the summary is deterministic.

    Shapes chosen to exercise all three of the reference's segment classes:
      * a segment WITH raw media          -> segments_with_raw
      * a segment whose raw media is gone but which kept its derived jsonl
                                          -> segments_purged
      * a segment with neither            -> counted in total only
    """
    # ⚠ A segment directory is named `HHMMSS_LEN` and nothing else — an
    # ISO-stamped name is silently not a segment, so the first version of this
    # seeder produced a storage summary of all zeros that read exactly like the
    # surface reporting zero. The name is the instrument here.
    chronicle = root / "chronicle"
    with_raw = chronicle / "20260810" / "tmux" / "090000_300"
    purged = chronicle / "20260810" / "tmux" / "100000_300"
    bare = chronicle / "20260811" / "screen" / "090000_120"
    for path in (with_raw, purged, bare):
        path.mkdir(parents=True, exist_ok=True)
    # Raw media (counted into raw_media_bytes, and .flac is a registry format).
    (with_raw / "audio.flac").write_bytes(b"\x00" * 4096)
    (with_raw / "monitor_1_diff.png").write_bytes(b"\x00" * 2048)
    # Derived sidecars (counted into derived_bytes).
    (with_raw / "audio.jsonl").write_text('{"seeded": true}\n', encoding="utf-8")
    (purged / "audio.jsonl").write_text('{"seeded": true}\n', encoding="utf-8")
    (bare / "notes.md").write_text("seeded\n", encoding="utf-8")
    # Stream records, so `api/storage.streams` is non-empty.
    streams = root / "streams"
    streams.mkdir(parents=True, exist_ok=True)
    for name in ("tmux", "screen"):
        (streams / f"{name}.json").write_text(
            json.dumps({"name": name, "seq": 1}, sort_keys=True) + "\n",
            encoding="utf-8",
        )


def snapshot_tree(root: Path) -> dict:
    """Record the on-disk journal the REFERENCE wrote.

    A read-wave port cannot re-derive this layout without the write routes, and
    hand-rolling an approximation of it would make the conformance test measure
    the fixture rather than the handler (VPE operating principle 15d). So the
    reference's own bytes travel with the corpus.
    """
    tree = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        rel = str(path.relative_to(root))
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            tree[rel] = "<BINARY>"
            continue
        # Two enumerated log locations: the journal-wide action log and the
        # per-facet logs. Both carry a wall-clock `timestamp` on every row.
        is_action_log = rel.startswith("config/actions/") or (
            rel.startswith("facets/") and "/logs/" in rel
        )
        if is_action_log and rel.endswith(".jsonl"):
            # Enumerated field path, not a value-shape match: the top-level
            # `timestamp` of an action-log row is wall-clock and would otherwise
            # make this snapshot irreproducible.
            lines = []
            for line in text.splitlines():
                if not line.strip():
                    continue
                try:
                    row = json.loads(line)
                except json.JSONDecodeError:
                    lines.append(line)
                    continue
                if isinstance(row, dict) and "timestamp" in row:
                    row["timestamp"] = "<VOLATILE:log_timestamp>"
                lines.append(json.dumps(row, sort_keys=True))
            text = "\n".join(lines) + "\n"
        tree[rel] = text
    return tree


def run_phase(name: str, config: dict, *, seed: bool = False,
              corrupt: bool = False) -> dict:
    root = Path(tempfile.mkdtemp(prefix=f"oracle-{name}-"))
    (root / "config").mkdir(parents=True)
    if corrupt:
        # P-journal-config house style: a config that EXISTS and will not parse
        # must raise in the owner's voice, never be substituted with defaults.
        (root / "config" / "journal.json").write_text(
            '{"identity": {"name": "Ada",\n', encoding="utf-8"
        )
    else:
        (root / "config" / "journal.json").write_text(
            json.dumps(config, indent=2) + "\n", encoding="utf-8"
        )
    os.environ["SOLSTONE_JOURNAL"] = str(root)
    os.environ["SOL_SKIP_SUPERVISOR_CHECK"] = "1"

    from solstone.convey import create_app

    app = create_app(str(root))
    app.config["TESTING"] = True
    client = app.test_client()

    # Per-phase copy: appending to the module-level list would leak one phase's
    # extra cases into every later phase.
    read_cases = list(READ_CASES)
    if name == "tokened":
        # `api/validate-keys` calls the live Plaud validator when a token is
        # present. An oracle must not make a third-party network call: it would
        # be irreproducible and it is not ours to send. The token branch of THIS
        # route stays unmeasured, deliberately and on the record.
        read_cases = [case for case in read_cases if case[0] != "api/validate-keys"]

    seeded = None
    if seed:
        seeded = seed_content(client)
        seed_multiday_logs(root)
        seed_chronicle(root)
        read_cases.extend([
            ("api/facet/work-life/logs?cursor=20260814",
             "/app/settings/api/facet/work-life/logs?cursor=20260814"),
            ("api/facet/work-life/logs?cursor=20260812",
             "/app/settings/api/facet/work-life/logs?cursor=20260812"),
            ("api/facet/work-life/logs?cursor=20260810",
             "/app/settings/api/facet/work-life/logs?cursor=20260810"),
            ("api/logs?cursor=20260812", "/app/settings/api/logs?cursor=20260812"),
            ("api/logs?cursor=20260811", "/app/settings/api/logs?cursor=20260811"),
            ("api/logs?cursor=20260810", "/app/settings/api/logs?cursor=20260810"),
            ("api/logs?cursor=20260899", "/app/settings/api/logs?cursor=20260899"),
        ])

    # Resolve one real category name so the bad-importance case is about
    # importance and not about the category being unknown.
    real_category = None
    probe = client.get("/app/settings/api/vision")
    if probe.status_code == 200:
        defaults = probe.get_json().get("category_defaults") or {}
        real_category = next(iter(sorted(defaults)), None)

    cases = {}
    if seeded is not None:
        cases["_seeded"] = {"created": seeded}
        for extra in ("work-life", "zeta-project", "muted-thing"):
            read_cases.append((f"api/facet/{extra}", f"/app/settings/api/facet/{extra}"))
            read_cases.append((f"api/facet/{extra}/activities",
                               f"/app/settings/api/facet/{extra}/activities"))
            read_cases.append((f"api/facet/{extra}/logs",
                               f"/app/settings/api/facet/{extra}/logs"))
    for case_name, url in read_cases:
        response = client.get(url)
        body = response.get_json(silent=True)
        normalized, hits = normalize(body) if body is not None else (None, [])
        cases[f"GET {case_name}"] = {
            "status": response.status_code,
            "content_type": response.headers.get("Content-Type"),
            "normalized": normalized,
            "normalized_paths": sorted(set(hits)),
            "digest": digest(normalized),
        }

    for case_name, method, url, payload in REFUSAL_CASES:
        sent = json.loads(
            json.dumps(payload).replace("__REAL_CATEGORY__", real_category or "unknown")
        ) if payload is not None else None
        response = client.open(method=method, path=url, json=sent) if sent is not None \
            else client.open(method=method, path=url)
        body = response.get_json(silent=True)
        normalized, hits = normalize(body) if body is not None else (None, [])
        cases[f"{method} {case_name}"] = {
            "status": response.status_code,
            "sent": sent,
            "normalized": normalized,
            "normalized_paths": sorted(set(hits)),
            "digest": digest(normalized),
        }

    if seed:
        cases["_journal_tree"] = {"files": snapshot_tree(root)}
    return cases


def main() -> int:
    out = {
        "rev": REV,
        "captured_by": "driving the reference; no value here is a restated expectation",
        "host": {"system": platform.system(), "machine": platform.machine()},
        "phases": {},
    }
    out["phases"]["established"] = run_phase("established", ESTABLISHED)
    out["phases"]["rich"] = run_phase("rich", RICH)
    out["phases"]["populated"] = run_phase("populated", RICH, seed=True)
    out["phases"]["corrupt"] = run_phase("corrupt", {}, corrupt=True)
    # Every phase above carries an EMPTY PLAUD token, so the `true` branch of
    # the env mask and the populated branch of key_validation were never
    # measured -- a port could hardcode `false` and match all of them.
    tokened = json.loads(json.dumps(RICH))
    tokened["env"]["PLAUD_ACCESS_TOKEN"] = "plaud-token-MUST-NOT-LEAK"
    out["phases"]["tokened"] = run_phase("tokened", tokened)
    target = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_OUTPUT
    target.write_text(json.dumps(out, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    total = sum(len(phase) for phase in out["phases"].values())
    print(f"captured {total} cases across {len(out['phases'])} phases -> {target}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
