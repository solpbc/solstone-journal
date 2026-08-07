# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

from __future__ import annotations

import json
import os
import shlex
import socket
import sqlite3
import subprocess
import sys
from pathlib import Path

import pytest

from tests import verify_indexer_differential as harness

try:
    from tests._indexer_differential_fixtures import (
        FULLTEXT_QUERY_CASES,
        FULLTEXT_TOP10_JACCARD_MIN,
        METADATA_FILTER_CASES,
    )
except ModuleNotFoundError:
    from _indexer_differential_fixtures import (
        FULLTEXT_QUERY_CASES,
        FULLTEXT_TOP10_JACCARD_MIN,
        METADATA_FILTER_CASES,
    )

FIXTURE_JOURNAL = Path("tests/fixtures/journal").resolve()
FUNCTIONAL_PATHS = tuple(f"docs/p{i:02}.md" for i in range(10))


def _quote_command(*parts: str | Path) -> str:
    return " ".join(shlex.quote(str(part)) for part in parts)


def _writer_script(tmp_path: Path) -> Path:
    script = tmp_path / "write_index.py"
    script.write_text(
        """
import os
import sqlite3
import sys
from pathlib import Path

mode = sys.argv[1]
if mode == "fail":
    sys.exit(7)
if mode == "missing":
    sys.exit(0)

journal = Path(os.environ["SOLSTONE_JOURNAL"])
db = journal / "indexer" / "journal.sqlite"
db.parent.mkdir(parents=True, exist_ok=True)
conn = sqlite3.connect(db)
conn.execute("CREATE TABLE files(path TEXT PRIMARY KEY, mtime INTEGER)")
conn.execute(\"\"\"
CREATE VIRTUAL TABLE chunks USING fts5(
    content,
    path UNINDEXED,
    day UNINDEXED,
    facet UNINDEXED,
    agent UNINDEXED,
    stream UNINDEXED,
    idx UNINDEXED,
    time_bucket UNINDEXED
)
\"\"\")
conn.execute("CREATE TABLE edge_files(path TEXT PRIMARY KEY, mtime INTEGER)")
conn.execute(\"\"\"
CREATE TABLE edges(
    src TEXT NOT NULL,
    dst TEXT NOT NULL,
    kind TEXT NOT NULL,
    directed INTEGER NOT NULL,
    src_name TEXT,
    dst_name TEXT,
    day TEXT,
    facet TEXT,
    source TEXT NOT NULL,
    path TEXT NOT NULL,
    anchor TEXT,
    label TEXT,
    ts INTEGER,
    weight INTEGER NOT NULL
)
\"\"\")
if mode != "empty":
    mtime = 20 if mode == "mtime2" else 10
    content = "different token" if mode == "different" else "same token"
    conn.execute(
        "INSERT INTO files(path, mtime) VALUES (?, ?)",
        ("entity_search:__mtime__", mtime),
    )
    conn.execute(
        "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) "
        "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        (content, "source.md", "20260101", "test", "test", "", 0, ""),
    )
conn.commit()
conn.close()
""".lstrip(),
        encoding="utf-8",
    )
    return script


def _command(tmp_path: Path, mode: str) -> str:
    return _quote_command(sys.executable, _writer_script(tmp_path), mode)


def _journal_indexer_command(tmp_path: Path) -> str:
    del tmp_path
    return _quote_command(
        harness.ROOT / "core" / "target" / "debug" / "solstone-core",
        "indexer",
        "--rescan-full",
    )


def _tree_inventory(root: Path) -> dict[str, tuple[int, int]]:
    return {
        path.relative_to(root).as_posix(): (stat.st_size, stat.st_mtime_ns)
        for path in sorted(root.rglob("*"))
        for stat in [path.lstat()]
    }


