#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import distribution_rust_notices as notices


class NoticePathDependencyTests(unittest.TestCase):
    def test_unknown_path_dependency_is_refused_in_the_actual_closure_walk(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            dist = root / "core/distribution"
            dist.mkdir(parents=True)
            (root / "core/Cargo.toml").write_text('[workspace]\nmembers = ["crates/owned"]\nexclude = ["external", "crates/solstone-core-service-legacy-evidence"]\n')
            (dist / "inventory.toml").write_text('[[entry]]\nkind="bin"\npackage="owned"\ntargets=["linux-x86_64"]\n')
            def package(name, manifest):
                return {"id": name, "name": name, "version": "1.0.0", "source": None, "manifest_path": str(manifest)}
            metadata = {
                "packages": [package("owned", root / "core/crates/owned/Cargo.toml"),
                             package("unreviewed", root / "core/external/Cargo.toml")],
                "resolve": {"nodes": [{"id": "owned", "deps": [{"pkg": "unreviewed", "dep_kinds": [{"kind": None}]}]},
                                      {"id": "unreviewed", "deps": []}]}
            }
            result = subprocess.CompletedProcess([], 0, stdout=json.dumps(metadata).encode(), stderr=b"")
            with patch.object(notices, "ROOT", root), patch.object(notices, "DIST", dist), \
                 patch.object(notices.subprocess, "run", return_value=result), \
                 patch.object(notices, "shipped_feature_arguments", return_value=[]):
                with self.assertRaisesRegex(RuntimeError, "unaccounted no-source dependency: unreviewed"):
                    notices.closure("linux-x86_64", "x86_64-unknown-linux-gnu")
                metadata["packages"][1]["name"] = "solstone-core-service-legacy-evidence"
                metadata["packages"][1]["manifest_path"] = str(root / "core/crates/solstone-core-service-legacy-evidence/Cargo.toml")
                result.stdout = json.dumps(metadata).encode()
                self.assertEqual(notices.closure("linux-x86_64", "x86_64-unknown-linux-gnu"), ([], {}))

    def test_old_index_refuses_vendor_and_origin_changed_together(self):
        original_root = notices.ROOT
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            dist = root / "core/distribution"
            for relative in ["core/vendor/velopack", "core/distribution/licenses/velopack"]:
                shutil.copytree(original_root / relative, root / relative)
            lock = root / "core/Cargo.lock"
            lock.write_text("version = 4\n")
            identities = [notices.VELOPACK_IDENTITY]
            metadata = {notices.VELOPACK_IDENTITY: {"name": "velopack", "version": "1.2.0", "source": None}}
            with patch.object(notices, "ROOT", root), patch.object(notices, "DIST", dist), \
                 patch.object(notices, "LOCK", lock), \
                 patch.object(notices, "closure", return_value=(identities, metadata)), \
                 patch.object(notices, "shipped_feature_arguments", return_value=[]):
                notices.write("linux-x86_64", "x86_64-unknown-linux-gnu", {"packages": []}, {})
                notices.check("linux-x86_64", "x86_64-unknown-linux-gnu")
                vendor = root / "core/vendor/velopack"
                before = notices.patched_velopack_tree_sha256(vendor)
                source = vendor / "src/bundle.rs"
                source.write_bytes(source.read_bytes() + b"\n")
                after = notices.patched_velopack_tree_sha256(vendor)
                origin = vendor / "ORIGIN.toml"
                text = origin.read_text()
                self.assertEqual(text.count(before), 1)
                origin.write_text(text.replace(before, after))
                notices.prove_velopack_provenance()
                with self.assertRaisesRegex(RuntimeError, "Rust notices are missing or stale"):
                    notices.check("linux-x86_64", "x86_64-unknown-linux-gnu")


class ScreenOcrNoticeTests(unittest.TestCase):
    def test_registry_notice_source_uses_metadata_identity_when_cache_has_duplicate_copies(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cargo_home = root / "cargo"
            first = cargo_home / "registry/src/index-a/block-buffer-0.10.4"
            second = cargo_home / "registry/src/index-b/block-buffer-0.10.4"
            first.mkdir(parents=True)
            second.mkdir(parents=True)
            manifest = second / "Cargo.toml"
            manifest.write_text('[package]\nname="block-buffer"\nversion="0.10.4"\n')
            package = {
                "name": "block-buffer",
                "version": "0.10.4",
                "manifest_path": str(manifest),
            }
            with patch.dict("os.environ", {"CARGO_HOME": str(cargo_home)}):
                self.assertEqual(notices.registry_dir(package), second.resolve())

    def test_independent_closure_uses_helper_manifest_and_rejects_path_dependencies(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            helper = root / "core/screen-ocr-helper/Cargo.toml"
            helper.parent.mkdir(parents=True)
            helper.write_text('[package]\nname="solstone-screen-ocr-helper"\n')
            oar = root / "oar/Cargo.toml"
            oar.parent.mkdir()
            oar.write_text('[package]\nname="oar-ocr"\n')
            source = notices.screen_ocr_git_source()
            metadata = {
                "packages": [
                    {"id": "helper", "name": "solstone-screen-ocr-helper", "version": "0.1.0",
                     "source": None, "manifest_path": str(helper)},
                    {"id": "oar", "name": "oar-ocr", "version": "0.10.0", "source": source,
                     "license": "Apache-2.0", "manifest_path": str(oar)},
                ],
                "resolve": {"nodes": [
                    {"id": "helper", "deps": [{"pkg": "oar", "dep_kinds": [{"kind": None}]}]},
                    {"id": "oar", "deps": []},
                ]},
            }
            lock = {"package": [{"name": "oar-ocr", "version": "0.10.0", "source": source}]}
            lock_path = root / "Cargo.lock"
            lock_path.write_text("fixture lock")
            result = subprocess.CompletedProcess([], 0, stdout=json.dumps(metadata).encode(), stderr=b"")
            with patch.object(notices, "SCREEN_OCR_MANIFEST", helper), \
                 patch.object(notices, "SCREEN_OCR_LOCK", lock_path), \
                 patch.object(notices.tomllib, "loads", side_effect=lambda text: lock), \
                 patch.object(notices.subprocess, "run", return_value=result) as run:
                identities, packages = notices.screen_ocr_closure(
                    "linux-x86_64", "x86_64-unknown-linux-gnu"
                )
                self.assertEqual(len(identities), 1)
                self.assertEqual(packages[identities[0]]["name"], "oar-ocr")
                self.assertIn(str(helper), run.call_args.args[0])

                metadata["packages"].append({
                    "id": "path-dep", "name": "unowned", "version": "1.0.0",
                    "source": None, "manifest_path": str(root / "unowned/Cargo.toml"),
                })
                metadata["resolve"]["nodes"][0]["deps"].append(
                    {"pkg": "path-dep", "dep_kinds": [{"kind": None}]}
                )
                metadata["resolve"]["nodes"].append({"id": "path-dep", "deps": []})
                result.stdout = json.dumps(metadata).encode()
                with self.assertRaisesRegex(RuntimeError, "unowned path dependency"):
                    notices.screen_ocr_closure("linux-x86_64", "x86_64-unknown-linux-gnu")

    def test_helper_recipe_lock_and_source_directions_are_bound(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            helper = root / "core/screen-ocr-helper"
            distribution = root / "core/distribution"
            helper.mkdir(parents=True)
            distribution.mkdir(parents=True)
            manifest = helper / "Cargo.toml"
            manifest.write_text('[package]\nname="solstone-screen-ocr-helper"\nlicense="AGPL-3.0-only"\n')
            lock = helper / "Cargo.lock"
            source = notices.screen_ocr_git_source()
            lock.write_text(f'[[package]]\nname="oar-ocr"\nversion="0.10.0"\nsource="{source}"\n')
            recipe = helper / "recipe.toml"
            recipe.write_text(
                f'[source]\noar_git="{notices.OAR_URL}"\n'
                f'oar_revision="{notices.OAR_REVISION}"\noar_default_features=false\n'
            )
            directions = helper / "SOURCE-DIRECTIONS.md"
            directions.write_text(" ".join((notices.ORT_SOURCE_URL, notices.ORT_SOURCE_REVISION,
                                             notices.EIGEN_SOURCE_URL, notices.EIGEN_SOURCE_REVISION)))
            with patch.object(notices, "SCREEN_OCR_MANIFEST", manifest), \
                 patch.object(notices, "SCREEN_OCR_LOCK", lock), \
                 patch.object(notices, "SCREEN_OCR_RECIPE", recipe), \
                 patch.object(notices, "SCREEN_OCR_SOURCE_DIRECTIONS", directions):
                notices.verify_screen_ocr_inputs()
                recipe.write_text(recipe.read_text().replace(notices.OAR_REVISION, "0" * 40))
                with self.assertRaisesRegex(RuntimeError, "pinned OAR source"):
                    notices.verify_screen_ocr_inputs()

    def test_screen_ocr_notice_write_and_check_use_separate_files_and_recipe_identity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            distribution = root / "core/distribution"
            helper = root / "core/screen-ocr-helper"
            distribution.mkdir(parents=True)
            helper.mkdir(parents=True)
            manifest = helper / "Cargo.toml"
            lock = helper / "Cargo.lock"
            recipe = helper / "recipe.toml"
            directions = helper / "SOURCE-DIRECTIONS.md"
            manifest.write_bytes(b"helper manifest")
            lock.write_bytes(b"helper lock")
            recipe.write_bytes(b"helper recipe")
            directions.write_bytes(b"source directions")
            license_text = b"fixture external license\n"
            sha = notices.digest(license_text)
            identity = "fixture@1.0.0 (registry+https://github.com/rust-lang/crates.io-index)"
            package = {"name": "fixture", "version": "1.0.0", "source": "registry+https://github.com/rust-lang/crates.io-index",
                       "license": "MIT"}
            row = {"identity": identity, "license_expression": "MIT", "notice_references": [
                {"sha256": sha, "bytes": len(license_text), "source": {"kind": "fixture"}}
            ]}
            closures = {identity: package}
            recipe_data = {"source": {"oar_revision": notices.OAR_REVISION}}
            with patch.object(notices, "SCREEN_OCR_MANIFEST", manifest), \
                 patch.object(notices, "SCREEN_OCR_LOCK", lock), \
                 patch.object(notices, "SCREEN_OCR_RECIPE", recipe), \
                 patch.object(notices, "SCREEN_OCR_SOURCE_DIRECTIONS", directions), \
                 patch.object(notices, "SCREEN_OCR_NOTICE", distribution / "screen-ocr-rust-NOTICES.txt"), \
                 patch.object(notices, "SCREEN_OCR_INDEX", distribution / "screen-ocr-rust-sources.json"), \
                 patch.object(notices, "verify_screen_ocr_inputs", return_value=(recipe_data, {})), \
                 patch.object(notices, "screen_ocr_closure", return_value=([identity], closures)), \
                 patch.object(notices, "screen_ocr_source_row", return_value=row), \
                 patch.object(notices, "source_text", return_value=license_text):
                notices.write_screen_ocr_notices()
                notices.check_screen_ocr_notices()
                index = json.loads((distribution / "screen-ocr-rust-sources.json").read_bytes())
                self.assertEqual(index["schema"], "solstone.screen-ocr-rust-notices.v1")
                self.assertEqual(index["cargo_lock_sha256"], notices.digest(b"helper lock"))
                self.assertEqual(index["recipe_sha256"], notices.digest(b"helper recipe"))
                self.assertEqual(index["packages"][0]["identity"], identity)
                (distribution / "screen-ocr-rust-NOTICES.txt").write_bytes(b"stale")
                with self.assertRaisesRegex(RuntimeError, "missing or stale"):
                    notices.check_screen_ocr_notices()


if __name__ == "__main__":
    unittest.main()
