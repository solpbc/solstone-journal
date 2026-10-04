#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Dependency-edge admission regressions for the mechanical notice refresh."""
import copy
import unittest
import tempfile
from pathlib import Path

import refresh_windows_rust_notices as refresh

SOURCE = "registry+https://github.com/rust-lang/crates.io-index"


def package(name, version="1.0.0", source=None, dependencies=None):
    row = {"name": name, "version": version}
    if source:
        row.update(source=source, checksum="unchanged")
    if dependencies:
        row["dependencies"] = dependencies
    return row


def graph(extra_edge=False, digest_reachable=True):
    edge = lambda key: {"pkg": key, "dep_kinds": [{"kind": None}]}
    return {
        "workspace:app": {"deps": [edge("workspace:indexer")] + (
            [edge("digest")] if digest_reachable else []
        )},
        "workspace:indexer": {"deps": [edge("digest")] if extra_edge else []},
        "digest": {"deps": []},
    }


class ExistingDependencyEdges(unittest.TestCase):
    def setUp(self):
        self.old = {"package": [
            package("app", dependencies=["indexer", "digest"]),
            package("indexer"),
            package("digest", source=SOURCE),
        ]}
        self.new = copy.deepcopy(self.old)
        self.new["package"][1]["dependencies"] = ["digest"]

    def admit(self, old_graph=None, new_graph=None):
        before = [p for p in self.old["package"] if p.get("source")]
        after = [p for p in self.new["package"] if p.get("source")]
        self.assertEqual(refresh.classify_external_delta(before, after), ([], []))
        changes = refresh.workspace_version_or_existing_dependency_edge_delta(
            self.old, self.new, external_unchanged=before == after
        )
        refresh.require_explained_notice_closure(
            old_graph or graph(), new_graph or graph(extra_edge=True), ["app"]
        )
        return changes

    def test_existing_external_edge_preserves_notice_closure(self):
        self.assertEqual(self.admit(), ["indexer"])

    def test_removed_existing_edge_preserves_notice_closure(self):
        self.old, self.new = self.new, self.old
        self.assertEqual(self.admit(), ["indexer"])

    def test_newly_reachable_external_package_refuses(self):
        with self.assertRaisesRegex(refresh.RefreshError, "notice closure changed"):
            self.admit(graph(digest_reachable=False), graph(extra_edge=True))

    def test_changed_external_population_refuses(self):
        # A removal (or an upgrade, which reads as one) always refuses; an
        # addition is a candidate the closure and licence checks decide.
        self.old["package"].append(package("unknown", source=SOURCE))
        with self.assertRaisesRegex(refresh.RefreshError, "population changed"):
            self.admit()

    def test_changed_external_row_cannot_be_admitted_by_claiming_equality(self):
        self.new["package"][2]["checksum"] = "changed"
        with self.assertRaises(refresh.RefreshError):
            self.admit()
        with self.assertRaises(refresh.RefreshError):
            refresh.workspace_version_or_existing_dependency_edge_delta(
                self.old, self.new, external_unchanged=True
            )

    def test_unknown_dependency_refuses(self):
        self.new["package"][1]["dependencies"] = ["unknown"]
        with self.assertRaises(refresh.RefreshError):
            self.admit()

    def test_ambiguous_bare_name_refuses_but_exact_version_resolves(self):
        for lock in (self.old, self.new):
            lock["package"].append(package("digest", version="2.0.0", source=SOURCE))
        with self.assertRaises(refresh.RefreshError):
            self.admit()
        self.new["package"][1]["dependencies"] = ["digest 1.0.0"]
        self.assertEqual(self.admit(), ["indexer"])

    def test_source_qualification_is_exact(self):
        packages = [package("digest", source=SOURCE), package("digest", source="registry+https://other.example/index")]
        self.assertIsNone(refresh._resolve_lock_dependency("digest 1.0.0", packages))
        self.assertEqual(refresh._resolve_lock_dependency(f"digest 1.0.0 ({SOURCE})", packages), ("digest", "1.0.0", SOURCE))
        self.assertIsNone(refresh._resolve_lock_dependency("digest 1.0.0 (unknown)", packages))

    def test_existing_workspace_edge_and_version_move_remain_supported(self):
        self.new["package"][1]["dependencies"] = ["app"]
        self.new["package"][0]["version"] = "2.0.0"
        self.assertEqual(self.admit(), ["app", "indexer"])

    def test_workspace_edge_survives_unrelated_external_git_pin(self):
        git_old = "git+https://example.invalid/spl?tag=v1#" + "a" * 40
        git_new = "git+https://example.invalid/spl?tag=v2#" + "b" * 40
        self.old["package"].append(package("spl-core", source=git_old))
        self.new["package"].append(package("spl-core", source=git_new))
        self.new["package"][1]["dependencies"] = ["app"]
        before = [p for p in self.old["package"] if p.get("source")]
        after = [p for p in self.new["package"] if p.get("source")]
        self.assertEqual(len(refresh.classify_external_delta(before, after)[0]), 1)
        self.assertEqual(
            refresh.workspace_version_or_existing_dependency_edge_delta(
                self.old, self.new, external_unchanged=before == after
            ),
            ["indexer"],
        )

    def test_existing_external_edge_survives_unrelated_external_git_pin(self):
        git_old = "git+https://example.invalid/spl?tag=v1#" + "a" * 40
        git_new = "git+https://example.invalid/spl?tag=v2#" + "b" * 40
        self.old["package"].append(package("spl-core", source=git_old))
        self.new["package"].append(package("spl-core", source=git_new))
        self.assertEqual(
            refresh.workspace_version_or_existing_dependency_edge_delta(
                self.old, self.new, external_unchanged=False
            ),
            ["indexer"],
        )


