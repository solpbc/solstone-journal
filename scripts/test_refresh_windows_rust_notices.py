#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Dependency-edge admission regressions for the mechanical notice refresh."""
import copy
import unittest

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
        refresh.require_unchanged_notice_closure(
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



if __name__ == "__main__":
    unittest.main()


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
