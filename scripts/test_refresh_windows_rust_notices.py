#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""Dependency-edge admission regressions for the mechanical notice refresh."""
import copy
import io
import json
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
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


class StandaloneInventoryRoots(unittest.TestCase):
    def test_windows_roots_separate_ocr_without_hiding_unaccounted_binaries(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            inventory = root / refresh.INVENTORY_RELATIVE_PATH
            inventory.parent.mkdir(parents=True)
            inventory.write_text(
                '[[entry]]\nkind="bin"\npackage="app"\ntargets=["windows-x86_64"]\n'
                '[[entry]]\nkind="bin"\npackage="solstone-screen-ocr-helper"\ntargets=["windows-x86_64"]\n'
                '[[entry]]\nkind="bin"\npackage="unaccounted"\ntargets=["windows-x86_64"]\n'
            )
            roots = refresh.windows_roots(root)
            self.assertEqual(roots, ["app", "unaccounted"])
            metadata = {"packages": [{"id": "app", "name": "app", "source": None}],
                        "resolve": {"nodes": [{"id": "app", "deps": []}]}}
            with self.assertRaisesRegex(refresh.RefreshError, "expected one workspace package"):
                refresh.selected_external_identities(metadata, roots)


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

    def test_second_version_qualifies_an_existing_workspace_edge(self):
        extra = package("digest", version="2.0.0", source=SOURCE)
        self.new["package"].append(extra)
        self.new["package"][0]["dependencies"] = ["indexer", "digest 1.0.0"]
        self.new["package"][1]["dependencies"] = ["digest 1.0.0"]
        self.assertEqual(
            refresh.workspace_version_or_existing_dependency_edge_delta(
                self.old, self.new, external_unchanged=False,
                added=frozenset({("digest", "2.0.0", SOURCE)}),
            ),
            ["app", "indexer"],
        )
        self.new["package"][1]["dependencies"] = ["digest"]
        with self.assertRaises(refresh.RefreshError):
            refresh.workspace_version_or_existing_dependency_edge_delta(
                self.old, self.new, external_unchanged=False,
                added=frozenset({("digest", "2.0.0", SOURCE)}),
            )

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


    def test_workspace_edge_to_the_moved_git_package_is_the_same_package(self):
        git_old = "git+https://example.invalid/spl?tag=v1#" + "a" * 40
        git_new = "git+https://example.invalid/spl?tag=v2#" + "b" * 40
        self.old["package"].append(package("spl-core", source=git_old))
        self.new["package"].append(package("spl-core", source=git_new))
        # app also gains an ordinary workspace edge, so its edges are compared
        # by identity (the shape solstone-core took at spl-rust v0.12.0).
        self.old["package"][0]["dependencies"] = ["digest", "spl-core"]
        self.new["package"][0]["dependencies"] = ["indexer", "digest", "spl-core"]
        self.new["package"][0]["version"] = "2.0.0"
        moved = {("spl-core", "1.0.0", git_old): ("spl-core", "1.0.0", git_new)}
        # Without the proven move the edge reads as one to a package that vanished.
        with self.assertRaisesRegex(refresh.RefreshError, "changed beyond"):
            refresh.workspace_version_or_existing_dependency_edge_delta(
                self.old, self.new, external_unchanged=False
            )
        self.assertEqual(
            refresh.workspace_version_or_existing_dependency_edge_delta(
                self.old, self.new, external_unchanged=False, moved_git=moved
            ),
            ["app", "indexer"],
        )
        # An existing crate that newly takes an edge to the moved package's new pin.
        gained = copy.deepcopy(self.new)
        gained["package"][1]["dependencies"] = ["digest", "spl-core"]
        self.assertEqual(
            refresh.workspace_version_or_existing_dependency_edge_delta(
                self.old, gained, external_unchanged=False, moved_git=moved
            ),
            ["app", "indexer"],
        )
        with self.assertRaisesRegex(refresh.RefreshError, "changed beyond"):
            refresh.workspace_version_or_existing_dependency_edge_delta(
                self.old, gained, external_unchanged=False
            )
        # The move maps only its own pair: an edge to some other git package still refuses.
        other = "git+https://example.invalid/other?tag=v1#" + "c" * 40
        self.new["package"].append(package("other", source=other))
        self.new["package"][0]["dependencies"] = ["indexer", "digest", "spl-core", "other"]
        with self.assertRaisesRegex(refresh.RefreshError, "changed beyond"):
            refresh.workspace_version_or_existing_dependency_edge_delta(
                self.old, self.new, external_unchanged=False, moved_git=moved
            )

    def test_added_workspace_crate_may_depend_on_the_moved_git_package(self):
        git_old = "git+https://example.invalid/spl?tag=v1#" + "a" * 40
        git_new = "git+https://example.invalid/spl?tag=v2#" + "b" * 40
        self.old["package"].append(package("spl-core", source=git_old))
        self.new["package"].append(package("spl-core", source=git_new))
        self.new["package"].append(package("linker", dependencies=["spl-core"]))
        moved = {("spl-core", "1.0.0", git_old): ("spl-core", "1.0.0", git_new)}
        declared = frozenset({"app", "indexer", "linker"})
        with self.assertRaisesRegex(refresh.RefreshError, "added workspace crate"):
            refresh.workspace_version_or_existing_dependency_edge_delta(
                self.old, self.new, external_unchanged=False, declared_workspace=declared
            )
        self.assertEqual(
            refresh.workspace_version_or_existing_dependency_edge_delta(
                self.old, self.new, external_unchanged=False,
                declared_workspace=declared, moved_git=moved,
            ),
            ["added:linker", "indexer"],
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

    def test_unchanged_git_pin_survives_lock_dependency_disambiguation(self):
        source = "git+https://example.invalid/spl?tag=v1#" + "a" * 40
        before = [package("spl-core", source=source, dependencies=["nix"])]
        after = [package("spl-core", source=source, dependencies=["nix 0.30.1"])]
        self.assertEqual(refresh.classify_external_delta(before, after), ([], []))

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

    def substitutions(self, fresh_digest_manifest, removed_prefixes):
        """Run build_git_substitutions over a tiny prior archive and vendor tree.

        The prior archive carries the moved git package, a removed registry
        package and a surviving registry package whose checksum manifest was
        written in the licence-identical upgrade's sorted-key form.
        """
        moved = "vendor/spl-core-0.1.0/"
        prior = {
            moved + "src/lib.rs": b"old\n",
            "vendor/p256-0.13.2/src/lib.rs": b"p256\n",
            "vendor/digest-1.0.0/src/lib.rs": b"digest\n",
            "vendor/digest-1.0.0/.cargo-checksum.json": json.dumps(
                {"files": {"src/lib.rs": "d" * 64}, "package": "c" * 64}, sort_keys=True
            ).encode(),
        }
        fresh = dict(prior)
        del fresh["vendor/p256-0.13.2/src/lib.rs"]
        fresh[moved + "src/lib.rs"] = b"new\n"
        fresh["vendor/digest-1.0.0/.cargo-checksum.json"] = fresh_digest_manifest
        old_source = "git+https://example.invalid/spl?tag=v1#" + "a" * 40
        new_source = "git+https://example.invalid/spl?tag=v2#" + "b" * 40
        moved_git = [(package("spl-core", "0.1.0", old_source), package("spl-core", "0.1.0", new_source))]
        with tempfile.TemporaryDirectory() as scratch:
            archive = Path(scratch) / "prior.tar.gz"
            with tarfile.open(archive, "w:gz") as tar:
                for name, data in prior.items():
                    info = tarfile.TarInfo(name)
                    info.size = len(data)
                    tar.addfile(info, io.BytesIO(data))
            vendor = Path(scratch) / "vendor"
            for name, data in fresh.items():
                path = vendor / name.removeprefix("vendor/")
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
            with patch.object(refresh, "verify_vendored_against_checkout"), patch.object(
                refresh, "git_source_substitutions", return_value={}
            ):
                return refresh.build_git_substitutions(
                    Path(scratch), archive, moved_git, {}, vendor, [], removed_prefixes
                )

    def compact(self, files_digest):
        return json.dumps(
            {"$comment": "cargo vendor", "files": {"src/lib.rs": files_digest}, "package": "c" * 64},
            separators=(",", ":"),
        ).encode()

    def test_removed_registry_prefix_is_excluded_and_reformatted_manifest_keeps_prior_bytes(self):
        removed = ("vendor/p256-0.13.2/",)
        self.assertEqual(
            self.substitutions(self.compact("d" * 64), removed),
            {"vendor/spl-core-0.1.0/src/lib.rs": b"new\n"},
        )
        # Without the removal named, the vanished member refuses as before.
        with self.assertRaisesRegex(refresh.RefreshError, "moved package prefix"):
            self.substitutions(self.compact("d" * 64), ())
        # A manifest that attests different bytes is a change outside the move.
        with self.assertRaisesRegex(refresh.RefreshError, "outside the packages whose pin moved"):
            self.substitutions(self.compact("e" * 64), removed)
        with self.assertRaisesRegex(refresh.RefreshError, "no files map"):
            self.substitutions(b'{"package": null}', removed)


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

    def test_zip_upgrade_and_velopack_source_loss_still_refuses(self):
        before = [
            package("zip", version="2.4.2", source=SOURCE),
            package("zip", version="8.6.0", source=SOURCE),
            package("velopack", version="1.2.0", source=SOURCE),
        ]
        after = [package("zip", version="9.0.3", source=SOURCE)]
        with self.assertRaisesRegex(refresh.RefreshError, "removed or upgraded"):
            refresh.classify_external_delta(before, after)

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
        for license in ("MIT OR Apache-2.0", "MIT/Apache-2.0", "(MIT OR Apache-2.0) AND Unicode-3.0", "MIT OR Apache-2.0 OR BSD-1-Clause"):
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


class RegistryRemoval(unittest.TestCase):
    """A checksummed registry removal is admitted while the closure only loses
    named packages; a removed package inside it must be named."""

    digest = f"digest@1.0.0 ({SOURCE})"
    pdf = f"pdf@1.0.0 ({SOURCE})"
    padding = f"padding@1.0.0 ({SOURCE})"

    def removed(self, name="pdf"):
        row = package(name, source=SOURCE)
        row["checksum"] = "c" * 64
        return row

    def graphs(self, padding_in_closure=False):
        edge = lambda key, kind=None: {"pkg": key, "dep_kinds": [{"kind": kind}]}
        old = {
            "workspace:app": {"deps": [edge(self.digest), edge(self.pdf, "dev")]},
            self.digest: {"deps": [edge(self.padding)] if padding_in_closure else []},
            self.pdf: {"deps": [edge(self.padding)]},
            self.padding: {"deps": []},
        }
        new = {
            "workspace:app": {"deps": [edge(self.digest)]},
            self.digest: {"deps": []},
        }
        return old, new

    def test_removal_only_is_split_off_and_survivors_classify_unchanged(self):
        before = [package("digest", source=SOURCE), self.removed(), self.removed("padding")]
        after = [before[0]]
        surviving, removed = refresh.split_registry_removals(before, after)
        self.assertEqual((surviving, removed), (after, [before[2], before[1]]))
        self.assertEqual(refresh.classify_external_delta(surviving, after), ([], []))

    def test_mixed_or_unchecksummed_removal_is_not_split_and_still_refuses(self):
        git = "git+https://example.invalid/x?tag=v1#" + "a" * 40
        added = self.removed("resolver")
        for before, after in (
            ([package("digest", source=SOURCE), self.removed()], [package("digest", source=SOURCE), added]),
            ([package("digest", source=SOURCE), package("pdf", source=git)], [package("digest", source=SOURCE)]),
            ([package("pdf", version="1.0.0", source=SOURCE)], [package("pdf", version="2.0.0", source=SOURCE)]),
        ):
            self.assertEqual(refresh.split_registry_removals(before, after), (before, []))
            with self.assertRaisesRegex(refresh.RefreshError, "removed or upgraded"):
                refresh.classify_external_delta(before, after)

    def test_workspace_edge_to_a_removed_package_is_admitted(self):
        old = {"package": [package("app", dependencies=["digest", "pdf"]),
                           package("digest", source=SOURCE), self.removed()]}
        new = {"package": [package("app", dependencies=["digest"]), package("digest", source=SOURCE)]}
        ids = frozenset({("pdf", "1.0.0", SOURCE)})
        self.assertEqual(
            refresh.workspace_version_or_existing_dependency_edge_delta(
                old, new, external_unchanged=False, removed=ids
            ),
            ["app"],
        )
        with self.assertRaisesRegex(refresh.RefreshError, "changed beyond"):
            refresh.workspace_version_or_existing_dependency_edge_delta(
                old, new, external_unchanged=False
            )

    def test_removal_outside_the_closure_is_admitted(self):
        old, new = self.graphs()
        self.assertEqual(
            refresh.require_explained_notice_closure(
                old, new, ["app"], removed=frozenset({self.pdf, self.padding})
            ),
            frozenset(),
        )

    def test_unnamed_removal_inside_the_closure_refuses(self):
        old, new = self.graphs(padding_in_closure=True)
        with self.assertRaisesRegex(refresh.RefreshError, "--admit-closure-removal"):
            refresh.require_explained_notice_closure(
                old, new, ["app"], removed=frozenset({self.pdf, self.padding})
            )

    def test_named_removal_inside_the_closure_is_admitted_and_stale_name_refuses(self):
        old, new = self.graphs(padding_in_closure=True)
        removed = frozenset({self.pdf, self.padding})
        self.assertEqual(
            refresh.require_explained_notice_closure(
                old, new, ["app"], removed=removed, admitted_removals=frozenset({self.padding})
            ),
            frozenset(),
        )
        old, new = self.graphs()
        with self.assertRaisesRegex(refresh.RefreshError, "did not leave"):
            refresh.require_explained_notice_closure(
                old, new, ["app"], removed=removed, admitted_removals=frozenset({self.padding})
            )

    def test_surviving_package_leaving_the_closure_still_refuses(self):
        old, new = self.graphs()
        new["workspace:app"]["deps"] = []
        with self.assertRaisesRegex(refresh.RefreshError, "notice closure changed"):
            refresh.require_explained_notice_closure(
                old, new, ["app"], removed=frozenset({self.pdf, self.padding}),
                admitted_removals=frozenset({self.digest}),
            )

    def test_partially_named_in_closure_removals_refuse(self):
        old, new = self.graphs(padding_in_closure=True)
        old[self.digest]["deps"].append({"pkg": self.pdf, "dep_kinds": [{"kind": None}]})
        with self.assertRaisesRegex(refresh.RefreshError, "--admit-closure-removal"):
            refresh.require_explained_notice_closure(
                old, new, ["app"], removed=frozenset({self.pdf, self.padding}),
                admitted_removals=frozenset({self.padding}),
            )

    def test_registry_crate_resourced_into_the_workspace_is_not_a_removal(self):
        before = [package("digest", source=SOURCE), self.removed()]
        after = [before[0]]
        self.assertEqual(
            refresh.split_registry_removals(before, after, frozenset({"app", "pdf"})),
            (before, []),
        )

    def index(self):
        text = lambda key: {"sha256": key * 64, "bytes": 1, "source": {}}
        return {
            "packages": [
                {"identity": self.digest, "windows_notice_population": True,
                 "notice_references": [text("a")]},
                {"identity": self.padding, "windows_notice_population": True,
                 "notice_references": [text("a"), text("b")]},
                {"identity": self.pdf, "windows_notice_population": False,
                 "notice_references": [text("c")]},
            ],
            "path_dependencies": [
                {"identity": "shim@1.0.0 (path+vendor/shim)", "windows_notice_population": True,
                 "notice_references": [text("b")]},
            ],
        }

    def test_removal_keeping_every_notice_text_referenced_is_admitted(self):
        rows = [self.removed("padding"), self.removed()]
        self.assertEqual(
            refresh.removed_index_identities(
                self.index(), rows, frozenset({self.padding}), {"a" * 64, "b" * 64}
            ),
            {self.padding, self.pdf},
        )

    def test_removal_orphaning_a_notice_text_refuses(self):
        index = self.index()
        index["path_dependencies"] = []
        with self.assertRaisesRegex(refresh.RefreshError, "referenced by no"):
            refresh.removed_index_identities(
                index, [self.removed("padding")], frozenset({self.padding}), {"a" * 64, "b" * 64}
            )

    def test_named_orphaned_notice_text_may_leave_and_only_by_name(self):
        index = self.index()
        index["path_dependencies"] = []
        texts = {"a" * 64, "b" * 64}
        row = [self.removed("padding")]
        lost = frozenset({self.padding})
        self.assertEqual(
            refresh.removed_index_identities(index, row, lost, texts, frozenset({"b" * 64})),
            {self.padding},
        )
        with self.assertRaisesRegex(refresh.RefreshError, "not orphaned"):
            refresh.removed_index_identities(
                index, row, lost, texts, frozenset({"a" * 64, "b" * 64})
            )
        index["path_dependencies"] = self.index()["path_dependencies"]
        with self.assertRaisesRegex(refresh.RefreshError, "not orphaned"):
            refresh.removed_index_identities(index, row, lost, texts, frozenset({"b" * 64}))

    def test_text_removal_operand_must_be_a_digest(self):
        self.assertEqual(refresh.parse_text_removals(["b" * 64]), frozenset({"b" * 64}))
        with self.assertRaises(SystemExit):
            refresh.parse_text_removals(["B" * 64])

    def test_population_flag_must_match_the_recovered_closure(self):
        with self.assertRaisesRegex(refresh.RefreshError, "population flag"):
            refresh.removed_index_identities(
                self.index(), [self.removed("padding")], frozenset(), {"a" * 64, "b" * 64}
            )

    def prior(self, scratch, payload):
        import io
        import json
        import tarfile

        archive = Path(scratch) / "prior.tar.gz"
        data = json.dumps(payload).encode()
        with tarfile.open(archive, "w:gz") as tar:
            info = tarfile.TarInfo(refresh.GRAPH_MEMBER_NAME)
            info.size = len(data)
            tar.addfile(info, io.BytesIO(data))
        return archive

    def metadata(self, scratch, features):
        import json

        path = Path(scratch) / "metadata.json"
        path.write_text(json.dumps({
            "packages": [{"id": "app-id", "name": "app", "version": "1.0.0", "source": None}],
            "resolve": {"nodes": [{"id": "app-id", "deps": [], "features": features}]},
        }))
        return path

    def test_unwrapped_embedded_graph_must_match_prior_metadata(self):
        bare = {"workspace:app": {"features": [], "deps": []}}
        with tempfile.TemporaryDirectory() as scratch:
            archive = self.prior(scratch, bare)
            with self.assertRaisesRegex(refresh.RefreshError, "--prior-metadata"):
                refresh.load_prior_graph(archive, None)
            self.assertEqual(
                refresh.load_prior_graph(archive, self.metadata(scratch, [])), bare
            )
            with self.assertRaisesRegex(refresh.RefreshError, "does not reproduce"):
                refresh.load_prior_graph(archive, self.metadata(scratch, ["std"]))
            wrapped = self.prior(scratch, {"graph": bare})
            self.assertEqual(refresh.load_prior_graph(wrapped, None), bare)


class RegistryUpgrade(unittest.TestCase):
    def crate(self, version="1.0.1", expression="MIT", text=b"same licence\n", extra=None):
        files = {"Cargo.toml": f'[package]\nname="digest"\nversion="{version}"\nlicense="{expression}"\n'.encode(),
                 "LICENSE": text, "src/lib.rs": version.encode(), **(extra or {})}
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode="w:gz") as archive:
            for name, data in files.items():
                member = tarfile.TarInfo(f"digest-{version}/{name}")
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data))
        return stream.getvalue(), files

    def row(self, version="1.0.0", checksum="a" * 64):
        return {**package("digest", version, SOURCE), "checksum": checksum}

    def test_pairs_only_unambiguous_checksummed_crates_io_upgrades(self):
        old, new = self.row(), self.row("1.0.1")
        self.assertEqual(refresh.split_registry_upgrades([old], [new]), [(old, new)])
        for left, right in (
            ([old], [{**new, "checksum": "invalid"}]),
            ([old], [{**new, "source": "git+https://example.invalid/repo"}]),
            ([old], [{**new, "source": "registry+https://example.invalid/index"}]),
            ([old], [self.row("0.9.0")]),
            ([old], [self.row("1.0.1-rc.1")]),
            ([old], [new, {**new, "name": "added"}]),
            ([old, self.row("0.5.0")], [new]),
            ([old, old], [new]),
        ):
            with self.subTest(left=left, right=right), self.assertRaises(refresh.RefreshError):
                refresh.split_registry_upgrades(left, right)

    def test_upgraded_identity_cannot_promote_or_remove_a_closure_member(self):
        old, new = "digest@1.0.0", "digest@1.0.1"
        before = {"workspace:app": {"deps": [{"pkg": old, "dep_kinds": [{"kind": None}]}]}, old: {"deps": []}}
        after = {"workspace:app": {"deps": [{"pkg": new, "dep_kinds": [{"kind": None}]}]}, new: {"deps": []}}
        mapped = refresh.replace_graph_identities(after, {new: old})
        self.assertEqual(refresh.require_explained_notice_closure(before, mapped, ["app"]), frozenset())
        for left, right in ((before, {**mapped, "workspace:app": {"deps": []}}),
                            ({**before, "workspace:app": {"deps": []}}, mapped)):
            with self.assertRaises(refresh.RefreshError):
                refresh.require_explained_notice_closure(left, right, ["app"])

    def fixture(self, root, expression="MIT", new_expression=None, new_text=b"same licence\n", extra=None, git_move=False):
        data, files = self.crate(expression=new_expression or expression, text=new_text, extra=extra)
        old_data, old_files = self.crate("1.0.0", expression)
        old, new = self.row(checksum=refresh.sha256_bytes(old_data)), self.row("1.0.1", refresh.sha256_bytes(data))
        outside = {**package("outside", source=SOURCE), "checksum": "b" * 64}
        git_old = package("transport", source="git+https://example.invalid/transport#" + "a" * 40)
        git_new = {**git_old, "source": git_old["source"].replace("a" * 40, "b" * 40)}
        def external(row):
            return [row, outside] + ([git_old if row == old else git_new] if git_move else [])
        def lock(row):
            return ('version=4\n[[package]]\nname="app"\nversion="1.0.0"\ndependencies=["digest"]\n'
                    + ''.join(f'[[package]]\nname="{p["name"]}"\nversion="{p["version"]}"\nsource="{p["source"]}"\nchecksum="{p["checksum"]}"\n' for p in external(row))).encode()
        def metadata(row):
            return {"packages": [{**package("app"), "id": "app", "source": None},
                                 {**row, "id": "digest"}, {**outside, "id": "outside"}],
                    "resolve": {"nodes": [
                        {"id": "app", "features": [], "deps": [{"name": "digest", "pkg": "digest", "dep_kinds": [{"kind": None}]}]},
                        {"id": "digest", "features": [], "deps": []},
                        {"id": "outside", "features": [], "deps": []}]}}
        old_meta, new_meta = metadata(old), metadata(new)
        distribution = root / "core/distribution"
        distribution.mkdir(parents=True)
        (root / "core/Cargo.toml").write_text('[workspace]\nmembers=[]\n')
        (distribution / "shipped-core-features.txt").write_text("")
        (distribution / "inventory.toml").write_text('[[entry]]\nkind="bin"\npackage="app"\ntargets=["windows-x86_64"]\n')
        (root / refresh.LOCK_RELATIVE_PATH).write_bytes(lock(new))
        vendor = root / "vendor"
        for row, source_files in [(old, old_files), (outside, {"Cargo.toml": b'[package]\nname="outside"\nversion="1.0.0"\n', "lib.rs": b"unchanged source"})]:
            directory = vendor / f'{row["name"]}-{row["version"]}'
            directory.mkdir(parents=True)
            for name, content in source_files.items():
                p = directory / name
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_bytes(content)
            (directory / ".cargo-checksum.json").write_text(json.dumps({"package": row["checksum"], "files": {n: refresh.sha256_bytes(b) for n, b in source_files.items()}}))
        rows, members, texts = refresh.added_package_rows([old], {(old["name"], old["version"]): {"license": expression}}, vendor, frozenset({("digest", "1.0.0")}))
        notices, spans = refresh.render_notices(texts)
        (root / refresh.NOTICES_RELATIVE_PATH).write_bytes(notices)
        archive = root / "prior.tar.gz"
        archive_members = {"Cargo.lock": lock(old), refresh.GRAPH_MEMBER_NAME: json.dumps({"graph": refresh.normalize_graph(old_meta)}).encode(), **members}
        for p in (vendor / "outside-1.0.0").iterdir():
            archive_members[f"vendor/outside-1.0.0/{p.name}"] = p.read_bytes()
        with tarfile.open(archive, "w:gz") as tar:
            for name, content in archive_members.items():
                member = tarfile.TarInfo(name)
                member.size = len(content)
                tar.addfile(member, io.BytesIO(content))
        index = {"packages": rows + [{**outside, "identity": f'outside@1.0.0 ({SOURCE})', "windows_notice_population": False}],
                 "notices_sha256": refresh.sha256_bytes(notices), "notice_texts": spans,
                 "cargo_lock_sha256": refresh.sha256_bytes(lock(old)),
                 "population": {"source_sha256": refresh.external_population_sha256(external(old)), "source_package_count": len(external(old)), "windows_notice_package_count": 1},
                 "dependency_source_companion": {"filename": archive.name, "bytes": archive.stat().st_size, "sha256": refresh.sha256_file(archive)}}
        (root / refresh.INDEX_RELATIVE_PATH).write_text(json.dumps(index))
        return archive, new_meta, data, archive_members

    def run_refresh(self, root, archive, metadata, data, **kwargs):
        original = refresh.upgraded_package_rows
        fetches = []
        def fetch(url):
            fetches.append(url)
            return 200, data
        with patch.object(refresh, "query_cargo_metadata", return_value=metadata), \
             patch.object(refresh, "run_cargo_vendor", side_effect=AssertionError("full vendor forbidden")), \
             patch.object(refresh, "upgraded_package_rows", side_effect=lambda *args: original(*args, fetch=fetch)):
            result = refresh.refresh(root, archive, None, root / "out", **kwargs)
        self.assertEqual(len(fetches), 1)
        return result

    def test_incremental_refresh_keeps_surviving_members_and_notices_identical(self):
        for expression in ("MIT", "CDLA-Permissive-2.0"):
            with self.subTest(expression=expression), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                archive, metadata, data, prior = self.fixture(root, expression)
                before = (root / refresh.NOTICES_RELATIVE_PATH).read_bytes()
                result = self.run_refresh(root, archive, metadata, data)
                self.assertIsNone(result["new_notices"])
                self.assertEqual(result["new_index"]["notices_sha256"], refresh.sha256_bytes(before))
                self.assertEqual(result["new_index"]["packages"][0]["version"], "1.0.1")
                with tarfile.open(root / "out" / result["report"]["new_archive"]["filename"]) as tar:
                    observed = {m.name: tar.extractfile(m).read() for m in tar if m.isfile()}
                survivors = {n: b for n, b in prior.items() if n.startswith("vendor/outside-")}
                self.assertTrue(survivors)
                self.assertEqual({n: observed[n] for n in survivors}, survivors)
                self.assertFalse(any(n.startswith("vendor/digest-1.0.0/") for n in observed))
                self.assertIn("vendor/digest-1.0.1/.cargo-checksum.json", observed)

    def test_licence_and_acquisition_refusals_leave_published_inputs_untouched(self):
        for changes in ({"new_expression": "ISC"}, {"new_text": b"changed\n"},
                        {"expression": "GPL-3.0-only"}, {"expression": "unknown"},
                        {"extra": {"native/NOTICE": b"new terms"}},
                        {"extra": {"Cargo.toml": b'[package]\nname="digest"\nversion="1.0.1"\nlicense="MIT"\nlicense-file="missing"\n'}}):
            with self.subTest(changes=changes), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                archive, metadata, data, _ = self.fixture(root, **changes)
                paths = [root / refresh.INDEX_RELATIVE_PATH, root / refresh.NOTICES_RELATIVE_PATH]
                before = [p.read_bytes() for p in paths]
                with self.assertRaises(refresh.RefreshError):
                    self.run_refresh(root, archive, metadata, data)
                self.assertEqual([p.read_bytes() for p in paths], before)
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive, metadata, data, _ = self.fixture(root)
            with self.assertRaisesRegex(refresh.RefreshError, "checksum"):
                self.run_refresh(root, archive, metadata, data + b"corrupt")

    def test_sequential_verifier_still_rejects_corrupt_surviving_sources(self):
        for corruption in ("source", "missing-null-checksum"):
            with self.subTest(corruption=corruption), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                archive, metadata, data, members = self.fixture(root)
                if corruption == "source":
                    members["vendor/outside-1.0.0/lib.rs"] = b"corrupt source"
                else:
                    name = "vendor/outside-1.0.0/.cargo-checksum.json"
                    checks = json.loads(members[name])
                    checks["files"]["missing.rs"] = None
                    members[name] = json.dumps(checks).encode()
                with tarfile.open(archive, "w:gz") as tar:
                    for name, content in members.items():
                        member = tarfile.TarInfo(name)
                        member.size = len(content)
                        tar.addfile(member, io.BytesIO(content))
                path = root / refresh.INDEX_RELATIVE_PATH
                index = json.loads(path.read_text())
                index["dependency_source_companion"].update(bytes=archive.stat().st_size, sha256=refresh.sha256_file(archive))
                path.write_text(json.dumps(index))
                with self.assertRaisesRegex(refresh.RefreshError, "failed its checksum"):
                    self.run_refresh(root, archive, metadata, data)

    def test_upgrades_cannot_mix_a_git_pin_move_or_closure_review(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive, metadata, data, _ = self.fixture(root)
            with self.assertRaisesRegex(refresh.RefreshError, "cannot mix"):
                self.run_refresh(root, archive, metadata, data, promotions=frozenset({("outside", "1.0.0")}))
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            archive, metadata, data, _ = self.fixture(root, git_move=True)
            with self.assertRaisesRegex(refresh.RefreshError, "cannot mix git moves"):
                self.run_refresh(root, archive, metadata, data)

    def test_archive_duplicate_paths_links_and_traversal_refuse(self):
        for name, kind in (("digest-1.0.1/../escaped", tarfile.REGTYPE),
                           ("digest-1.0.1/link", tarfile.SYMTYPE),
                           ("digest-1.0.1/Cargo.toml", tarfile.REGTYPE)):
            with self.subTest(name=name, kind=kind), tempfile.TemporaryDirectory() as tmp:
                stream = io.BytesIO()
                with tarfile.open(fileobj=stream, mode="w:gz") as tar:
                    for entry in ["digest-1.0.1/Cargo.toml", name]:
                        member = tarfile.TarInfo(entry)
                        member.type = kind if entry == name else tarfile.REGTYPE
                        tar.addfile(member, io.BytesIO(b""))
                data = stream.getvalue()
                with self.assertRaises(refresh.RefreshError):
                    refresh.acquire_registry_upgrade(self.row("1.0.1", refresh.sha256_bytes(data)), Path(tmp), lambda _: (200, data))


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
