import hashlib
import json
import pathlib
import plistlib
import re
import shutil
import subprocess
import tempfile
import unittest


# run-ci.sh (the support checkout's) runs this Python suite only after swift test exits. That ordering is
# load-bearing: this test's child make process must own SwiftPM's .build lock itself.
REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
MAKEFILE = REPO_ROOT / "Makefile"
SOURCE_PROVENANCE = REPO_ROOT / "Sources/journal/Resources/runtime-entry-candidate-provenance.json"
SOURCE_INFO_PLIST = REPO_ROOT / "Sources/journal/Info.plist"
JOURNAL_APP = REPO_ROOT / "journal.app"


def parse_makefile():
    blocks = {}
    current = None
    target_re = re.compile(r"^([A-Za-z0-9_.-]+):(.*)$")

    for line in MAKEFILE.read_text(encoding="utf-8").splitlines():
        match = target_re.match(line)
        if match and not line.startswith("."):
            current = blocks[match.group(1)] = {
                "prereqs": match.group(2),
                "recipe": [],
            }
            continue

        if current is not None and line.startswith("\t"):
            current["recipe"].append(line)

    return blocks


def remove_path(path):
    if path.is_symlink() or path.is_file():
        path.unlink()
    elif path.is_dir():
        shutil.rmtree(path)


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class JournalProvenanceBundleTest(unittest.TestCase):
    def setUp(self):
        self.temporary_directory = tempfile.TemporaryDirectory(
            prefix="journal-provenance-bundle-",
            dir="/var/tmp",
        )
        self.saved_journal_app = pathlib.Path(self.temporary_directory.name) / "journal.app"
        self.had_journal_app = JOURNAL_APP.exists() or JOURNAL_APP.is_symlink()
        if self.had_journal_app:
            shutil.move(str(JOURNAL_APP), str(self.saved_journal_app))

    def tearDown(self):
        remove_path(JOURNAL_APP)
        if self.had_journal_app:
            shutil.move(str(self.saved_journal_app), str(JOURNAL_APP))
        self.temporary_directory.cleanup()

    def test_source_provenance_target_matches_source_app_identity(self):
        with SOURCE_INFO_PLIST.open("rb") as handle:
            info = plistlib.load(handle)
        provenance = json.loads(SOURCE_PROVENANCE.read_text(encoding="utf-8"))

        self.assertEqual(
            provenance["target"],
            {
                "bundle_identifier": info["CFBundleIdentifier"],
                "bundle_short_version": info["CFBundleShortVersionString"],
                "bundle_version": info["CFBundleVersion"],
            },
        )

    def test_unsigned_assembly_refreshes_apple_products_and_copies_provenance(self):
        products = pathlib.Path(subprocess.check_output(
            ["swift", "build", "-c", "release", "--arch", "arm64", "--arch", "arm64", "--show-bin-path"],
            cwd=REPO_ROOT,
            text=True,
        ).strip())
        self.assertTrue(products.is_absolute())
        self.assertTrue(products.resolve().is_relative_to((REPO_ROOT / ".build").resolve()))
        for name in ("journal", "solstone-watchdog", "SolstoneJournalMac_journal.bundle", "solstone_JournalMarkKit.bundle"):
            remove_path(products / name)

        result = subprocess.run(
            ["make", "journal-app-unsigned"],
            cwd=REPO_ROOT,
            text=True,
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

        journal = products / "journal"
        watchdog = products / "solstone-watchdog"
        product_provenance = (
            products
            / "SolstoneJournalMac_journal.bundle/Contents/Resources/Resources/runtime-entry-candidate-provenance.json"
        )
        assembled_provenance = (
            JOURNAL_APP
            / "Contents/Resources/SolstoneJournalMac_journal.bundle/Contents/Resources/Resources/runtime-entry-candidate-provenance.json"
        )

        self.assertTrue(journal.is_file())
        self.assertTrue(watchdog.is_file())
        self.assertTrue(product_provenance.is_file())
        self.assertTrue(assembled_provenance.is_file())
        self.assertEqual(
            sha256(SOURCE_PROVENANCE),
            sha256(product_provenance),
        )
        self.assertEqual(
            sha256(SOURCE_PROVENANCE),
            sha256(assembled_provenance),
        )

        blocks = parse_makefile()
        journal_bundle_copy = (
            '\t@cp -r "$(SWIFT_PRODUCTS_DIR)/SolstoneJournalMac_journal.bundle" '
            "journal.app/Contents/Resources/"
        )
        mark_bundle_copy = (
            '\t@cp -r "$(SWIFT_PRODUCTS_DIR)/solstone_JournalMarkKit.bundle" '
            "journal.app/Contents/Resources/"
        )
        makefile_text = MAKEFILE.read_text(encoding="utf-8")
        assembly_recipe = "\n".join(blocks["assemble-journal-products"]["recipe"])
        distribution_recipe = "\n".join(blocks["bundle-dist-journal"]["recipe"])

        self.assertEqual(makefile_text.count(journal_bundle_copy), 1)
        self.assertEqual(makefile_text.count(mark_bundle_copy), 1)
        self.assertIn(journal_bundle_copy, assembly_recipe)
        self.assertIn(mark_bundle_copy, assembly_recipe)
        self.assertNotIn(journal_bundle_copy, distribution_recipe)
        self.assertNotIn(mark_bundle_copy, distribution_recipe)
        self.assertEqual(blocks["journal-app-unsigned"]["recipe"], [])
        self.assertEqual(
            blocks["journal-app-unsigned"]["prereqs"].split(),
            ["assemble-journal-app"],
        )

        prohibited = {"unlock-signing", "signing-check", "journal-native-runtime"}
        self.assertTrue(prohibited.isdisjoint(self.transitive_prerequisites(
            "journal-app-unsigned",
            blocks,
        )))

    def transitive_prerequisites(self, target, blocks):
        visited = set()

        def visit(name):
            if name in visited:
                return
            visited.add(name)
            for prerequisite in blocks.get(name, {}).get("prereqs", "").split():
                visit(prerequisite)

        visit(target)
        return visited


if __name__ == "__main__":
    unittest.main()