class ExternalPopulationDigest(unittest.TestCase):
    def test_order_independent(self):
        a = [package("digest", source=SOURCE), package("indexer", version="2.0.0", source=SOURCE)]
        b = list(reversed(a))
        self.assertEqual(
            refresh.external_population_sha256(a), refresh.external_population_sha256(b)
        )

    def test_changed_population_changes_digest(self):
        a = [package("digest", source=SOURCE)]
        b = [package("digest", version="2.0.0", source=SOURCE)]
        self.assertNotEqual(
            refresh.external_population_sha256(a), refresh.external_population_sha256(b)
        )


class GitPinVendorDelta(unittest.TestCase):
    prefix = "vendor/spl-transport-0.1.0/"

    def test_git_pin_may_change_resolved_edges_before_closure_check(self):
        old_source = "git+https://example.invalid/spl?tag=v1#" + "a" * 40
        new_source = "git+https://example.invalid/spl?tag=v2#" + "b" * 40
        before = [package("spl-core", source=old_source, dependencies=["base64"])]
        after = [package("spl-core", source=new_source, dependencies=["base64", "indexmap"])]
        self.assertEqual(refresh.classify_external_delta(before, after), ([(before[0], after[0])], []))

    def test_git_pin_still_refuses_unrelated_row_change(self):
        old_source = "git+https://example.invalid/spl?tag=v1#" + "a" * 40
        new_source = "git+https://example.invalid/spl?tag=v2#" + "b" * 40
        before = [package("spl-core", source=old_source)]
        after = [package("spl-core", source=new_source)]
        after[0]["checksum"] = "changed"
        with self.assertRaisesRegex(refresh.RefreshError, "changed beyond"):
            refresh.classify_external_delta(before, after)

    def test_added_file_under_moved_prefix_is_admitted(self):
        prior = {self.prefix + "src/lib.rs"}
        fresh = {self.prefix + "src/lib.rs", self.prefix + "src/observe.rs"}
        self.assertEqual(
            refresh.admit_git_pin_vendor_delta(prior, fresh, (self.prefix,)),
            [self.prefix + "src/observe.rs"],
        )

    def test_identical_member_set_returns_empty(self):
        members = {self.prefix + "src/lib.rs"}
        self.assertEqual(
            refresh.admit_git_pin_vendor_delta(members, members, (self.prefix,)),
            [],
        )

    def test_added_file_outside_moved_prefix_refuses(self):
        prior = {self.prefix + "src/lib.rs"}
        fresh = {self.prefix + "src/lib.rs", "vendor/other-1.0.0/src/lib.rs"}
        with self.assertRaisesRegex(refresh.RefreshError, "moved package prefix"):
            refresh.admit_git_pin_vendor_delta(prior, fresh, (self.prefix,))

    def test_removed_file_refuses(self):
        prior = {self.prefix + "src/lib.rs", self.prefix + "src/old.rs"}
        fresh = {self.prefix + "src/lib.rs"}
        with self.assertRaisesRegex(refresh.RefreshError, "moved package prefix"):
            refresh.admit_git_pin_vendor_delta(prior, fresh, (self.prefix,))

