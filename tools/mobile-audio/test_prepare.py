#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
import copy
import json
from pathlib import Path
import unittest
from prepare import verify_audio_pins


class AudioPins(unittest.TestCase):
    def test_unrelated_engine_update_does_not_admit_changed_audio(self):
        # Relevant source-pin projection of a captured iOS native build receipt.
        captured = json.loads((Path(__file__).parent / 'tests/fixtures/retained-audio-pins.json').read_text())
        current = copy.deepcopy(captured['inputs'])
        current['llama'] = {'revision': 'unrelated-local-thinking-engine'}
        verify_audio_pins(captured, current)
        unrelated = copy.deepcopy(current)
        unrelated['llama']['revision'] = 'different-local-thinking-engine'
        verify_audio_pins(captured, unrelated)
        for name in captured['inputs']:
            for field in ('url', 'revision', 'sha256'):
                with self.subTest(name=name, field=field):
                    changed = copy.deepcopy(current)
                    changed[name][field] = 'changed-audio-source'
                    with self.assertRaises(AssertionError):
                        verify_audio_pins(captured, changed)


if __name__ == '__main__':
    unittest.main()
