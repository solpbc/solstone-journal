#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from prepare import main, verify_audio_pins, sound_ced_input, retained_onnx_input, digest, stage_copy


def legacy_receipt():
    return json.loads((Path(__file__).parent / 'tests/fixtures/retained-audio-pins.json').read_text())


def shared_receipt():
    captured = legacy_receipt()
    repo = Path(__file__).resolve().parents[2]
    loader = 'core/distribution/mobile/parakeet-mapped-weights.patch'
    guard = 'core/distribution/mobile/ggml-metal-mapped-buffer-null.patch'
    failure = 'core/distribution/mobile/parakeet-graph-failure.patch'
    weights = 'core/distribution/mobile/parakeet-weight-failure.patch'
    loading = 'core/distribution/mobile/parakeet-load-lifecycle.patch'
    captured['patches'] = [dict(component='parakeet', path=name, sha256=digest(repo / name))
                           for name in (loader, failure, weights, loading)]
    current = copy.deepcopy(captured['inputs'])
    pins = json.loads((Path(__file__).resolve().parents[2] /
                       'core/distribution/mobile/inputs.json').read_text())['inputs']
    current['rfdetr'] = copy.deepcopy(pins['rfdetr'])
    captured['inputs']['rfdetr'] = copy.deepcopy(current['rfdetr'])
    # Synthetic binding tests the receipt contract; no native inference is claimed.
    binding = {
        'schema': 'solstone.mobile-media-ggml.v1',
        'sources': {name: current[name]['sha256'] for name in ('ggml', 'parakeet', 'rfdetr')},
        'tree_sha256': 'a' * 64,
        'patches': [
            {'source_component': 'parakeet', 'path': 'third_party/ggml-patches/audio.patch',
             'sha256': 'b' * 64},
            {'source_component': 'rfdetr', 'path': 'third_party/ggml-patches/detector.patch',
             'sha256': 'c' * 64},
            {'source_component': 'mobile', 'path': guard, 'sha256': digest(repo / guard)},
        ],
    }
    captured['inputs']['media-shared-ggml'] = binding
    captured['inputs']['parakeet-patched-ggml'] = {'tree_sha256': binding['tree_sha256']}
    return captured, current