class SourceOfferArchive(unittest.TestCase):
    def test_checkout_git_metadata_is_not_archived(self):
        import io
        import pathlib
        import tarfile
        import tempfile

        with tempfile.TemporaryDirectory() as root:
            checkout = pathlib.Path(root)
            (checkout / ".git" / "logs").mkdir(parents=True)
            (checkout / ".git" / "config").write_text("[remote]\n")
            (checkout / ".git" / "logs" / "HEAD").write_text("identity\n")
            (checkout / ".cargo-ok").write_text("")
            (checkout / "src").mkdir()
            (checkout / "src" / "lib.rs").write_text("")
            (checkout / "LICENSE").write_text("licence\n")

            data = refresh.source_tar(checkout)

        with tarfile.open(fileobj=io.BytesIO(data)) as tar:
            names = sorted(tar.getnames())
        self.assertEqual(names, ["LICENSE", "src", "src/lib.rs"])


class RegistryAdditionOutsideClosure(unittest.TestCase):
    def added(self, name="resolver", source=SOURCE, checksum="c" * 64):
        row = package(name, source=source)
        row["checksum"] = checksum
        return row

    def test_registry_addition_is_returned_for_the_caller_to_check(self):
        before = [package("digest", source=SOURCE)]
        extra = self.added()
        moved, added = refresh.classify_external_delta(before, before + [extra])
        self.assertEqual((moved, added), ([], [extra]))

    def test_removal_still_refuses(self):
        before = [package("digest", source=SOURCE), package("old", source=SOURCE)]
        with self.assertRaisesRegex(refresh.RefreshError, "removed or upgraded"):
            refresh.classify_external_delta(before, [before[0], self.added()])

    def test_git_addition_refuses(self):
        before = [package("digest", source=SOURCE)]
        git = "git+https://example.invalid/x?tag=v1#" + "a" * 40
        with self.assertRaisesRegex(refresh.RefreshError, "checksummed registry"):
            refresh.classify_external_delta(before, before + [self.added(source=git)])

    def test_workspace_edge_to_an_addition_is_admitted(self):
        extra = self.added()
        old = {"package": [package("app", dependencies=["digest"]), package("digest", source=SOURCE)]}
        new = copy.deepcopy(old)
        new["package"][0]["dependencies"] = ["digest", "resolver"]
        new["package"].append(extra)
        ids = frozenset({(extra["name"], extra["version"], extra["source"])})
        self.assertEqual(
            refresh.workspace_version_or_existing_dependency_edge_delta(
                old, new, external_unchanged=True, added=ids
            ),
            ["app"],
        )
        with self.assertRaisesRegex(refresh.RefreshError, "changed beyond"):
            refresh.workspace_version_or_existing_dependency_edge_delta(
                old, new, external_unchanged=True
            )

    def meta(self, license):
        return {"packages": [{"name": "resolver", "version": "1.0.0", "source": SOURCE, "license": license}]}

    def test_permissive_licences_are_admitted(self):
        for license in ("MIT OR Apache-2.0", "MIT/Apache-2.0", "(MIT OR Apache-2.0) AND Unicode-3.0"):
            refresh.require_permissive_additions(self.meta(license), [self.added()])

    def test_copyleft_or_missing_licence_refuses(self):
        for license in ("GPL-3.0-only", "MIT OR LGPL-2.1", "", None):
            with self.assertRaisesRegex(refresh.RefreshError, "permissive"):
                refresh.require_permissive_additions(self.meta(license), [self.added()])


class RegistryEdgeReResolution(unittest.TestCase):
    def test_registry_row_with_only_new_edges_is_admitted(self):
        before = [package("curve", source=SOURCE), package("kdf", source=SOURCE)]
        after = copy.deepcopy(before)
        after[0]["dependencies"] = ["kdf"]
        self.assertEqual(refresh.classify_external_delta(before, after), ([], []))

    def test_registry_row_with_a_new_checksum_still_refuses(self):
        before = [package("curve", source=SOURCE)]
        after = copy.deepcopy(before)
        after[0]["dependencies"] = ["kdf"]
        after[0]["checksum"] = "changed"
        with self.assertRaisesRegex(refresh.RefreshError, "not a git dependency"):
            refresh.classify_external_delta(before, after)


