# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Command-line entry point for the journal device simulator."""

from __future__ import annotations

import argparse
import json
import math
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path

from .field_manifest import build_field_manifest
from .manifest import SCHEMA, ManifestError, load_manifest
from .process import LinkBridge, LinkProcessError
from .runner import RunOutcome, SimulationFailure, Simulator, SimulatorConfig


def _port(raw: str) -> int:
    try:
        value = int(raw)
    except ValueError as error:
        raise argparse.ArgumentTypeError(
            "must be an integer from 1 to 65535"
        ) from error
    if not 1 <= value <= 65535:
        raise argparse.ArgumentTypeError("must be an integer from 1 to 65535")
    return value


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

    pair = subparsers.add_parser(
        "pair", help="store an isolated native credential bundle for a later run"
    )
    pair.add_argument("--pair-code", required=True)
    pair.add_argument("--state-dir", type=Path, required=True)
    pair.add_argument("--solstone-bin", default="solstone")
    pair.add_argument("--convey-port", type=_port)
    pair.add_argument("--request-timeout", type=float, default=90.0)

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
    connection.add_argument(
        "--paired",
        action="store_true",
        help="use the credential bundle already stored under state-dir",
    )
    run.add_argument("--solstone-bin", default="solstone")
    run.add_argument("--relay-url")
    run.add_argument("--convey-port", type=_port)
    run.add_argument("--state-dir", type=Path)
    run.add_argument("--evidence", type=Path)
    run.add_argument("--date-mode", choices=("shift", "preserve"), default="shift")
    run.add_argument("--anchor-day")
    run.add_argument(
        "--journal-root",
        type=Path,
        help="optional read-only sandbox journal root for server-authored/derived output proof",
    )
    run.add_argument(
        "--expected-cid",
        help="authenticated sha256: device CID required for external-bridge white-box proof",
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
                "verification": profile.verification,
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


def _pair(args: argparse.Namespace) -> int:
    if not math.isfinite(args.request_timeout) or args.request_timeout <= 0:
        raise ManifestError("request_timeout must be a positive finite number")
    state_dir = args.state_dir.absolute()
    bridge = LinkBridge(
        solstone_bin=args.solstone_bin,
        pair_code=args.pair_code,
        state_dir=state_dir,
        carrier="direct",
        relay_url=None,
        convey_port=args.convey_port,
        startup_timeout=args.request_timeout,
    )
    bridge.ensure_paired()
    sys.stdout.write(
        json.dumps(
            {"paired": True, "state_dir": str(state_dir)},
            sort_keys=True,
        )
        + "\n"
    )
    return 0


def _run(args: argparse.Namespace) -> int:
    if args.paired and args.state_dir is None:
        raise ManifestError("--paired requires an explicit --state-dir")
    manifest = load_manifest(args.manifest, args.fixture_root)
    state_dir = (args.state_dir or _default_state_dir()).absolute()
    evidence_path = (args.evidence or (state_dir / "evidence.json")).absolute()
    simulator = Simulator(
        SimulatorConfig(
            manifest=manifest,
            profile=args.profile,
            carrier=args.carrier,
            state_dir=state_dir,
            evidence_path=evidence_path,
            bridge_url=args.bridge_url,
            pair_code=args.pair_code,
            paired=args.paired,
            solstone_bin=args.solstone_bin,
            relay_url=args.relay_url,
            convey_port=args.convey_port,
            date_mode=args.date_mode,
            anchor_day=args.anchor_day,
            journal_root=args.journal_root.resolve() if args.journal_root else None,
            expected_cid=args.expected_cid,
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
        if args.command == "pair":
            return _pair(args)
        return _run(args)
    except (LinkProcessError, ManifestError, SimulationFailure) as error:
        sys.stderr.write(f"configuration error: {error}\n")
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
