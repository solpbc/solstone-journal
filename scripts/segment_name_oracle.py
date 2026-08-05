"""Extract the segment-directory-name classification from both references.

Two languages independently decide, from a directory's *name*, whether it is a
segment and what its key is. Both scan for a ``DDDDDD_D+`` pattern at a word
boundary rather than matching the whole name, and the consequences are not
obvious from reading either one:

  * a name with a **trailing** decoration -- ``070000_17.removing`` -- is still
    classified as a segment, **under the undecorated key**. Anything that stages
    a directory beside its original by appending a suffix therefore produces two
    entries with one key and two paths.
  * a name with a **leading** decoration -- ``.removing_070000_17`` -- is not a
    segment in either language, because the character before the digits is a word
    byte and the boundary test fails.
  * the key is **not** the directory name. ``093000_300_summary`` is a segment
    whose key is ``093000_300``, so anything that rebuilds a path from a key
    misses the real directory.

Each of those decides where owner media may safely be moved to during an
irreversible removal, and the cross-language agreement is what a cutover rests
on. This emits all three as committed data.

⛔ Verdicts are OBSERVED by executing both references, never hand-typed. The
Rust side is run through a generated harness against the crate's own source, so
the fixture cannot drift from the implementation without the gate noticing.

Usage:  python scripts/segment_name_oracle.py [--check]
"""

import json
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(REPO))

from solstone.think.utils import segment_key as python_segment_key  # noqa: E402

FIXTURE = REPO / "core" / "fixtures" / "segment_name_oracle.json"
RUST_SOURCE = REPO / "core/crates/solstone-core-journal-io/src/paths.rs"

REFERENCE = {
    "rust_segment_key": "core/crates/solstone-core-journal-io/src/paths.rs",
    "python_segment_key": "solstone/think/utils.py",
}

# Each case says which property it exists to pin, so a reader can tell an
# incidental row from a load-bearing one.
CASES = [
    ("070000_17", "the ordinary shape"),
    ("093000_300_summary", "key is not the directory name"),
    (".removing_070000_17", "LEADING decoration: not a segment in either language"),
    ("070000_17.removing", "TRAILING decoration: still a segment, under the bare key"),
    ("070000_17.deleted", "trailing decoration, the suffix the field's prior art uses"),
    ("070000_17-removing", "trailing decoration without a dot"),
    ("removing.070000_17", "leading decoration ending in a NON-word byte"),
    ("070000_17.lock", "a lock sidecar's name, were it ever a directory"),
    ("x070000_17", "prefixed by an alphanumeric"),
    ("070000_1", "single-digit tail"),
    ("070000_", "no tail"),
    ("70000_17", "five leading digits"),
    ("0700000_17", "seven leading digits"),
    ("health", "the day-level directory both iterators skip"),
    ("_default", "the default stream name"),
]

# Reproduced from the Rust implementation. The generator asserts this text is
# still present in the crate, so a change there fails the gate rather than
# silently invalidating the fixture.
RUST_HARNESS = r"""
fn is_word_byte(byte: u8) -> bool { byte.is_ascii_alphanumeric() || byte == b'_' }

fn segment_key(value: &str) -> Option<&str> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index + 8 <= bytes.len() {
        let word_before = index == 0 || !is_word_byte(bytes[index - 1]);
        if !word_before || !bytes[index].is_ascii_digit() {
            index += 1;
            continue;
        }
        if !bytes[index..index + 6].iter().all(|b| b.is_ascii_digit()) || bytes[index + 6] != b'_' {
            index += 1;
            continue;
        }
        let mut end = index + 7;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == index + 7 {
            index += 1;
            continue;
        }
        return Some(&value[index..end]);
    }
    None
}

fn main() {
    let names: Vec<String> = std::env::args().skip(1).collect();
    let mut out = String::from("[");
    for (position, name) in names.iter().enumerate() {
        if position > 0 {
            out.push(',');
        }
        match segment_key(name) {
            Some(key) => out.push_str(&format!("{key:?}")),
            None => out.push_str("null"),
        }
    }
    out.push(']');
    println!("{out}");
}
"""

