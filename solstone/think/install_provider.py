# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Top-level `journal install-provider <name>` — install a local provider runtime.

Local-system-only: only meaningful on the host that stores the journal. Moved
here from the old journal-access provider-install surface.
"""

from __future__ import annotations

import argparse
import json
import sys

from solstone.think.providers import local_install
from solstone.think.utils import require_solstone


def main() -> int:
    parser = argparse.ArgumentParser(
        prog="journal install-provider",
        description="Install or retry the local provider runtime.",
    )
    parser.add_argument("name", help="Provider name (only 'local' is supported).")
    args = parser.parse_args()

    require_solstone()

    if args.name != "local":
        print(
            f"unsupported provider {args.name!r}; only 'local' is supported; "
            "cogitate runs baseline for hosted providers",
            file=sys.stderr,
        )
        return 2

    print(json.dumps(local_install.install_local(), indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
