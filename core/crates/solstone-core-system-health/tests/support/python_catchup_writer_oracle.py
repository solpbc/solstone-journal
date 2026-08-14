# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
import os
import sys

sys.path.insert(0, os.environ["SOLSTONE_REPO_ROOT"])

from solstone.think.catchup_state import (
    record_daily_catchup_progress,
    record_segment_repair_attempt,
    record_segment_repair_outcome,
)

day = os.environ["ORACLE_DAY"]
record_daily_catchup_progress(day, cleared=1, remaining=2)
record_segment_repair_attempt(day, started_at=1.0)
record_segment_repair_outcome(
    day,
    success=False,
    timed_out=True,
    timeout_seconds=3.0,
    ended_at=4.0,
    cleared=1,
    remaining=2,
)
with open(os.path.join(os.environ["SOLSTONE_JOURNAL"], "health", "catchup-state.json"), encoding="utf-8") as state:
    print(json.dumps(json.load(state), sort_keys=True))