def _create_index_db(path: Path, *, content: str = "same token") -> None:
    script = _writer_script(path.parent)
    env = os.environ.copy()
    journal = path.parent / f"{path.stem}_journal"
    env["SOLSTONE_JOURNAL"] = str(journal)
    subprocess.run(
        [sys.executable, str(script), "same"],
        env=env,
        check=True,
        capture_output=True,
        text=True,
    )
    db = journal / "indexer" / "journal.sqlite"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(db.read_bytes())
    if content != "same token":
        with sqlite3.connect(path) as conn:
            conn.execute("DELETE FROM chunks")
            conn.execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) "
                "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                (content, "source.md", "20260101", "test", "test", "", 0, ""),
            )
            conn.commit()


def _functional_schema(conn: sqlite3.Connection) -> None:
    conn.execute("CREATE TABLE files(path TEXT PRIMARY KEY, mtime INTEGER)")
    conn.execute(
        """
        CREATE VIRTUAL TABLE chunks USING fts5(
            content,
            path UNINDEXED,
            day UNINDEXED,
            facet UNINDEXED,
            agent UNINDEXED,
            stream UNINDEXED,
            idx UNINDEXED,
            time_bucket UNINDEXED
        )
        """
    )
    conn.execute("CREATE TABLE edge_files(path TEXT PRIMARY KEY, mtime INTEGER)")
    conn.execute(
        """
        CREATE TABLE edges(
            src TEXT NOT NULL,
            dst TEXT NOT NULL,
            kind TEXT NOT NULL,
            directed INTEGER NOT NULL,
            src_name TEXT,
            dst_name TEXT,
            day TEXT,
            facet TEXT,
            source TEXT NOT NULL,
            path TEXT NOT NULL,
            anchor TEXT,
            label TEXT,
            ts INTEGER,
            weight INTEGER NOT NULL
        )
        """
    )


def _functional_content(path: str, *, jwt: bool = True, suffix: str = "") -> str:
    phrase = " JWT token" if jwt else ""
    return f"authentication module FastAPI{phrase} common {path} {suffix}".strip()


def _functional_rows(
    paths: tuple[str, ...] = FUNCTIONAL_PATHS,
    *,
    idx_offset: int = 0,
    suffix: str = "",
    stream: str | None = "default",
) -> list[dict[str, object]]:
    return [
        {
            "content": _functional_content(path, suffix=suffix),
            "path": path,
            "day": "20240102",
            "facet": "work",
            "agent": "news",
            "stream": stream,
            "idx": idx + idx_offset,
            "time_bucket": "morning",
        }
        for idx, path in enumerate(paths)
    ]


def _functional_edges() -> list[dict[str, object]]:
    return [
        {
            "src": "alice",
            "dst": "bob",
            "kind": "works-with",
            "directed": 0,
            "src_name": "Alice",
            "dst_name": "Bob",
            "day": "20240102",
            "facet": "work",
            "source": "test",
            "path": "edges.json",
            "anchor": "a1",
            "label": "Alice works with Bob",
            "ts": 1,
            "weight": 4,
        },
        {
            "src": "alice",
            "dst": "project",
            "kind": "committed-to",
            "directed": 1,
            "src_name": "Alice",
            "dst_name": "Project",
            "day": "20240102",
            "facet": "work",
            "source": "test",
            "path": "edges.json",
            "anchor": "a2",
            "label": "Alice committed to Project",
            "ts": 2,
            "weight": 5,
        },
    ]


