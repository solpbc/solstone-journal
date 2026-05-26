# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI for setting the convey web UI password.

Usage:
    journal password set    Set the convey password
    journal password reset  Reset the convey password (alias for set)
"""

from __future__ import annotations

import argparse
import getpass
import json
import os
import sys
from pathlib import Path

from werkzeug.security import generate_password_hash

from solstone.think.utils import get_config, get_journal, require_solstone, setup_cli


def _set_password() -> None:
    """Prompt for a password, hash it, and write to journal config."""
    password = getpass.getpass("Password: ")
    confirm = getpass.getpass("Confirm password: ")

    if password != confirm:
        print("Passwords do not match.", file=sys.stderr)
        sys.exit(1)

    password_hash = generate_password_hash(password)

    config = get_config()
    config.setdefault("convey", {})["password_hash"] = password_hash
    config.get("convey", {}).pop("password", None)

    config_path = Path(get_journal()) / "config" / "journal.json"
    config_path.parent.mkdir(parents=True, exist_ok=True)
    with open(config_path, "w", encoding="utf-8") as f:
        json.dump(config, f, indent=2, ensure_ascii=False)
        f.write("\n")
    os.chmod(config_path, 0o600)

    print("Password set successfully.")


def main() -> None:
    parser = argparse.ArgumentParser(description="Manage convey web UI password")
    subparsers = parser.add_subparsers(dest="subcommand")
    subparsers.add_parser("set", help="Set the convey password")
    subparsers.add_parser("reset", help="Reset the convey password")

    args = setup_cli(parser)
    require_solstone()

    if args.subcommand in ("set", "reset"):
        _set_password()
    else:
        parser.print_help()
        sys.exit(1)


if __name__ == "__main__":
    main()