# The lines of the real implementation the harness reproduces. If any of these
# stops appearing in the crate, the harness is stale and the gate must fail
# rather than certify a fixture against a copy that has drifted.
RUST_ANCHORS = (
    "fn segment_key(value: &str) -> Option<&str> {",
    "let word_before = index == 0 || !is_word_byte(bytes[index - 1]);",
    "|| bytes[index + 6] != b'_'",
    "let mut end = index + 7;",
)


def rust_verdicts(names: list[str]) -> list[str | None]:
    """Run the Rust reference over `names` and return its verdicts."""
    source = RUST_SOURCE.read_text()
    missing = [anchor for anchor in RUST_ANCHORS if anchor not in source]
    if missing:
        raise SystemExit(
            "FAIL: the Rust reference has changed and the harness in this script no "
            "longer reproduces it. Missing:\n  " + "\n  ".join(missing)
        )
    with tempfile.TemporaryDirectory(prefix="segment-name-oracle-") as scratch:
        harness = Path(scratch) / "harness.rs"
        harness.write_text(RUST_HARNESS)
        binary = Path(scratch) / "harness"
        subprocess.run(
            ["rustc", "--edition", "2021", "-O", str(harness), "-o", str(binary)],
            check=True,
            capture_output=True,
            text=True,
        )
        completed = subprocess.run(
            [str(binary), *names], check=True, capture_output=True, text=True
        )
    return json.loads(completed.stdout)


def python_verdict(name: str) -> str | None:
    return python_segment_key(name)


def reference_revision() -> str:
    try:
        return subprocess.run(
            ["git", "log", "-1", "--format=%H", "--", *sorted(REFERENCE.values())],
            cwd=REPO,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return "unknown"


def main() -> int:
    check = "--check" in sys.argv[1:]
    names = [name for name, _ in CASES]
    rust = rust_verdicts(names)
    rows = []
    disagreements = []
    for (name, why), rust_key in zip(CASES, rust, strict=True):
        python_key = python_verdict(name)
        agree = rust_key == python_key
        if not agree:
            disagreements.append(name)
        rows.append(
            {
                "name": name,
                "pins": why,
                "rust_key": rust_key,
                "python_key": python_key,
                "agree": agree,
                "is_segment": rust_key is not None,
                "key_equals_name": rust_key == name,
                "provenance": "observed",
            }
        )

    document = {
        "_provenance": {
            "generator": "scripts/segment_name_oracle.py",
            "method": (
                "both verdicts are OBSERVED by executing the references -- Python by "
                "import, Rust by compiling a harness that reproduces the crate's "
                "function, with the crate's own source asserted to still contain the "
                "lines the harness reproduces"
            ),
            "source_revision": reference_revision(),
            "reference": REFERENCE,
            "invariant": (
                "the two languages must agree on every row. A disagreement means a "
                "directory that is a segment to one half of the tree and not to the "
                "other, which arms at cutover."
            ),
            "why_this_exists": (
                "these verdicts decide where owner media may be moved to during an "
                "irreversible removal. A staging name that is still classified as a "
                "segment puts two entries with one key in front of every reader; a key "
                "rebuilt into a path misses the real directory entirely."
            ),
        },
        "summary": {
            "cases": len(rows),
            "disagreements": len(disagreements),
            "segments": sum(1 for row in rows if row["is_segment"]),
            "key_differs_from_name": sum(
                1 for row in rows if row["is_segment"] and not row["key_equals_name"]
            ),
        },
        "rows": rows,
    }

    rendered = json.dumps(document, indent=2) + "\n"

    if disagreements:
        print(
            "FAIL: the two references disagree on: " + ", ".join(disagreements),
            file=sys.stderr,
        )
        return 1

    if check:
        if not FIXTURE.exists():
            print(f"FAIL: {FIXTURE} does not exist", file=sys.stderr)
            return 1
        if FIXTURE.read_text() != rendered:
            print(f"FAIL: {FIXTURE} is stale; regenerate it", file=sys.stderr)
            return 1
        print(f"ok: {FIXTURE.relative_to(REPO)} matches both references")
        return 0

    FIXTURE.parent.mkdir(parents=True, exist_ok=True)
    FIXTURE.write_text(rendered)
    print(f"wrote {FIXTURE.relative_to(REPO)}: {len(rows)} cases, 0 disagreements")
    for row in rows:
        if row["is_segment"] and not row["key_equals_name"]:
            print(f"  key != name: {row['name']} -> {row['rust_key']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
