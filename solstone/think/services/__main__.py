# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""python -m solstone.think.services — entry point for `sol services`."""

from solstone.think.services.cli import main

if __name__ == "__main__":
    raise SystemExit(main())
