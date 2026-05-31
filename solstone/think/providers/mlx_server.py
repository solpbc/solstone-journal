# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Launch mlx-vlm with a stable managed proctitle for orphan sweeping."""

from __future__ import annotations

import runpy
import sys

import setproctitle

MLX_SERVER_PROCESS_NAME = "mlx-vlm-server"


def main() -> None:
    setproctitle.setproctitle(MLX_SERVER_PROCESS_NAME)
    sys.argv = ["mlx_vlm.server", *sys.argv[1:]]
    runpy.run_module("mlx_vlm.server", run_name="__main__")


if __name__ == "__main__":
    main()