class WorkspaceCrateRemoval(unittest.TestCase):
    def test_removed_workspace_crate_and_its_dropped_edges_are_admitted(self):
        old = {"package": [package("app", dependencies=["transfer"]), package("transfer")]}
        new = {"package": [package("app")]}
        self.assertEqual(
            refresh.workspace_version_or_existing_dependency_edge_delta(
                old, new, external_unchanged=True
            ),
            ["removed:transfer", "app"],
        )

    def test_added_workspace_crate_still_refuses(self):
        old = {"package": [package("app")]}
        new = {"package": [package("app"), package("fresh")]}
        with self.assertRaisesRegex(refresh.RefreshError, "crate was added"):
            refresh.workspace_version_or_existing_dependency_edge_delta(
                old, new, external_unchanged=True
            )


class RegistryAdditionInsideClosure(unittest.TestCase):
    """A package a Windows binary reaches is admitted only by name."""

    webview = f"webview@1.0.0 ({SOURCE})"
    digest = f"digest@1.0.0 ({SOURCE})"

    def graphs(self):
        edge = lambda key: {"pkg": key, "dep_kinds": [{"kind": None}]}
        old = {
            "workspace:app": {"deps": [edge(self.digest)]},
            self.digest: {"deps": []},
        }
        new = copy.deepcopy(old)
        new["workspace:shell"] = {"deps": [edge(self.digest), edge(self.webview)]}
        new[self.webview] = {"deps": []}
        return old, new

    def check(self, admitted=frozenset({webview}), added_workspace=frozenset({"shell"})):
        old, new = self.graphs()
        return refresh.require_explained_notice_closure(
            old,
            new,
            ["app", "shell"],
            added=frozenset({self.webview}),
            admitted=admitted,
            added_workspace=added_workspace,
        )

    def meta(self, license):
        return {"packages": [{"name": "webview", "version": "1.0.0", "source": SOURCE, "license": license}]}

    def lock_row(self):
        row = package("webview", source=SOURCE)
        row["checksum"] = "c" * 64
        return row

    def test_named_in_closure_addition_is_admitted(self):
        self.assertEqual(self.check(), {self.webview})
        refresh.require_permissive_additions(self.meta("MIT"), [self.lock_row()])

    def test_unnamed_in_closure_addition_refuses(self):
        with self.assertRaisesRegex(refresh.RefreshError, "--admit-closure-addition"):
            self.check(admitted=frozenset())

    def test_copyleft_in_closure_addition_refuses_even_when_named(self):
        self.assertEqual(self.check(), {self.webview})
        for license in ("GPL-3.0-only", "MIT AND LGPL-2.1-or-later", None):
            with self.assertRaisesRegex(refresh.RefreshError, "permissive"):
                refresh.require_permissive_additions(self.meta(license), [self.lock_row()])

    def test_named_package_outside_the_closure_refuses(self):
        old, new = self.graphs()
        new["workspace:shell"]["deps"] = new["workspace:shell"]["deps"][:1]
        with self.assertRaisesRegex(refresh.RefreshError, "do not reach"):
            refresh.require_explained_notice_closure(
                old,
                new,
                ["app", "shell"],
                added=frozenset({self.webview}),
                admitted=frozenset({self.webview}),
                added_workspace=frozenset({"shell"}),
            )

    def test_existing_package_entering_the_closure_still_refuses(self):
        old, new = self.graphs()
        other = f"other@1.0.0 ({SOURCE})"
        new[other] = {"deps": []}
        new["workspace:shell"]["deps"].append({"pkg": other, "dep_kinds": [{"kind": None}]})
        with self.assertRaisesRegex(refresh.RefreshError, "notice closure changed"):
            refresh.require_explained_notice_closure(
                old,
                new,
                ["app", "shell"],
                added=frozenset({self.webview}),
                admitted=frozenset({self.webview, other}),
                added_workspace=frozenset({"shell"}),
            )

    def test_new_root_must_be_an_added_workspace_crate(self):
        with self.assertRaisesRegex(refresh.RefreshError, "absent from the recovered graph"):
            self.check(added_workspace=frozenset())

    def test_admission_operand_must_name_a_version(self):
        self.assertEqual(
            refresh.parse_admissions(["webview@1.0.0"]), {("webview", "1.0.0")}
        )
        for value in ("webview", "webview@", "@1.0.0", "a@b@c"):
            with self.assertRaisesRegex(refresh.RefreshError, "NAME@VERSION"):
                refresh.parse_admissions([value])


