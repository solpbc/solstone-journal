# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Contract guard for the surviving import model."""

from __future__ import annotations

import_contract = __import__("solstone.apps.import.contract", fromlist=["contract"])
_ACTION_SCHEMA = import_contract._ACTION_SCHEMA
_SAVE_RESPONSE_FIELDS = import_contract._SAVE_RESPONSE_FIELDS


def test_save_response_contract_keeps_action_enum_and_optional_in_progress():
    fields = {field.name: field for field in _SAVE_RESPONSE_FIELDS}

    assert set(_ACTION_SCHEMA["enum"]) == {"start", "do_not_start"}
    assert fields["in_progress"].type == "boolean"
    assert fields["in_progress"].required is False
