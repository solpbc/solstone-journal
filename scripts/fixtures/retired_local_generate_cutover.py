# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc


def run_generate(endpoint):
    if endpoint.is_bundled:
        return resolve_context_window()
