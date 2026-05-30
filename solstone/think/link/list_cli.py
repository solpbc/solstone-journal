# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Caller-side bundle enumeration for `sol link list`; reads peer bundles under the journal and, with `--observers`, observer bundles under the solstone-observer config tree, without network access or writes."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from solstone.convey.utils import relative_time
from solstone.think.link.observer_paths import observer_spl_root
from solstone.think.utils import get_journal

DASH = "—"
BULLET = "•"


@dataclass(frozen=True)
class _Bundle:
    kind: str
    label: str
    instance_id: str
    home_label: str | None
    paired_at: str | None
    fingerprint: str | None
    bundle_dir: str


def add_arguments(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--observers",
        action="store_true",
        default=False,
        help="Include observer bundles from the solstone-observer config tree",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        default=False,
        help="Output a JSON array",
    )


def main(args: argparse.Namespace) -> int:
    peers = _sorted_bundles(_walk_bundle_root(Path(get_journal()) / "peers", "peer"))
    observers = (
        _sorted_bundles(_walk_bundle_root(observer_spl_root(), "observer"))
        if args.observers
        else []
    )

    if args.json:
        records = _json_records(_sorted_bundles([*peers, *observers]))
        print(json.dumps(records))
        return 0

    _print_human(peers, observers, include_observers=args.observers)
    return 0


def _walk_bundle_root(root: Path, kind: str) -> list[_Bundle]:
    if not root.exists():
        return []
    if not root.is_dir():
        _warn(
            f"warning: {kind} bundle root {_absolute(root)} exists but is not a "
            "directory; treating as empty"
        )
        return []

    bundles: list[_Bundle] = []
    for bundle_dir in sorted(root.iterdir()):
        if not bundle_dir.is_dir():
            continue
        peer_json = bundle_dir / "peer.json"
        if not peer_json.exists():
            continue
        try:
            raw = peer_json.read_text(encoding="utf-8")
        except (OSError, UnicodeDecodeError) as exc:
            _warn(
                f"warning: skipping {kind} bundle {_absolute(bundle_dir)}: "
                f"cannot read peer.json ({type(exc).__name__})"
            )
            continue
        try:
            peer = json.loads(raw)
        except json.JSONDecodeError:
            _warn(
                f"warning: skipping {kind} bundle {_absolute(bundle_dir)}: "
                "peer.json is not valid JSON"
            )
            continue

        if not isinstance(peer, dict):
            _warn_missing_required(kind, bundle_dir, "label")
            continue
        label = _required_str(peer, "label")
        if label is None:
            _warn_missing_required(kind, bundle_dir, "label")
            continue
        instance_id = _required_str(peer, "instance_id")
        if instance_id is None:
            _warn_missing_required(kind, bundle_dir, "instance_id")
            continue

        bundles.append(
            _Bundle(
                kind=kind,
                label=label,
                instance_id=instance_id,
                home_label=_optional_str(peer, "home_label"),
                paired_at=_optional_str(peer, "paired_at"),
                fingerprint=_optional_str(peer, "fingerprint"),
                bundle_dir=_absolute(bundle_dir),
            )
        )
    return bundles


def _required_str(peer: dict[str, Any], field: str) -> str | None:
    value = peer.get(field)
    if not isinstance(value, str) or not value:
        return None
    return value


def _optional_str(peer: dict[str, Any], field: str) -> str | None:
    value = peer.get(field)
    if not isinstance(value, str) or not value:
        return None
    return value


def _warn_missing_required(kind: str, bundle_dir: Path, field: str) -> None:
    _warn(
        f"warning: skipping {kind} bundle {_absolute(bundle_dir)}: "
        f"peer.json missing required field '{field}'"
    )


def _warn(msg: str) -> None:
    sys.stderr.write(f"{msg}\n")


def _absolute(path: Path) -> str:
    return str(path.resolve())


def _relative_time(iso: str | None) -> str:
    if not iso:
        return "never"
    try:
        then = dt.datetime.strptime(iso, "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=dt.UTC)
    except ValueError:
        return iso
    now = dt.datetime.now(dt.UTC)
    delta_seconds = max(0, (now - then).total_seconds())
    return f"{relative_time(delta_seconds)} ago"


def _sort_key(bundle: _Bundle) -> tuple[int, float | str, str]:
    basename = Path(bundle.bundle_dir).name
    if bundle.paired_at:
        try:
            then = dt.datetime.strptime(bundle.paired_at, "%Y-%m-%dT%H:%M:%SZ").replace(
                tzinfo=dt.UTC
            )
        except ValueError:
            pass
        else:
            return (0, -then.timestamp(), basename)
    return (1, "", basename)


def _sorted_bundles(bundles: list[_Bundle]) -> list[_Bundle]:
    return sorted(bundles, key=_sort_key)


def _display(value: str | None) -> str:
    return value or DASH


def _print_human(
    peers: list[_Bundle],
    observers: list[_Bundle],
    *,
    include_observers: bool,
) -> None:
    if not include_observers:
        if not peers:
            print("No peers paired yet.")
            return
        _print_section("Peers", peers, "No peers paired yet.")
        return

    _print_section("Peers", peers, "No peers paired yet.")
    print()
    _print_section("Observers", observers, "No observers paired yet.")


def _print_section(heading: str, bundles: list[_Bundle], empty_message: str) -> None:
    print(f"{heading}:")
    if not bundles:
        print(f"  {empty_message}")
        return
    for bundle in bundles:
        print(f"  {bundle.label} ({bundle.instance_id})")
        print(
            f"    paired {_relative_time(bundle.paired_at)} {BULLET} "
            f"home: {_display(bundle.home_label)} {BULLET} "
            f"fingerprint: {_display(bundle.fingerprint)}"
        )


def _json_records(bundles: list[_Bundle]) -> list[dict[str, str | None]]:
    return [
        {
            "kind": bundle.kind,
            "label": bundle.label,
            "instance_id": bundle.instance_id,
            "home_label": bundle.home_label,
            "paired_at": bundle.paired_at,
            "fingerprint": bundle.fingerprint,
            "bundle_dir": bundle.bundle_dir,
        }
        for bundle in bundles
    ]
