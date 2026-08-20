# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Command-line entry point for the journal device simulator."""

from __future__ import annotations

import argparse
import json
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path

from .field_manifest import build_field_manifest
from .manifest import SCHEMA, ManifestError, load_manifest
from .runner import RunOutcome, SimulationFailure, Simulator, SimulatorConfig


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="python3 -m tools.journal_device_sim",
        description="Submit digest-pinned fixture segments through a real solstone link bridge.",
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    validate = subparsers.add_parser(
        "validate", help="validate and inventory a fixture manifest"
    )
    validate.add_argument("--manifest", type=Path, required=True)
    validate.add_argument("--fixture-root", type=Path)

    field_manifest = subparsers.add_parser(
        "field-manifest", help="build an exact simulator manifest from field_journal"
    )
    field_manifest.add_argument("--field-root", type=Path, required=True)
    field_manifest.add_argument("--output", type=Path, required=True)

    run = subparsers.add_parser("run", help="run one fixture profile")
    run.add_argument("--manifest", type=Path, required=True)
    run.add_argument("--fixture-root", type=Path)
    run.add_argument("--profile", required=True)
    run.add_argument("--carrier", choices=("direct", "relay"), required=True)
    connection = run.add_mutually_exclusive_group(required=True)
    connection.add_argument(
        "--bridge-url", help="already-running loopback solstone link bridge"
    )
    connection.add_argument(
        "--pair-code", help="pair-link URL; never written to state or evidence"
    )
    run.add_argument("--solstone-bin", default="solstone")
    run.add_argument("--relay-url")
    run.add_argument("--state-dir", type=Path)
    run.add_argument("--evidence", type=Path)
    run.add_argument("--date-mode", choices=("shift", "preserve"), default="shift")
    run.add_argument("--anchor-day")
    run.add_argument(
        "--journal-root",
        type=Path,
        help="optional read-only sandbox journal root for server-authored/derived output proof",
    )
    run.add_argument("--request-timeout", type=float, default=90.0)
    run.add_argument("--processing-timeout", type=float, default=0.0)
    run.add_argument("--poll-interval", type=float, default=1.0)
    run.add_argument("--max-attempts", type=int, default=3)
    run.add_argument(
        "--keep-credentials",
        action="store_true",
        help="retain isolated link credentials after a passing run for debugging",
    )
    return parser


def _default_state_dir() -> Path:
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return Path("scratch") / "journal-device-sim" / f"{stamp}-{uuid.uuid4().hex[:8]}"


def _validate(args: argparse.Namespace) -> int:
    manifest = load_manifest(args.manifest, args.fixture_root)
    payload = {
        "schema": SCHEMA,
        "manifest": str(manifest.path),
        "fixture_root": str(manifest.root),
        "sha256": manifest.digest,
        "segments": len(manifest.segments),
        "profiles": {
            name: {
                "segments": len(profile.segment_ids),
                "bytes": sum(
                    file.size
                    for segment_id in profile.segment_ids
                    for file in manifest.segments[segment_id].files
                ),
                "verify_duplicate": profile.verify_duplicate,
                "verify_processing": profile.verify_processing,
            }
            for name, profile in sorted(manifest.profiles.items())
        },
    }
    sys.stdout.write(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    return 0


def _field_manifest(args: argparse.Namespace) -> int:
    output = args.output.resolve()
    if output.exists():
        raise ManifestError(f"refusing to overwrite existing field manifest: {output}")
    value = build_field_manifest(args.field_root)
    output.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with output.open("x", encoding="utf-8") as handle:
        json.dump(value, handle, indent=2, sort_keys=True, ensure_ascii=True)
        handle.write("\n")
    sys.stdout.write(
        json.dumps(
            {
                "output": str(output),
                "segments": len(value["segments"]),
                "profiles": {
                    name: len(profile["segments"])
                    for name, profile in value["profiles"].items()
                },
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    )
    return 0


def _run(args: argparse.Namespace) -> int:
    manifest = load_manifest(args.manifest, args.fixture_root)
    state_dir = (args.state_dir or _default_state_dir()).resolve()
    evidence_path = (args.evidence or (state_dir / "evidence.json")).resolve()
    simulator = Simulator(
        SimulatorConfig(
            manifest=manifest,
            profile=args.profile,
            carrier=args.carrier,
            state_dir=state_dir,
            evidence_path=evidence_path,
            bridge_url=args.bridge_url,
            pair_code=args.pair_code,
            solstone_bin=args.solstone_bin,
            relay_url=args.relay_url,
            date_mode=args.date_mode,
            anchor_day=args.anchor_day,
            journal_root=args.journal_root.resolve() if args.journal_root else None,
            request_timeout=args.request_timeout,
            processing_timeout=args.processing_timeout,
            poll_interval=args.poll_interval,
            max_attempts=args.max_attempts,
            keep_credentials=args.keep_credentials,
        )
    )
    outcome = simulator.run()
    sys.stdout.write(f"{outcome.value} evidence={evidence_path}\n")
    return {
        RunOutcome.PASS: 0,
        RunOutcome.FAIL: 1,
        RunOutcome.BLOCKED: 2,
        RunOutcome.INCONCLUSIVE: 3,
    }[outcome]


def main(argv: list[str] | None = None) -> int:
    parser = _parser()
    args = parser.parse_args(argv)
    try:
        if args.command == "validate":
            return _validate(args)
        if args.command == "field-manifest":
            return _field_manifest(args)
        return _run(args)
    except (ManifestError, SimulationFailure) as error:
        sys.stderr.write(f"configuration error: {error}\n")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
