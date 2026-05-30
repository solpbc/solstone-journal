# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""python -m solstone.think.spl — entry point for `journal spl`."""

from solstone.think.spl.service import main

if __name__ == "__main__":
    raise SystemExit(main())