def _write_functional_db(
    path: Path,
    *,
    rows: list[dict[str, object]] | None = None,
    edges: list[dict[str, object]] | None = None,
    file_paths: tuple[str, ...] = FUNCTIONAL_PATHS,
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(path) as conn:
        _functional_schema(conn)
        for file_path in file_paths:
            conn.execute(
                "INSERT INTO files(path, mtime) VALUES (?, ?)",
                (file_path, 10),
            )
        conn.executemany(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            [
                (
                    row["content"],
                    row["path"],
                    row["day"],
                    row["facet"],
                    row["agent"],
                    row["stream"],
                    row["idx"],
                    row["time_bucket"],
                )
                for row in (rows if rows is not None else _functional_rows())
            ],
        )
        conn.execute(
            "INSERT INTO edge_files(path, mtime) VALUES (?, ?)",
            ("edges.json", 10),
        )
        conn.executemany(
            """
            INSERT INTO edges(
                src, dst, kind, directed, src_name, dst_name, day, facet,
                source, path, anchor, label, ts, weight
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            [
                (
                    edge["src"],
                    edge["dst"],
                    edge["kind"],
                    edge["directed"],
                    edge["src_name"],
                    edge["dst_name"],
                    edge["day"],
                    edge["facet"],
                    edge["source"],
                    edge["path"],
                    edge["anchor"],
                    edge["label"],
                    edge["ts"],
                    edge["weight"],
                )
                for edge in (edges if edges is not None else _functional_edges())
            ],
        )
        conn.commit()


def _compare_functional_paths(tmp_path: Path, left: Path, right: Path) -> dict:
    return harness.compare_functional(left, right, tmp_path / "functional")


def _fixture_index(tmp_path: Path) -> Path:
    journal = tmp_path / "indexed-journal"
    harness.copytree_tracked(FIXTURE_JOURNAL, journal)
    os.environ["SOLSTONE_JOURNAL"] = str(journal)
    import solstone.think.utils as think_utils

    think_utils._journal_path_cache = None
    from solstone.think.indexer.journal import scan_journal

    scan_journal(str(journal), full=True)
    return journal


def test_stderr_classifier_allows_standalone_edge_skip_warning() -> None:
    stderr = "\n".join(
        [
            f"{harness.EDGE_SKIP_PREFIX}20240102/default/234567_300/screen.jsonl"
            ": invalid segment key 234567_300",
            "unexpected diagnostic",
        ]
    )

    classified = harness.classify_stderr(stderr)

    assert classified["rules"][0]["count"] == 1
    assert classified["unclassified"] == ["unexpected diagnostic"]


def test_stderr_classifier_allows_markdown_sanitize_warning() -> None:
    stderr = "\n".join(
        [
            "WARNING:solstone.think.markdown:Dropped 1 line(s) exceeding 2048 chars during markdown sanitization",
            "WARNING:solstone.think.markdown:Dropped 5 line(s) exceeding 2048 chars during markdown sanitization",
        ]
    )
    classified = harness.classify_stderr(stderr)
    markdown_rule = next(
        rule
        for rule in classified["rules"]
        if rule["name"] == harness.MARKDOWN_SANITIZE_RULE
    )
    assert markdown_rule["count"] == 2
    assert markdown_rule["examples"] == stderr.splitlines()
    assert classified["unclassified"] == []


def test_stderr_classifier_allows_native_markdown_sanitize_warning() -> None:
    stderr = "\n".join(
        [
            "warning: Dropped 1 line(s) exceeding 2048 chars during markdown sanitization",
            "warning: Dropped 5 line(s) exceeding 2048 chars during markdown sanitization",
        ]
    )
    classified = harness.classify_stderr(stderr)
    native_markdown_rule = next(
        rule
        for rule in classified["rules"]
        if rule["name"] == harness.NATIVE_MARKDOWN_SANITIZE_RULE
    )
    assert native_markdown_rule["count"] == 2
    assert native_markdown_rule["examples"] == stderr.splitlines()
    assert classified["unclassified"] == []


def test_stderr_classifier_allows_native_edge_skip_warning() -> None:
    stderr = "\n".join(
        [
            f"{harness.NATIVE_EDGE_SKIP_PREFIX}"
            "20240102/default/234567_300/screen.jsonl"
            ": invalid segment key 234567_300",
            "unexpected diagnostic",
        ]
    )

    classified = harness.classify_stderr(stderr)
    native_edge_rule = next(
        rule
        for rule in classified["rules"]
        if rule["name"] == harness.NATIVE_EDGE_SKIP_RULE
    )

    assert native_edge_rule["count"] == 1
    assert classified["unclassified"] == ["unexpected diagnostic"]


def test_stderr_classifier_rejects_markdown_near_misses() -> None:
    stderr = "\n".join(
        [
            "ERROR:solstone.think.markdown:Dropped 1 line(s) exceeding 2048 chars during markdown sanitization",
            "WARNING:solstone.think.other:Dropped 1 line(s) exceeding 2048 chars during markdown sanitization",
            "WARNING:solstone.think.markdown:Some unrelated warning",
            "warning: Dropped 1 line(s) exceeding 4096 chars during markdown sanitization",
            "warning: dropped 1 line(s) exceeding 2048 chars during markdown sanitization",
            "WARNING:some.other.module:generic warning",
        ]
    )
    classified = harness.classify_stderr(stderr)
    markdown_rule = next(
        rule
        for rule in classified["rules"]
        if rule["name"] == harness.MARKDOWN_SANITIZE_RULE
    )
    assert markdown_rule["count"] == 0
    assert classified["unclassified"] == stderr.splitlines()


def test_stderr_classifier_mixed_run_fails_closed() -> None:
    mixed = "\n".join(
        [
            "WARNING:solstone.think.markdown:Dropped 1 line(s) exceeding 2048 chars during markdown sanitization",
            "unexpected diagnostic",
        ]
    )
    classified = harness.classify_stderr(mixed)
    markdown_rule = next(
        rule
        for rule in classified["rules"]
        if rule["name"] == harness.MARKDOWN_SANITIZE_RULE
    )
    assert markdown_rule["count"] == 1
    assert classified["unclassified"] == ["unexpected diagnostic"]

    command = {
        "id": "left",
        "exit_code": 0,
        "stderr_classification": classified,
        "checks": {
            "exit": {"status": "ok"},
            "database": {"status": "ok"},
            "stderr": {
                "status": "unclassified" if classified["unclassified"] else "ok"
            },
        },
    }
    failure = harness._failure_for_commands([command])
    assert failure == {
        "class": "stderr_unclassified",
        "command_id": "left",
        "count": 1,
    }


def test_runner_prepares_tracked_clean_copies_with_equal_mtimes(tmp_path: Path) -> None:
    copies = harness._prepare_working_copies(FIXTURE_JOURNAL, tmp_path / "work")

    assert set(copies) == {"left", "right"}
    assert not (copies["left"] / harness.DB_REL).exists()
    assert not (copies["right"] / harness.DB_REL).exists()
    assert harness._mtime_mismatches(copies["left"], copies["right"]) == []


def test_runner_sets_journal_env_and_captures_exit_codes(tmp_path: Path) -> None:
    report = harness.run_differential(
        journal=FIXTURE_JOURNAL,
        command_a=_command(tmp_path, "same"),
        command_b=_command(tmp_path, "same"),
        work_root=tmp_path / "work",
    )

    assert report["classification"] == "equal"
    assert [command["exit_code"] for command in report["commands"]] == [0, 0]
    assert report["commands"][0]["journal"] != report["commands"][1]["journal"]
    assert all(
        command["checks"]["database"]["status"] == "ok"
        for command in report["commands"]
    )


def test_missing_database_is_failed_not_equal(tmp_path: Path) -> None:
    report = harness.run_differential(
        journal=FIXTURE_JOURNAL,
        command_a=_command(tmp_path, "missing"),
        command_b=_command(tmp_path, "missing"),
        work_root=tmp_path / "work",
    )

    assert report["classification"] == "failed"
    assert report["failure"]["class"] == "db_missing"


def test_empty_database_is_failed_not_equal(tmp_path: Path) -> None:
    report = harness.run_differential(
        journal=FIXTURE_JOURNAL,
        command_a=_command(tmp_path, "empty"),
        command_b=_command(tmp_path, "empty"),
        work_root=tmp_path / "work",
    )

    assert report["classification"] == "failed"
    assert report["failure"]["class"] == "db_empty"


def test_wal_representation_invariance_canonicalizes_equal(tmp_path: Path) -> None:
    left = tmp_path / "left.sqlite"
    right = tmp_path / "right.sqlite"
    _create_index_db(right)

    conn = sqlite3.connect(left)
    try:
        conn.execute("PRAGMA journal_mode=WAL")
        conn.execute("CREATE TABLE files(path TEXT PRIMARY KEY, mtime INTEGER)")
        conn.execute(
            """
            CREATE VIRTUAL TABLE chunks USING fts5(
                content,
                path UNINDEXED,
                day UNINDEXED,
                facet UNINDEXED,
                agent UNINDEXED,
                stream UNINDEXED,
                idx UNINDEXED,
                time_bucket UNINDEXED
            )
            """
        )
        conn.execute("CREATE TABLE edge_files(path TEXT PRIMARY KEY, mtime INTEGER)")
        conn.execute(
            """
            CREATE TABLE edges(
                src TEXT NOT NULL,
                dst TEXT NOT NULL,
                kind TEXT NOT NULL,
                directed INTEGER NOT NULL,
                src_name TEXT,
                dst_name TEXT,
                day TEXT,
                facet TEXT,
                source TEXT NOT NULL,
                path TEXT NOT NULL,
                anchor TEXT,
                label TEXT,
                ts INTEGER,
                weight INTEGER NOT NULL
            )
            """
        )
        conn.execute(
            "INSERT INTO files(path, mtime) VALUES (?, ?)",
            ("entity_search:__mtime__", 10),
        )
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) "
            "VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            ("same token", "source.md", "20260101", "test", "test", "", 0, ""),
        )
        conn.commit()

        assert left.read_bytes() != right.read_bytes()
        _normalized, comparison = harness.canonicalize_pair(
            left, right, tmp_path / "scratch"
        )
    finally:
        conn.close()

    assert comparison["classification"] == "equal"


def test_shadow_table_only_change_still_equal(tmp_path: Path) -> None:
    left = tmp_path / "left.sqlite"
    right = tmp_path / "right.sqlite"
    _create_index_db(left)
    _create_index_db(right)
    with sqlite3.connect(right) as conn:
        conn.execute("INSERT INTO chunks(chunks, rank) VALUES('automerge', 2)")
        conn.commit()

    assert left.read_bytes() != right.read_bytes()
    _normalized, comparison = harness.canonicalize_pair(
        left, right, tmp_path / "scratch"
    )

    assert comparison["classification"] == "equal"


def test_seeded_divergence_is_unexpected_differs_and_cli_nonzero(
    tmp_path: Path,
    capsys,
) -> None:
    exit_code = harness.main(
        [
            "--journal",
            str(FIXTURE_JOURNAL),
            "--a",
            _command(tmp_path, "same"),
            "--b",
            _command(tmp_path, "different"),
            "--work-dir",
            str(tmp_path / "work"),
        ]
    )

    report = json.loads(capsys.readouterr().out)
    assert exit_code != 0
    assert report["classification"] == "unexpected-differs"


def test_mtime_only_divergence_is_functionally_equal(tmp_path: Path) -> None:
    report = harness.run_differential(
        journal=FIXTURE_JOURNAL,
        command_a=_command(tmp_path, "same"),
        command_b=_command(tmp_path, "mtime2"),
        work_root=tmp_path / "work",
    )

    assert report["classification"] == "functionally-equal"
    assert [rule["name"] for rule in report["normalization"]["rules_fired"]] == [
        harness.ENTITY_SEARCH_MTIME_RULE
    ]


def test_command_failure_is_distinct_and_cli_nonzero(tmp_path: Path, capsys) -> None:
    exit_code = harness.main(
        [
            "--journal",
            str(FIXTURE_JOURNAL),
            "--a",
            _command(tmp_path, "same"),
            "--b",
            _command(tmp_path, "fail"),
            "--work-dir",
            str(tmp_path / "work"),
        ]
    )

    report = json.loads(capsys.readouterr().out)
    assert exit_code != 0
    assert report["classification"] == "failed"
    assert report["failure"]["class"] == "command_nonzero"
    assert report["failure"]["command_id"] == "right"


def test_fixture_corpus_reports_equal_with_visible_edge_skips(tmp_path: Path) -> None:
    command = _journal_indexer_command(tmp_path)

    report = harness.run_differential(
        journal=FIXTURE_JOURNAL,
        command_a=command,
        command_b=command,
        work_root=tmp_path / "work",
    )

    table_counts = {
        table["name"]: table["row_counts"]["left"]
        for table in report["canonical"]["tables"]
    }
    skip_counts = [
        next(
            rule["count"]
            for rule in command_report["stderr_classification"]["rules"]
            if rule["name"] == harness.NATIVE_EDGE_SKIP_RULE
        )
        for command_report in report["commands"]
    ]
    corpus = report["provenance"]["corpus"]

    assert report["classification"] == "equal"
    assert report["normalization"]["rules_fired"] == []
    assert corpus["copy_route"] == "git-archive-head"
    assert corpus["identity"]["repo_commit"]
    assert table_counts == {
        "files": 176,
        "chunks": 590,
        "edge_files": 101,
        "edges": 30,
    }
    assert skip_counts == [1, 1]


def test_functional_byte_different_but_equivalent(tmp_path: Path) -> None:
    left = tmp_path / "left.sqlite"
    right = tmp_path / "right.sqlite"
    _write_functional_db(left, rows=_functional_rows())
    _write_functional_db(
        right,
        rows=_functional_rows(idx_offset=100, suffix="right"),
    )

    assert left.read_bytes() != right.read_bytes()
    comparison = _compare_functional_paths(tmp_path, left, right)

    assert comparison["classification"] == "functionally-equal"
    assert comparison["functional"]["failed_components"] == []


def test_functional_missing_coverage_tuple_reports_missing(tmp_path: Path) -> None:
    left = tmp_path / "left.sqlite"
    right = tmp_path / "right.sqlite"
    rows = [row for row in _functional_rows() if row["path"] != "docs/p09.md"]
    _write_functional_db(left)
    _write_functional_db(right, rows=rows)

    comparison = _compare_functional_paths(tmp_path, left, right)
    coverage = comparison["functional"]["chunk_coverage"]

    assert comparison["classification"] == "unexpected-differs"
    assert "chunk_coverage" in comparison["functional"]["failed_components"]
    assert {
        "path": "docs/p09.md",
        "day": "20240102",
        "facet": "work",
        "agent": "news",
        "stream": "default",
        "time_bucket": "morning",
    } in coverage["missing"]


def test_functional_fulltext_overlap_failure_names_query(tmp_path: Path) -> None:
    paths = tuple(f"docs/p{i:02}.md" for i in range(11))
    left_rows = _functional_rows(paths)
    right_rows = _functional_rows(paths)
    for row in left_rows:
        if row["path"] == "docs/p10.md":
            row["content"] = _functional_content(str(row["path"]), jwt=False)
    for row in right_rows:
        if row["path"] == "docs/p09.md":
            row["content"] = _functional_content(str(row["path"]), jwt=False)

    left = tmp_path / "left.sqlite"
    right = tmp_path / "right.sqlite"
    _write_functional_db(left, rows=left_rows, file_paths=paths)
    _write_functional_db(right, rows=right_rows, file_paths=paths)

    comparison = _compare_functional_paths(tmp_path, left, right)
    cases = comparison["functional"]["fulltext"]["cases"]
    quoted = next(case for case in cases if case["name"] == "quoted_jwt_token")

    assert comparison["classification"] == "unexpected-differs"
    assert "fulltext" in comparison["functional"]["failed_components"]
    assert quoted["passed"] is False
    assert quoted["jaccard"] < FULLTEXT_TOP10_JACCARD_MIN
    assert quoted["top3_subset_ok"] == {
        "left_top3_in_right_top10": True,
        "right_top3_in_left_top10": True,
        "both": True,
    }


@pytest.mark.parametrize(
    "edge_variant",
    ("dropped", "relabeled", "new_kind"),
)
def test_functional_edge_differences_are_unexpected(
    tmp_path: Path,
    edge_variant: str,
) -> None:
    left_edges = _functional_edges()
    right_edges = _functional_edges()
    if edge_variant == "dropped":
        right_edges = right_edges[1:]
    elif edge_variant == "relabeled":
        right_edges[0] = {**right_edges[0], "kind": "knows"}
    elif edge_variant == "new_kind":
        right_edges.append({**right_edges[0], "kind": "new-kind", "dst": "carol"})

    left = tmp_path / "left.sqlite"
    right = tmp_path / "right.sqlite"
    _write_functional_db(left, edges=left_edges)
    _write_functional_db(right, edges=right_edges)

    comparison = _compare_functional_paths(tmp_path, left, right)

    assert comparison["classification"] == "unexpected-differs"
    assert "edges" in comparison["functional"]["failed_components"]


def test_functional_metadata_filter_path_set_difference(tmp_path: Path) -> None:
    left = tmp_path / "left.sqlite"
    right = tmp_path / "right.sqlite"
    right_rows = _functional_rows()
    for row in right_rows:
        if row["path"] == "docs/p09.md":
            row["agent"] = "other"
    _write_functional_db(left)
    _write_functional_db(right, rows=right_rows)

    comparison = _compare_functional_paths(tmp_path, left, right)
    filters = comparison["functional"]["metadata_filters"]
    work_news = next(
        case for case in filters["cases"] if case["name"] == "work_news_all"
    )

    assert comparison["classification"] == "unexpected-differs"
    assert "metadata_filters" in comparison["functional"]["failed_components"]
    assert "docs/p09.md" in work_news["only_left"]


def test_functional_null_empty_coverage_representation_is_equal(
    tmp_path: Path,
) -> None:
    left = tmp_path / "left.sqlite"
    right = tmp_path / "right.sqlite"
    _write_functional_db(left, rows=_functional_rows(stream=None))
    _write_functional_db(right, rows=_functional_rows(stream=""))

    comparison = _compare_functional_paths(tmp_path, left, right)

    assert comparison["classification"] == "functionally-equal"
    assert comparison["functional"]["failed_components"] == []


def test_functional_real_facet_difference_is_unexpected(tmp_path: Path) -> None:
    left = tmp_path / "left.sqlite"
    right = tmp_path / "right.sqlite"
    right_rows = _functional_rows()
    for row in right_rows:
        if row["path"] == "docs/p09.md":
            row["facet"] = "personal"
    _write_functional_db(left)
    _write_functional_db(right, rows=right_rows)

    comparison = _compare_functional_paths(tmp_path, left, right)

    assert comparison["classification"] == "unexpected-differs"


def test_functional_fixture_cases_are_non_empty_on_reference_index(
    tmp_path: Path,
) -> None:
    journal = _fixture_index(tmp_path)
    from solstone.think.indexer.journal import search_journal

    for case in (*FULLTEXT_QUERY_CASES, *METADATA_FILTER_CASES):
        with harness._temporary_solstone_journal(journal):
            total, _ = search_journal(
                case["query"],
                limit=0,
                **case["filters"],
            )
            _, results = search_journal(
                case["query"],
                limit=total,
                **case["filters"],
            )
        paths = {result["metadata"]["path"] for result in results}
        assert total > 0, case["name"]
        assert paths, case["name"]
        assert total == case["reference_total"], case["name"]
        assert len(paths) == case["reference_distinct_paths"], case["name"]


def test_fixture_corpus_reports_functionally_equal(tmp_path: Path) -> None:
    command = _journal_indexer_command(tmp_path)

    report = harness.run_differential(
        journal=FIXTURE_JOURNAL,
        command_a=command,
        command_b=command,
        work_root=tmp_path / "work",
        mode="functional",
    )
    functional = report["functional"]

    assert report["mode"] == "functional"
    assert report["classification"] == "functionally-equal"
    assert functional["failed_components"] == []
    assert functional["files"]["equal"] is True
    assert functional["chunk_coverage"]["equal"] is True
    assert functional["metadata_filters"]["passed"] is True
    assert functional["fulltext"]["passed"] is True
    assert functional["edges"]["passed"] is True


def test_full_copy_mode_handles_non_git_journal_and_preserves_source_sqlite(
    tmp_path: Path,
) -> None:
    source = tmp_path / "non-git-journal"
    harness.copytree_tracked(FIXTURE_JOURNAL, source)
    source_sqlite = source / "imports" / "health-dedupe.sqlite"
    source_sqlite.parent.mkdir(parents=True, exist_ok=True)
    source_sqlite.write_bytes(b"source sqlite content")

    copies = harness._prepare_working_copies(
        source,
        tmp_path / "prep-work",
        copy_mode="full",
    )

    assert not (copies["left"] / harness.DB_REL).exists()
    assert not (copies["right"] / harness.DB_REL).exists()
    assert harness._mtime_mismatches(copies["left"], copies["right"]) == []
    assert (copies["left"] / "imports" / "health-dedupe.sqlite").read_bytes() == (
        b"source sqlite content"
    )

    report = harness.run_differential(
        journal=source,
        command_a=_command(tmp_path, "same"),
        command_b=_command(tmp_path, "same"),
        work_root=tmp_path / "run-work",
        copy_mode="full",
    )

    corpus = report["provenance"]["corpus"]
    assert report["classification"] == "equal"
    assert corpus["copy_route"] == "copytree-full"
    assert corpus["copy_mode"] == "full"
    assert corpus["copy_exclusions"] == list(harness.INDEX_DB_EXCLUSION_RELS)
    assert (
        tmp_path / "run-work" / "left" / "journal" / "imports" / "health-dedupe.sqlite"
    ).read_bytes() == b"source sqlite content"


def test_harness_does_not_use_network_or_write_outside_workdir(
    tmp_path: Path,
    monkeypatch,
) -> None:
    private_repo = tmp_path / "private-repo"
    private_corpus = private_repo / "tests" / "fixtures" / "journal"
    harness.copytree_tracked(FIXTURE_JOURNAL, private_corpus)
    subprocess.run(
        ["git", "init", "-q"],
        cwd=private_repo,
        capture_output=True,
        text=True,
        check=True,
    )
    subprocess.run(
        ["git", "add", "."],
        cwd=private_repo,
        capture_output=True,
        text=True,
        check=True,
    )
    before_inventory = _tree_inventory(private_corpus)
    connect_calls: list[tuple[object, object]] = []

    def fail_connect(self: socket.socket, address: object) -> None:
        connect_calls.append((self, address))
        raise AssertionError("network call attempted")

    # This catches in-process harness networking; subprocess networking is out of scope.
    monkeypatch.setattr(socket.socket, "connect", fail_connect)

    report = harness.run_differential(
        journal=private_corpus,
        command_a=_command(tmp_path, "same"),
        command_b=_command(tmp_path, "same"),
        work_root=tmp_path / "work",
    )

    after_inventory = _tree_inventory(private_corpus)

    assert report["classification"] == "equal"
    assert report["provenance"]["corpus"]["copy_route"] == "git-ls-files-live"
    assert report["provenance"]["corpus"]["identity"] is None
    assert connect_calls == []
    assert before_inventory == after_inventory
