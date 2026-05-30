# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Caller-side `sol link` command namespace."""

from __future__ import annotations

import argparse
import sys

from solstone.think.link import join_cli, list_cli, serve_cli


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="solstone link access commands")
    subparsers = parser.add_subparsers(
        dest="command",
        metavar="{join,list,serve}",
        title="commands",
    )
    join_parser = subparsers.add_parser(
        "join",
        help="join a solstone with a short code or pair link",
    )
    join_cli.add_arguments(join_parser)
    list_parser = subparsers.add_parser(
        "list",
        help="list caller-side link bundles",
    )
    list_cli.add_arguments(list_parser)
    serve_parser = subparsers.add_parser(
        "serve",
        help="serve a loopback proxy over a link tunnel",
    )
    serve_cli.add_arguments(serve_parser)
    return parser


def main(argv: list[str] | None = None) -> int:
    """CLI entry point for `sol link`."""
    parser = _build_parser()
    args = parser.parse_args(list(sys.argv[1:] if argv is None else argv))
    if args.command is None:
        parser.print_help()
        return 0
    if args.command == "join":
        return join_cli.main(args)
    if args.command == "list":
        return list_cli.main(args)
    if args.command == "serve":
        return serve_cli.main(args)
    return 0
