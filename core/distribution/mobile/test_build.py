# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc
"""Failure controls for the mobile build receipt and artifact boundary."""
import argparse
import contextlib
import concurrent.futures
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import threading
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "scripts"))
import mobile_native_build as mobile


class BuildControls(unittest.TestCase):
    def test_cpp_only_ios_build_does_not_acquire_or_modify_rust_toolchains(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args = argparse.Namespace(work_dir=root / "build", cache_dir=root / "cache",
                                      jobs=1, platform="ios", components=["llama"], full_run=False)
            build = mobile.Build(args)
            with (
                mock.patch.object(mobile, "require_current_pins"),
                mock.patch.object(mobile.platform, "system", return_value="Darwin"),
                mock.patch.object(mobile.platform, "machine", return_value="arm64"),
                mock.patch.object(build, "output", return_value="/fixture/sdk"),
                mock.patch.object(build, "run") as calls,
            ):
                build.setup()
            commands = [row.args[1] for row in calls.call_args_list]
            self.assertFalse(any(command[0] in ("rustup", "rustc") for command in commands))
            self.assertTrue(any(command[0] == "cmake" for command in commands))

    def test_qwen_rejects_a_4k_relro_end_even_with_16k_load_segments(self):
        # Captured from the first NDK r30 Qwen link: max-page-size alone leaves
        # GNU_RELRO at 0x100d000, which is unsafe on a 16 KiB Android runtime.
        loads = "LOAD 0xf60710 0xf68710 0xf68710 0xa4770 0xa48f0 RW 0x4000\n"
        bad = loads + "GNU_RELRO 0xf60710 0xf68710 0xf68710 0xa4770 0xa48f0 R 0x1\n"
        with self.assertRaisesRegex(ValueError, "GNU_RELRO"):
            mobile.require_android_qwen_geometry(bad)
        with self.assertRaises(ValueError):
            mobile.require_android_qwen_geometry(loads)

    def test_qwen_dynamic_exports_are_exact_on_both_platforms(self):
        for apple in (False, True):
            symbols = ["_" + name if apple else name for name in mobile.QWEN_EXPORTS]
            text = "\n".join("000000001 T " + name for name in symbols)
            self.assertEqual(set(mobile.require_qwen_exports(text, apple)), mobile.QWEN_EXPORTS)
            with self.assertRaisesRegex(ValueError, "unexpected"):
                mobile.require_qwen_exports(text + "\n000000002 T ggml_init", apple)
            with self.assertRaisesRegex(ValueError, "missing"):
                mobile.require_qwen_exports("\n".join(text.splitlines()[1:]), apple)

    def test_qwen_identity_binds_platform_patch_and_adapter_without_model_duplication(self):
        with tempfile.TemporaryDirectory() as directory:
            wrapper = Path(directory)
            revision = "1" * 40
            for name in mobile.QWEN_SOURCE_FILES:
                (wrapper / name).write_text("adapter " + name)
            (wrapper / "CMakeLists.txt").write_text('JOURNAL_MOBILE_QWEN_SOURCE_REVISION="' + revision + '"')
            pin = {"revision": revision, "sha256": "2" * 64}
            patches = [{"path": "lifecycle.patch", "sha256": "3" * 64}]
            binding, first = mobile.qwen_runtime_binding("ios", pin, "4" * 64, patches, wrapper, {"compiler": "test-compiler"})
            self.assertEqual(binding["policy"]["context_tokens"], 32768)
            self.assertEqual(binding["policy"]["parallel_slots"], 1)
            self.assertEqual((binding["policy"]["batch_tokens"], binding["policy"]["ubatch_tokens"]), (128, 128))
            android = mobile.qwen_runtime_binding("android", pin, "4" * 64, patches, wrapper, {"compiler": "test-compiler"})[0]
            self.assertEqual((android["policy"]["batch_tokens"], android["policy"]["ubatch_tokens"]), (2048, 512))
            self.assertEqual(mobile.qwen_runtime_binding("ios", pin, "4" * 64, patches, wrapper, {"compiler": "test-compiler"})[1], first)
            self.assertNotEqual(mobile.qwen_runtime_binding("android", pin, "4" * 64, patches, wrapper, {"compiler": "test-compiler"})[1], first)
            (wrapper / "journal_mobile_qwen.cpp").write_text("changed adapter")
            self.assertNotEqual(mobile.qwen_runtime_binding("ios", pin, "4" * 64, patches, wrapper, {"compiler": "test-compiler"})[1], first)
            pin["revision"] = "5" * 40
            with self.assertRaisesRegex(ValueError, "source revision"):
                mobile.qwen_runtime_binding("ios", pin, "4" * 64, patches, wrapper, {"compiler": "test-compiler"})

    def test_default_sources_can_supply_patches_before_their_engine_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "input.tar.gz"
            payload = b"pinned source\n"
            with tarfile.open(archive, "w:gz") as bundle:
                member = tarfile.TarInfo("upstream/file.txt")
                member.size = len(payload)
                bundle.addfile(member, io.BytesIO(payload))
            digest = mobile.sha256(archive)
            args = argparse.Namespace(work_dir=root / "build", cache_dir=root / "cache",
                                      jobs=1, platform="android", components=["parakeet"], full_run=False)
            build = mobile.Build(args)
            build.pins["parakeet"] = dict(url="https://invalid.example/source", sha256=digest, revision="fixture")
            archive.rename(build.cache / f"parakeet-{digest}.tar.gz")
            source = build.source("parakeet")
            self.assertEqual((source / "file.txt").read_bytes(), payload)
            # A second consumer shares this build's verified source, without
            # extraction over a tree that its first consumer has started using.
            (source / "consumer-state").write_text("retained")
            self.assertEqual(build.source("parakeet"), source)
            self.assertEqual((source / "consumer-state").read_text(), "retained")
            with self.assertRaisesRegex(ValueError, "populated"):
                build.source("parakeet", source)

    def test_media_engines_receive_the_same_complete_patch_union(self):
        with tempfile.TemporaryDirectory() as directory:
            build, sources = self.media_fixture(Path(directory))
            sources["sound-ced"] = build.sources / "sound-ced"
            sources["sound-ced"].mkdir()
            with contextlib.redirect_stdout(io.StringIO()):
                for component in (*mobile.MEDIA_GGML_COMPONENTS, "sound-ced"):
                    build.ggml(component, sources[component])
            binding = build.summary["inputs"]["media-shared-ggml"]
            self.assertEqual(binding["schema"], mobile.MEDIA_GGML_SCHEMA)
            self.assertEqual(set(binding["sources"]), {"ggml", "parakeet", "rfdetr"})
            # The duplicated common patch appears once; both distinct engine
            # requirements survive in each resulting tree and receipt.
            self.assertEqual(len(binding["patches"]), 5)
            self.assertEqual({p["source_component"] for p in binding["patches"]}, {"parakeet", "rfdetr", "mobile"})
            for component in (*mobile.MEDIA_GGML_COMPONENTS, "sound-ced"):
                self.assertEqual(build.summary["inputs"][component + "-patched-ggml"]["tree_sha256"], binding["tree_sha256"])
                ggml = sources[component] / "third_party/ggml"
                for file in ("common.c", "metal.metal", "cpu.c", "leaf.c", "guard.c"):
                    self.assertEqual((ggml / file).read_text(), "fixed\n")

    def test_android_llama_patch_binds_only_a_successfully_patched_tree(self):
        for before, succeeds in (("base", True), ("different", False)):
            with self.subTest(before=before), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                args = argparse.Namespace(work_dir=root / "build", cache_dir=root / "cache",
                                          jobs=1, platform="android", components=["llama"], full_run=False)
                build = mobile.Build(args)
                source = build.sources / "llama"
                source.mkdir()
                (source / "kernel.cpp").write_text(before + "\n")
                patch = root / "core/distribution/mobile/llama-android-dotprod-repack.patch"
                patch.parent.mkdir(parents=True)
                patch.write_text(self.patch("kernel.cpp"))
                with mock.patch.object(mobile, "ROOT", root), contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                    if succeeds:
                        build.patch_llama(source)
                    else:
                        with self.assertRaises(subprocess.CalledProcessError):
                            build.patch_llama(source)
                if succeeds:
                    self.assertEqual((source / "kernel.cpp").read_text(), "fixed\n")
                    self.assertEqual(build.summary["inputs"]["llama-patched-source"]["tree_sha256"], mobile.tree_digest(source))
                    self.assertEqual(build.summary["patches"][0]["sha256"], mobile.sha256(patch))
                else:
                    self.assertNotIn("llama-patched-source", build.summary["inputs"])
                    self.assertEqual(build.summary["patches"], [])

    def test_ios_llama_does_not_require_or_apply_the_android_patch(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args = argparse.Namespace(work_dir=root / "build", cache_dir=root / "cache",
                                      jobs=1, platform="ios", components=["llama"], full_run=False)
            build = mobile.Build(args)
            with mock.patch.object(mobile, "ROOT", root), mock.patch.object(build, "run", side_effect=AssertionError("unexpected patch")):
                build.patch_llama(root / "missing-source")
            self.assertEqual(build.summary["patches"], [])
            self.assertNotIn("llama-patched-source", build.summary["inputs"])

    def test_conflicting_media_patch_cannot_publish_a_closure_binding(self):
        with tempfile.TemporaryDirectory() as directory:
            build, sources = self.media_fixture(Path(directory))
            patch = sources["rfdetr"] / "third_party/ggml-patches/0002-profile.patch"
            patch.write_text(self.patch("cpu.c", "absent", "fixed"))
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(subprocess.CalledProcessError):
                    build.ggml("parakeet", sources["parakeet"])
            self.assertNotIn("media-shared-ggml", build.summary["inputs"])
            self.assertNotIn("parakeet-patched-ggml", build.summary["inputs"])

    def test_missing_media_patch_family_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            _, sources = self.media_fixture(Path(directory))
            for patch in (sources["rfdetr"] / "third_party/ggml-patches").glob("*.patch"):
                patch.unlink()
            with self.assertRaisesRegex(ValueError, "patch series missing"):
                mobile.media_ggml_patches(sources)

    @staticmethod
    def patch(file, before="base", after="fixed"):
        return f"diff --git a/{file} b/{file}\n--- a/{file}\n+++ b/{file}\n@@ -1 +1 @@\n-{before}\n+{after}\n"

    def media_fixture(self, root):
        args = argparse.Namespace(work_dir=root / "build", cache_dir=root / "cache",
                                  jobs=1, platform="android", components=["parakeet", "rfdetr"], full_run=False)
        build = mobile.Build(args)
        sources = {component: build.sources / component for component in mobile.MEDIA_GGML_COMPONENTS}
        for component, source in sources.items():
            folder = source / "third_party/ggml-patches"
            folder.mkdir(parents=True)
            (folder / "0001-common.patch").write_text(self.patch("common.c"))
            file = "metal.metal" if component == "parakeet" else "cpu.c"
            (folder / "0002-profile.patch").write_text(self.patch(file))
        build.source_roots.update(sources)
        base = root / "base-ggml"
        base.mkdir()
        for file in ("common.c", "metal.metal", "cpu.c", "leaf.c", "guard.c"):
            (base / file).write_text("base\n")
        archive = root / "ggml.tar.gz"
        with tarfile.open(archive, "w:gz") as bundle:
            bundle.add(base, arcname="upstream")
        digest = mobile.sha256(archive)
        build.pins["ggml"] = dict(url="https://invalid.example/ggml", sha256=digest, revision="fixture")
        archive.rename(build.cache / f"ggml-{digest}.tar.gz")
        patch = root / "core/distribution/mobile/ggml-android-q8-dotprod.patch"
        patch.parent.mkdir(parents=True)
        patch.write_text(self.patch("leaf.c"))
        (patch.parent / "ggml-metal-mapped-buffer-null.patch").write_text(self.patch("guard.c"))
        patcher = mock.patch.object(mobile, "ROOT", root)
        patcher.start()
        self.addCleanup(patcher.stop)
        return build, sources

    def test_android_media_leaf_failure_does_not_bind_shared_tree(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            build, sources = self.media_fixture(root)
            patch = root / "core/distribution/mobile/ggml-android-q8-dotprod.patch"
            patch.write_text(self.patch("missing.c"))
            with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                with self.assertRaises(subprocess.CalledProcessError):
                    build.ggml("parakeet", sources["parakeet"])
            self.assertNotIn("media-shared-ggml", build.summary["inputs"])

    def test_ios_media_union_does_not_require_android_leaf(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            build, sources = self.media_fixture(root)
            build.args.platform = "ios"
            (root / "core/distribution/mobile/ggml-android-q8-dotprod.patch").unlink()
            with contextlib.redirect_stdout(io.StringIO()):
                build.ggml("parakeet", sources["parakeet"])
            binding = build.summary["inputs"]["media-shared-ggml"]
            self.assertEqual(len(binding["patches"]), 4)
            self.assertEqual((sources["parakeet"] / "third_party/ggml/leaf.c").read_text(), "base\n")

    def test_mapped_buffer_guard_is_identical_when_detector_builds_first(self):
        with tempfile.TemporaryDirectory() as directory:
            build, sources = self.media_fixture(Path(directory))
            build.args.platform = "ios"
            with contextlib.redirect_stdout(io.StringIO()):
                for component in ("rfdetr", "parakeet"):
                    build.ggml(component, sources[component])
            first = build.summary["inputs"]["rfdetr-patched-ggml"]
            self.assertEqual(first, build.summary["inputs"]["parakeet-patched-ggml"])
            for source in sources.values():
                self.assertEqual((source / "third_party/ggml/guard.c").read_text(), "fixed\n")

    def test_cargo_binary_selection_excludes_host_build_scripts_and_launchers(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory)
            log = target / "cargo.log"
            binary = {"reason": "compiler-artifact", "target": {"kind": ["bin"]},
                      "executable": str(target / "native")}
            host = {"reason": "compiler-artifact", "target": {"kind": ["custom-build"]},
                    "executable": "/host/build-script"}
            (target / "launcher").write_text("#!/bin/sh\n")
            log.write_text("human diagnostic\n" + json.dumps(host) + "\n" + json.dumps(binary) + "\n")
            self.assertEqual(mobile.cargo_binaries([log], target), [target / "native"])
            binary["executable"] = "/host/native"
            log.write_text(json.dumps(binary) + "\n")
            with self.assertRaisesRegex(ValueError, "outside the target directory"):
                mobile.cargo_binaries([log], target)

    def test_running_receipt_cannot_report_success(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args = argparse.Namespace(work_dir=root / "build", cache_dir=root / "cache",
                                      jobs=1, platform="android", components=["ced"], full_run=False)
            build = mobile.Build(args)
            build.run("control", [sys.executable, "-c", "pass"])
            receipt = json.loads((build.receipts / "mobile-native-build.json").read_text())
            self.assertEqual(receipt["status"], "running")
            self.assertIsNone(receipt["exit"])
            self.assertFalse(receipt["complete"])
            with self.assertRaises(FileExistsError):
                mobile.Build(args)

    def test_command_failure_reaches_process_and_terminal_receipt(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def fail(build):
                build.run("failed-control", [sys.executable, "-c", "raise SystemExit(7)"])
            argv = ["mobile-native-build", "--platform", "android", "--work-dir", str(root / "build"),
                    "--cache-dir", str(root / "cache"), "--components", "ced"]
            with mock.patch.object(sys, "argv", argv), mock.patch.object(mobile.Build, "execute", fail):
                with contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(mobile.main(), 7)
            receipt = json.loads((root / "build/receipts/mobile-native-build.json").read_text())
            self.assertEqual(receipt["status"], "failed")
            self.assertEqual(receipt["exit"], 7)
            self.assertFalse(receipt["complete"])
            self.assertEqual(receipt["steps"][-1]["exit"], 7)

    def test_mixed_or_missing_architecture_is_refused(self):
        mobile.require_android_architecture("  Machine: AArch64\n  Machine: AArch64\n")
        for text in ("", "Machine: Advanced Micro Devices X86-64\n",
                     "Machine: AArch64\nMachine: ARM\n"):
            with self.assertRaises(ValueError):
                mobile.require_android_architecture(text)
        mobile.require_ios_platform("platform 2\nminos 26.0\nplatform 2\nminos 13.0\n")
        mobile.require_ios_platform("platform 2\nminos 26.0\nLoad command 3\ncmd LC_SOURCE_VERSION\nversion 1267.0\n")
        for text in ("", "platform 2\nplatform 1\n", "platform 7\n", "platform 2\nminos 26.1\n"):
            with self.assertRaises(ValueError):
                mobile.require_ios_platform(text)

    def test_changed_cached_bytes_are_not_repaired_or_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "input.tar.gz"
            archive.write_bytes(b"verified input")
            pin = {"filename": archive.name, "url": "https://invalid.example/input", "sha256": mobile.sha256(archive)}
            self.assertEqual(mobile.acquire(pin, root), archive)
            archive.write_bytes(b"changed input")
            with self.assertRaisesRegex(ValueError, "input digest mismatch"):
                mobile.acquire(pin, root)

    def test_untracked_source_changes_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "-q", root], check=True)
            before = mobile.source_digest(root)
            (root / "recipe.py").write_text("new source\n")
            self.assertNotEqual(before, mobile.source_digest(root))

    def test_concurrent_acquisitions_share_only_verified_final_bytes(self):
        barrier = threading.Barrier(2, timeout=5)
        class Response(io.BytesIO):
            first = True
            def read(self, size=-1):
                if self.first:
                    self.first = False
                    barrier.wait()
                return super().read(size)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            payload = b"verified concurrent input"
            pin = {"filename": "input.tar.gz", "url": "https://invalid.example/input",
                   "sha256": mobile.hashlib.sha256(payload).hexdigest()}
            with mock.patch("android_cross_build.urllib.request.urlopen", side_effect=lambda *a, **k: Response(payload)):
                with concurrent.futures.ThreadPoolExecutor(max_workers=2) as workers:
                    results = list(workers.map(lambda _: mobile.acquire(pin, root), range(2)))
            self.assertEqual(results, [root / pin["filename"]] * 2)
            self.assertEqual(results[0].read_bytes(), payload)
            self.assertEqual(list(root.iterdir()), [results[0]])


if __name__ == "__main__":
    unittest.main()
