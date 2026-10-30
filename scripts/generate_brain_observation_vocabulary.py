#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Generate the closed observation vocabulary from its Rust/Brain authorities."""
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
source = (ROOT / 'core/crates/solstone-core-brain/src/observation.rs').read_text()
rows = ['# Generated from Brain observation enums and core/fixtures/local_contract.json.',
        '# Regenerate: python3 scripts/generate_brain_observation_vocabulary.py',
        '# Do not edit values here; change the owning authority.']
for field, name in [('caller', 'ObservationCaller'), ('phase', 'ObservationPhase'),
                    ('outcome', 'ObservationOutcome'), ('failure_category', 'ObservationFailure')]:
    body = source.split('pub enum ' + name + ' {', 1)[1].split('\n}', 1)[0]
    for line in body.splitlines():
        line = line.strip()
        if not line:
            continue
        if not re.fullmatch(r'[A-Z][A-Za-z0-9]*,', line):
            raise ValueError('unsupported enum shape: ' + name)
        value = re.sub(r'(?<!^)([A-Z])', r'_\1', line[:-1]).lower()
        rows.append(field + '\t' + value)
contract = json.loads((ROOT / 'core/fixtures/local_contract.json').read_text())['brain_state']
for value in contract['reason_codes']:
    rows.append('reason_code\t' + value)
for value in sorted({v for s in contract['diagnostic_metadata_schemas'].values()
                     for v in s.get('verification_reason', [])}):
    rows.append('verification_reason\t' + value)
(ROOT / 'core/fixtures/brain_observation_vocabulary.tsv').write_text('\n'.join(rows) + '\n')
