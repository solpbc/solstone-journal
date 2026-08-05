# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Private one-record, read-only RF-DETR install-state query for native helpers."""

from __future__ import annotations

import json

from solstone.think.providers.rfdetr_install import rfdetr_paths


def main() -> None:
    paths = rfdetr_paths()
    print(
        json.dumps(
            {
                "status": paths.status,
                "binary_path": str(paths.binary_path) if paths.binary_path else None,
                "model_path": str(paths.model_path) if paths.model_path else None,
            }
        )
    )


if __name__ == "__main__":
    main()