class AudioPins(unittest.TestCase):
    def test_pinned_onnx_reuse_binds_original_producer_and_retained_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'receipts').mkdir()
            library = root / 'libonnxruntime.a'
            library.write_bytes(b'ONNX retention contract fixture; not target binary proof')
            checksum = digest(library)
            artifact = dict(component='onnx-runtime', path='/original/work/libonnxruntime.a',
                            sha256=checksum, size_bytes=library.stat().st_size)
            receipt = dict(status='passed', exit=0, platform='ios', source_commit='original-source',
                           artifacts=[artifact])
            kept = dict(artifact, retained_path=str(library))
            receipt_path = root / 'receipts/mobile-native-build.json'
            retention_path = root / 'retention.json'
            def save(producer, retained):
                receipt_path.write_text(json.dumps(producer))
                retention_path.write_text(json.dumps({'artifacts': retained}))
            save(receipt, [kept])
            source, binding = retained_onnx_input(root, checksum)
            self.assertEqual(source, library)
            self.assertEqual(binding['source_commit'], 'original-source')
            self.assertEqual(binding['reused_from'], str(root))
            self.assertEqual(binding['producer_receipt_sha256'], digest(receipt_path))
            mutations = [
                lambda r, k: r.update(status='failed'),
                lambda r, k: r.update(exit=2),
                lambda r, k: r.update(platform='android'),
                lambda r, k: r.update(artifacts=[]),
                lambda r, k: r['artifacts'].append(copy.deepcopy(r['artifacts'][0])),
                lambda r, k: k[0].update(path='/different/work/libonnxruntime.a'),
                lambda r, k: k[0].update(sha256='0' * 64),
                lambda r, k: k[0].update(size_bytes=1),
            ]
            for mutate in mutations:
                producer, retained = copy.deepcopy(receipt), [copy.deepcopy(kept)]
                mutate(producer, retained)
                save(producer, retained)
                with self.assertRaises(ValueError): retained_onnx_input(root, checksum)
            save(receipt, [kept])
            with self.assertRaises(ValueError): retained_onnx_input(root, '0' * 64)
            alias = root / 'renamed.a'
            alias.write_bytes(library.read_bytes())
            save(receipt, [dict(kept, retained_path=str(alias))])
            with self.assertRaises(ValueError): retained_onnx_input(root, checksum)
            link_directory = root / 'linked'
            link_directory.mkdir()
            linked = link_directory / library.name
            linked.symlink_to(library)
            save(receipt, [dict(kept, retained_path=str(linked))])
            with self.assertRaises(ValueError): retained_onnx_input(root, checksum)
            save(receipt, [kept])
            library.write_bytes(b'tampered retained binary')
            with self.assertRaises(ValueError): retained_onnx_input(root, checksum)

    def test_staging_refuses_links_before_overwriting_their_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'input'
            source.write_bytes(b'verified model')
            victim = root / 'retained'
            victim.write_bytes(b'preserved input')
            destination = root / 'model.gguf'
            destination.symlink_to(victim)
            with self.assertRaises(ValueError): stage_copy(source, destination)
            self.assertEqual(victim.read_bytes(), b'preserved input')
            linked = root / 'models'
            linked.symlink_to(root, target_is_directory=True)
            with self.assertRaises(ValueError): stage_copy(source, linked / 'retained')
            self.assertEqual(victim.read_bytes(), b'preserved input')
            regular = root / 'model-copy.gguf'
            stage_copy(source, regular)
            self.assertEqual(regular.read_bytes(), source.read_bytes())

    def test_isolated_sound_ced_binds_producer_source_wrapper_and_shared_tree(self):
        repo = Path(__file__).resolve().parents[2]
        pins = json.loads((repo / 'core/distribution/mobile/inputs.json').read_text())['inputs']
        captured, _ = shared_receipt()
        shared = captured['inputs']['media-shared-ggml']
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'receipts').mkdir()
            library = root / 'libsolstone_sound_ced.a'
            library.write_bytes(b'archive receipt fixture; not a native proof')
            artifact = dict(component='sound-ced-archive-0', path=str(library),
                            sha256=digest(library), size_bytes=library.stat().st_size)
            receipt = dict(status='passed', exit=0, platform='ios',
                driver_sha256=digest(repo / 'scripts/mobile_native_build.py'), artifacts=[artifact],
                inputs={'ced': pins['ced'], 'media-shared-ggml': shared,
                    'sound-ced-patched-ggml': {'tree_sha256': shared['tree_sha256']},
                    'sound-ced-wrapper': {
                        'sha256': digest(repo / 'core/distribution/mobile/sound-ced/CMakeLists.txt'),
                        'source': 'ced', 'symbol_prefix': 'solstone_sound_ced',
                        'ggml_tree_sha256': shared['tree_sha256']}})
            record = root / 'receipts/mobile-native-build.json'
            record.write_text(json.dumps(receipt))
            (root / 'retention.json').write_text(json.dumps({'artifacts': [
                dict(component=artifact['component'], retained_path=str(library), sha256=artifact['sha256'])]}))
            source, binding = sound_ced_input(root, repo, pins, shared)
            self.assertEqual(source, library)
            self.assertEqual(binding['sha256'], artifact['sha256'])
            mutations = [
                lambda r: r.update(status='failed'),
                lambda r: r.update(driver_sha256='0' * 64),
                lambda r: r['inputs']['ced'].update(revision='changed'),
                lambda r: r['inputs']['sound-ced-wrapper'].update(symbol_prefix='ced'),
                lambda r: r['inputs']['media-shared-ggml'].update(tree_sha256='0' * 64),
                lambda r: r['artifacts'][0].update(sha256='0' * 64),
            ]
            for mutate in mutations:
                changed = copy.deepcopy(receipt)
                mutate(changed)
                record.write_text(json.dumps(changed))
                with self.assertRaises(ValueError): sound_ced_input(root, repo, pins, shared)
            record.write_text(json.dumps(receipt))
            library.write_bytes(b'changed bytes')
            with self.assertRaises(ValueError): sound_ced_input(root, repo, pins, shared)

    def test_unrelated_engine_update_does_not_admit_changed_audio(self):
        captured, current = shared_receipt()
        current['llama'] = {'revision': 'unrelated-local-thinking-engine'}
        verify_audio_pins(captured, current)
        unrelated = copy.deepcopy(current)
        unrelated['llama']['revision'] = 'different-local-thinking-engine'
        verify_audio_pins(captured, unrelated)
        for name in ('parakeet', 'parakeet-ced', 'parakeet-voice-detect', 'ggml', 'rfdetr'):
            for field in ('url', 'revision', 'sha256'):
                with self.subTest(name=name, field=field):
                    changed = copy.deepcopy(current)
                    changed[name][field] = 'changed-audio-source'
                    with self.assertRaises(ValueError):
                        verify_audio_pins(captured, changed)

    def test_unchecked_graph_receipt_is_refused(self):
        captured, current = shared_receipt()
        failure = 'core/distribution/mobile/parakeet-graph-failure.patch'
        for checksum in (None, '0' * 64):
            changed = copy.deepcopy(captured)
            changed['patches'] = [row for row in changed['patches'] if row['path'] != failure]
            if checksum is not None:
                changed['patches'].append(dict(component='parakeet', path=failure, sha256=checksum))
            with self.assertRaisesRegex(ValueError, 'checked Parakeet graph failures'):
                verify_audio_pins(changed, current)

    def test_unchecked_weight_preparation_receipt_is_refused(self):
        captured, current = shared_receipt()
        weights = 'core/distribution/mobile/parakeet-weight-failure.patch'
        for checksum in (None, '0' * 64):
            changed = copy.deepcopy(captured)
            changed['patches'] = [row for row in changed['patches'] if row['path'] != weights]
            if checksum is not None:
                changed['patches'].append(dict(component='parakeet', path=weights, sha256=checksum))
            with self.assertRaisesRegex(ValueError, 'checked Parakeet weight preparation'):
                verify_audio_pins(changed, current)

    def test_unowned_load_failure_receipt_is_refused(self):
        captured, current = shared_receipt()
        loading = 'core/distribution/mobile/parakeet-load-lifecycle.patch'
        for checksum in (None, '0' * 64):
            changed = copy.deepcopy(captured)
            changed['patches'] = [row for row in changed['patches'] if row['path'] != loading]
            if checksum is not None:
                changed['patches'].append(dict(component='parakeet', path=loading, sha256=checksum))
            with self.assertRaisesRegex(ValueError, 'owned Parakeet load failure cleanup'):
                verify_audio_pins(changed, current)

    def test_matching_shared_closure_is_returned_without_mutating_receipt(self):
        captured, current = shared_receipt()
        before = copy.deepcopy(captured)
        result = verify_audio_pins(captured, current)
        self.assertEqual(result, captured['inputs']['media-shared-ggml'])
        self.assertEqual(captured, before)
        captured['inputs']['rfdetr-patched-ggml'] = {'tree_sha256': result['tree_sha256']}
        verify_audio_pins(captured, current)

    def test_legacy_receipt_is_refused_even_with_unchanged_audio_pins(self):
        captured = legacy_receipt()
        with self.assertRaises(ValueError):
            verify_audio_pins(captured, copy.deepcopy(captured['inputs']))

    def test_shared_binding_requires_current_sources_and_matching_compiled_tree(self):
        for field in ('schema', 'tree_sha256'):
            with self.subTest(field=field):
                captured, current = shared_receipt()
                captured['inputs']['media-shared-ggml'][field] = 'changed'
                with self.assertRaises(ValueError):
                    verify_audio_pins(captured, current)
        for source in ('ggml', 'parakeet', 'rfdetr'):
            with self.subTest(source=source):
                captured, current = shared_receipt()
                captured['inputs']['media-shared-ggml']['sources'][source] = 'd' * 64
                with self.assertRaises(ValueError):
                    verify_audio_pins(captured, current)
        for engine in ('parakeet', 'rfdetr'):
            with self.subTest(engine=engine):
                captured, current = shared_receipt()
                captured['inputs'][engine + '-patched-ggml'] = {'tree_sha256': 'd' * 64}
                with self.assertRaises(ValueError):
                    verify_audio_pins(captured, current)

    def test_both_patch_families_are_required(self):
        for family in ('parakeet', 'rfdetr', 'mobile'):
            with self.subTest(family=family):
                captured, current = shared_receipt()
                binding = captured['inputs']['media-shared-ggml']
                binding['patches'] = [p for p in binding['patches'] if p['source_component'] != family]
                with self.assertRaises(ValueError):
                    verify_audio_pins(captured, current)

    def test_old_loader_or_guard_cannot_satisfy_current_audio_admission(self):
        for missing, stale in ((True, False), (False, True)):
            captured, current = shared_receipt()
            if missing:
                captured['patches'] = []
            if stale:
                captured['patches'][0]['sha256'] = 'd' * 64
            with self.assertRaises(ValueError): verify_audio_pins(captured, current)
        captured, current = shared_receipt()
        captured['inputs']['media-shared-ggml']['patches'][-1]['sha256'] = 'd' * 64
        with self.assertRaises(ValueError): verify_audio_pins(captured, current)

    def test_malformed_or_ambiguous_patch_records_are_refused(self):
        mutations = [
            lambda binding: binding.update(patches=[]),
            lambda binding: binding['patches'].append(copy.deepcopy(binding['patches'][0])),
            lambda binding: binding['patches'][0].update(source_component='other'),
            lambda binding: binding['patches'][0].update(sha256='not-a-digest'),
            lambda binding: binding['patches'][0].update(path='../outside.patch'),
            lambda binding: binding['patches'][0].update(path='/outside.patch'),
        ]
        for index, mutate in enumerate(mutations):
            with self.subTest(case=index):
                captured, current = shared_receipt()
                mutate(captured['inputs']['media-shared-ggml'])
                with self.assertRaises(ValueError):
                    verify_audio_pins(captured, current)

    def test_legacy_refusal_happens_before_any_output_is_created(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            retained = root / 'retained'
            receipts = retained / 'receipts'
            receipts.mkdir(parents=True)
            captured = legacy_receipt()
            captured.update(status='passed', exit=0, platform='ios')
            (receipts / 'mobile-native-build.json').write_text(json.dumps(captured))
            output = root / 'output'
            with patch('sys.argv', ['prepare.py', '--retained', str(retained), '--output', str(output)]):
                with self.assertRaises(ValueError):
                    main()
            self.assertFalse(output.exists())

    def test_failed_or_wrong_platform_receipt_is_refused_before_output(self):
        for fields in ({'status': 'failed', 'exit': 7, 'platform': 'ios'},
                       {'status': 'passed', 'exit': 0, 'platform': 'android'}):
            with self.subTest(fields=fields), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                retained = root / 'retained'
                (retained / 'receipts').mkdir(parents=True)
                captured, _ = shared_receipt()
                captured.update(fields)
                (retained / 'receipts/mobile-native-build.json').write_text(json.dumps(captured))
                (retained / 'retention.json').write_text(json.dumps({'artifacts': []}))
                output = root / 'output'
                with patch('sys.argv', ['prepare.py', '--retained', str(retained), '--output', str(output)]):
                    with self.assertRaisesRegex(ValueError, 'successful iOS native build'):
                        main()
                self.assertFalse(output.exists())

    def test_empty_archive_set_cannot_emit_verified_inputs(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            retained = root / 'retained'
            (retained / 'receipts').mkdir(parents=True)
            captured, _ = shared_receipt()
            captured.update(status='passed', exit=0, platform='ios')
            (retained / 'receipts/mobile-native-build.json').write_text(json.dumps(captured))
            (retained / 'retention.json').write_text(json.dumps({'artifacts': []}))
            output = root / 'output'
            with patch('sys.argv', ['prepare.py', '--retained', str(retained), '--output', str(output)]):
                with self.assertRaisesRegex(ValueError, 'Parakeet archive set'):
                    main()
            self.assertFalse((output / 'inputs.json').exists())


if __name__ == '__main__':
    unittest.main()