class ShippedFeaturesAndPromotions(unittest.TestCase):
    def test_metadata_uses_canonical_shipped_features(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            features = root / "core/distribution/shipped-core-features.txt"
            features.parent.mkdir(parents=True)
            features.write_text("# release features\njournal-mcp-endpoint\nother # enabled\n\n")
            completed = type("Result", (), {"returncode": 0, "stdout": b"{}"})()
            with patch.object(refresh.subprocess, "run", return_value=completed) as run:
                refresh.query_cargo_metadata(root)
            argv = run.call_args.args[0]
            self.assertEqual(argv[-4:], ["--features", "solstone-core/journal-mcp-endpoint", "--features", "solstone-core/other"])

    def test_existing_gain_requires_explicit_promotion_and_stale_review_refuses(self):
        identity = f"digest@1.0.0 ({SOURCE})"
        old = {"workspace:app": {"deps": []}, identity: {"deps": []}}
        new = copy.deepcopy(old)
        new["workspace:app"]["deps"] = [{"pkg": identity, "dep_kinds": [{"kind": None}]}]
        with self.assertRaises(refresh.RefreshError):
            refresh.require_explained_notice_closure(old, new, ["app"])
        self.assertEqual(refresh.require_explained_notice_closure(old, new, ["app"], promoted=frozenset({identity})), {identity})
        with self.assertRaisesRegex(refresh.RefreshError, "do not reach"):
            refresh.require_explained_notice_closure(old, old, ["app"], promoted=frozenset({identity}))

    def test_standard_apache_requires_published_option_and_preserves_default_refusal(self):
        import io
        import tarfile
        row = {"name": "digest", "version": "1.0.0", "source": SOURCE,
               "license_expression": "Apache-2.0 OR MIT", "notice_references": []}
        import json
        root = Path(refresh.__file__).resolve().parent.parent
        index = json.loads((root / refresh.INDEX_RELATIVE_PATH).read_text())
        span = next(item for item in index["notice_texts"] if item["sha256"] == "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30")
        text = (root / refresh.NOTICES_RELATIVE_PATH).read_bytes()[span["byte_start_inclusive"]:span["byte_end_exclusive"]]
        with tempfile.TemporaryDirectory() as scratch:
            archive = Path(scratch) / "source.tar.gz"
            def write(expression, notice=False):
                with tarfile.open(archive, "w:gz") as tar:
                    data = f'[package]\nname="digest"\nversion="1.0.0"\nlicense="{expression}"\n'.encode()
                    info = tarfile.TarInfo("vendor/digest-1.0.0/Cargo.toml")
                    info.size = len(data)
                    tar.addfile(info, io.BytesIO(data))
                    if notice:
                        info = tarfile.TarInfo("vendor/digest-1.0.0/NOTICE")
                        info.size = 11
                        tar.addfile(info, io.BytesIO(b"attribution"))
            write(row["license_expression"])
            with self.assertRaises(refresh.RefreshError):
                refresh.promote_notice_rows(archive, [row])
            promoted, texts = refresh.promote_notice_rows(archive, [row], frozenset({("digest", "1.0.0")}), fetch=lambda url: (200, text))
            self.assertEqual(texts, {refresh.sha256_bytes(text): text})
            self.assertEqual(promoted[0]["notice_references"][0]["source"]["license_identifier"], "Apache-2.0")
            with self.assertRaisesRegex(refresh.RefreshError, "could not be acquired"):
                refresh.promote_notice_rows(archive, [row], frozenset({("digest", "1.0.0")}), fetch=lambda url: (200, text + b"changed"))
            write(row["license_expression"], notice=True)
            with self.assertRaisesRegex(refresh.RefreshError, "absent text"):
                refresh.promote_notice_rows(archive, [row], frozenset({("digest", "1.0.0")}), fetch=lambda url: (200, text))
            row["license_expression"] = "MIT"
            write("MIT")
            with self.assertRaisesRegex(refresh.RefreshError, "licence option"):
                refresh.promote_notice_rows(archive, [row], frozenset({("digest", "1.0.0")}))

    def test_promotion_reproduces_acquired_text_and_refuses_changed_notice(self):
        import io
        import tarfile
        text = b"MIT licence text with original attribution\n"
        row = {"name": "digest", "version": "1.0.0", "source": SOURCE,
               "license_expression": "MIT", "windows_notice_population": False,
               "notice_references": [{"bytes": len(text), "sha256": refresh.sha256_bytes(text),
                                      "source": {"kind": "cargo-registry-archive", "member": "LICENSE"}}]}
        with tempfile.TemporaryDirectory() as scratch:
            archive = Path(scratch) / "source.tar.gz"
            with tarfile.open(archive, "w:gz") as tar:
                for member, data in {"Cargo.toml": b'[package]\nname="digest"\nversion="1.0.0"\nlicense="MIT"\n', "LICENSE": text}.items():
                    info = tarfile.TarInfo("vendor/digest-1.0.0/" + member)
                    info.size = len(data)
                    tar.addfile(info, io.BytesIO(data))
            rows, texts = refresh.promote_notice_rows(archive, [row])
            self.assertTrue(rows[0]["windows_notice_population"])
            self.assertEqual(texts, {refresh.sha256_bytes(text): text})
            self.assertFalse(row["windows_notice_population"])
            row["notice_references"][0]["sha256"] = "0" * 64
            with self.assertRaisesRegex(refresh.RefreshError, "source text"):
                refresh.promote_notice_rows(archive, [row])


class AddedWorkspaceCrate(unittest.TestCase):
    def locks(self):
        extra = package("webview", source=SOURCE)
        extra["checksum"] = "c" * 64
        old = {"package": [package("app"), package("digest", source=SOURCE)]}
        new = copy.deepcopy(old)
        new["package"] += [package("shell", dependencies=["app", "digest", "webview"]), extra]
        return old, new, frozenset({("webview", "1.0.0", SOURCE)})

    def test_declared_crate_with_resolvable_edges_is_admitted(self):
        old, new, added = self.locks()
        self.assertEqual(
            refresh.workspace_version_or_existing_dependency_edge_delta(
                old, new, external_unchanged=True, added=added,
                declared_workspace=frozenset({"app", "shell"}),
            ),
            ["added:shell"],
        )

    def test_undeclared_crate_refuses(self):
        old, new, added = self.locks()
        with self.assertRaisesRegex(refresh.RefreshError, "crate was added"):
            refresh.workspace_version_or_existing_dependency_edge_delta(
                old, new, external_unchanged=True, added=added,
                declared_workspace=frozenset({"app"}),
            )

    def test_edge_to_an_unaccounted_package_refuses(self):
        old, new, _ = self.locks()
        with self.assertRaisesRegex(refresh.RefreshError, "added workspace crate"):
            refresh.workspace_version_or_existing_dependency_edge_delta(
                old, new, external_unchanged=True,
                declared_workspace=frozenset({"app", "shell"}),
            )

    def test_existing_crate_may_take_an_edge_to_the_new_crate(self):
        old, new, added = self.locks()
        new["package"][0]["dependencies"] = ["shell"]
        new["package"][2]["dependencies"] = ["digest", "webview"]
        self.assertEqual(
            refresh.workspace_version_or_existing_dependency_edge_delta(
                old, new, external_unchanged=True, added=added,
                declared_workspace=frozenset({"app", "shell"}),
            ),
            ["added:shell", "app"],
        )


class InClosureNoticeTexts(unittest.TestCase):
    mit = b"MIT License\n\nCopyright (c) upstream\n"

    def vendor(self, root, licence=None):
        import json
        import pathlib

        directory = pathlib.Path(root) / "webview-1.0.0"
        (directory / "src").mkdir(parents=True)
        (directory / "src" / "lib.rs").write_text("")
        (directory / ".cargo_vcs_info.json").write_text(json.dumps(
            {"git": {"sha1": "a" * 40}, "path_in_vcs": "crates/webview"}
        ))
        if licence is not None:
            (directory / "LICENSE-MIT").write_bytes(licence)
        return pathlib.Path(root)

    def rows(self, vendor_root, fetch=None):
        lock_row = package("webview", source=SOURCE)
        lock_row["checksum"] = "c" * 64
        metadata = {("webview", "1.0.0"): {
            "license": "MIT", "repository": "https://github.com/upstream/webview-rs",
        }}
        return refresh.added_package_rows(
            [lock_row], metadata, vendor_root,
            in_closure=frozenset({("webview", "1.0.0")}),
            fetch=fetch or self.fail_fetch,
        )

    def fail_fetch(self, url):
        self.fail(f"unexpected fetch of {url}")

    def upstream(self, served=None):
        import json

        blob = refresh.git_blob_sha1(self.mit)
        tree = {"truncated": False, "tree": [
            {"path": "crates/webview/Cargo.toml", "type": "blob", "sha": "1" * 40},
            {"path": "crates/webview", "type": "tree", "sha": "2" * 40},
            {"path": "LICENSE", "type": "blob", "sha": blob},
            {"path": "README.md", "type": "blob", "sha": "3" * 40},
        ]}
        responses = {
            "https://api.github.com/repos/upstream/webview-rs/git/trees/"
            + "a" * 40 + "?recursive=1": json.dumps(tree).encode(),
            "https://raw.githubusercontent.com/upstream/webview-rs/"
            + "a" * 40 + "/LICENSE": served if served is not None else self.mit,
        }
        return lambda url: (200, responses[url])

    def test_archive_licence_becomes_a_population_row_and_a_notice_text(self):
        import tempfile

        with tempfile.TemporaryDirectory() as root:
            rows, members, texts = self.rows(self.vendor(root, self.mit))
        self.assertTrue(rows[0]["windows_notice_population"])
        self.assertEqual(rows[0]["notice_status"], "texts-acquired")
        self.assertEqual(
            [r["source"]["kind"] for r in rows[0]["notice_references"]],
            ["cargo-registry-archive"],
        )
        self.assertEqual(texts, {refresh.sha256_bytes(self.mit): self.mit})
        self.assertIn("vendor/webview-1.0.0/LICENSE-MIT", members)

    def test_crate_without_a_licence_takes_its_pinned_upstream_text(self):
        import tempfile

        with tempfile.TemporaryDirectory() as root:
            rows, _, texts = self.rows(self.vendor(root), self.upstream())
        (reference,) = rows[0]["notice_references"]
        self.assertEqual(reference["source"]["kind"], "pinned-upstream-git-blob")
        self.assertEqual(reference["source"]["member"], "LICENSE")
        self.assertEqual(reference["source"]["revision"], "a" * 40)
        self.assertEqual(reference["source"]["git_blob_sha1"], refresh.git_blob_sha1(self.mit))
        self.assertEqual(texts, {refresh.sha256_bytes(self.mit): self.mit})

    def test_upstream_text_that_does_not_match_its_blob_refuses(self):
        import tempfile

        with tempfile.TemporaryDirectory() as root:
            with self.assertRaisesRegex(refresh.RefreshError, "does not hash to the blob"):
                self.rows(self.vendor(root), self.upstream(served=self.mit + b"tampered"))

    def test_outside_closure_row_takes_no_notice_text(self):
        import tempfile

        lock_row = package("webview", source=SOURCE)
        lock_row["checksum"] = "c" * 64
        with tempfile.TemporaryDirectory() as root:
            rows, _, texts = refresh.added_package_rows(
                [lock_row], {("webview", "1.0.0"): {"license": "MIT"}},
                self.vendor(root, self.mit), fetch=self.fail_fetch,
            )
        self.assertFalse(rows[0]["windows_notice_population"])
        self.assertEqual(texts, {})


class NoticesRendering(unittest.TestCase):
    def test_texts_are_ordered_by_digest_and_indexed_by_byte_range(self):
        texts = {refresh.sha256_bytes(data): data for data in (b"zeta\n", b"alpha")}
        rendered, index = refresh.render_notices(texts)
        self.assertTrue(rendered.startswith(refresh.NOTICES_HEADER))
        self.assertEqual([row["sha256"] for row in index], sorted(texts))
        for row in index:
            span = rendered[row["byte_start_inclusive"]:row["byte_end_exclusive"]]
            self.assertEqual(span, texts[row["sha256"]])
            self.assertEqual(row["bytes"], len(span))
            heading = f"SHA-256: {row['sha256']}\n\n".encode()
            self.assertEqual(rendered[row["byte_start_inclusive"] - len(heading):row["byte_start_inclusive"]], heading)
        self.assertTrue(rendered.endswith(b"\n\n"))

    def test_text_under_the_wrong_digest_refuses(self):
        with self.assertRaises(refresh.RefreshError):
            refresh.render_notices({"0" * 64: b"text"})


if __name__ == "__main__":
    unittest.main()
