# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

"""CLI interface for speaker voiceprint management.

Speaker writer commands preview by default; pass ``--commit`` to persist.
For ``attribute-segment``, ``--save`` / ``--accumulate`` only take effect
when ``--commit`` is also passed.

Commands:
    sol call speakers status [section]
    sol call speakers bootstrap [--commit] [--json]
    sol call speakers resolve-names [--commit] [--json]
    sol call speakers attribute-segment <day> <stream> <segment> [--commit] [--json]
    sol call speakers backfill [--commit] [--json]
    sol call speakers backfill-last-seen [--commit] [--json]
    sol call speakers wipe [--commit] [--json]
    sol call speakers discover [--json]
    sol call speakers identify <cluster-id> <name> [--entity-id ID]
    sol call speakers merge-names <alias> <canonical>
    sol call speakers link-import <name> --entity-id <ID>
    sol call speakers seed-from-imports [--commit] [--json]
    sol call speakers suggest [--limit N] [--json]
    sol call speakers detect [--json]
    sol call speakers confirm-owner [--backfill] [--json]
    sol call speakers reject-owner
    sol call speakers owner-ready
"""

from __future__ import annotations

import typer

from solstone.think.utils import require_solstone

app = typer.Typer(
    name="speakers",
    help="Speaker voiceprint management.",
    no_args_is_help=True,
)


@app.callback()
def _require_up() -> None:
    require_solstone()


@app.command("status")
def status(
    section: str | None = typer.Argument(
        None,
        help=(
            "Section to show (embeddings, owner, speakers, clusters, imports, "
            "attribution). Omit for all."
        ),
    ),
) -> None:
    """Show speaker subsystem status as JSON."""
    import json as json_mod

    from solstone.apps.speakers.status import get_speakers_status

    result = get_speakers_status(section=section)
    typer.echo(json_mod.dumps(result, indent=2, default=str))


@app.command("bootstrap")
def bootstrap(
    commit: bool = typer.Option(
        False,
        "--commit",
        help="Persist results. Without this flag the command only reports what would happen.",
    ),
    json_output: bool = typer.Option(
        False, "--json", help="Output full result as JSON."
    ),
) -> None:
    """Bootstrap voiceprints from single-speaker segments.

    Scans the full journal for segments where speakers.json lists exactly
    one speaker. In those segments, all non-owner embeddings belong to that
    speaker. Saves them as voiceprints using the owner centroid for
    owner subtraction.
    """
    from solstone.apps.speakers.bootstrap import bootstrap_voiceprints

    if not commit and not json_output:
        typer.echo("REPORT ONLY — pass --commit to persist.\n")

    if not json_output:
        typer.echo("Bootstrapping voiceprints from single-speaker segments...")
    stats = bootstrap_voiceprints(dry_run=not commit)

    if "error" in stats:
        typer.echo(f"Error: {stats['error']}", err=True)
        raise typer.Exit(1)
    if json_output:
        import json as json_mod

        typer.echo(json_mod.dumps(stats, indent=2, default=str))
        return

    typer.echo(f"\nSegments scanned: {stats['segments_scanned']}")
    typer.echo(f"Single-speaker segments: {stats['single_speaker_segments']}")
    typer.echo(f"Unique speakers: {len(stats['speakers_found'])}")
    typer.echo(f"Entities created: {stats['entities_created']}")
    typer.echo(f"Embeddings saved: {stats['embeddings_saved']}")
    typer.echo(f"Embeddings skipped (owner): {stats['embeddings_skipped_owner']}")
    typer.echo(
        f"Embeddings skipped (duplicate): {stats['embeddings_skipped_duplicate']}"
    )

    if stats["speakers_found"]:
        typer.echo("\nTop speakers by embedding count:")
        sorted_speakers = sorted(
            stats["speakers_found"].items(), key=lambda x: x[1], reverse=True
        )
        for name, count in sorted_speakers[:15]:
            typer.echo(f"  {name}: {count}")
        if len(sorted_speakers) > 15:
            typer.echo(f"  ... and {len(sorted_speakers) - 15} more")

    if stats["errors"]:
        typer.echo(f"\nErrors ({len(stats['errors'])}):", err=True)
        for err in stats["errors"]:
            typer.echo(f"  {err}", err=True)


@app.command("resolve-names")
def resolve_names(
    commit: bool = typer.Option(
        False,
        "--commit",
        help="Persist results. Without this flag the command only reports what would happen.",
    ),
    json_output: bool = typer.Option(
        False, "--json", help="Output full result as JSON."
    ),
) -> None:
    """Resolve speaker name variants using voiceprint similarity.

    Compares voiceprint centroids between all entities. Pairs with cosine
    similarity > 0.90 are flagged as the same person. Unambiguous variants
    (short name is first word of full name) are auto-merged by adding the
    short name as an aka on the canonical entity.
    """
    from solstone.apps.speakers.bootstrap import resolve_name_variants

    if not commit and not json_output:
        typer.echo("REPORT ONLY — pass --commit to persist.\n")

    if not json_output:
        typer.echo("Resolving speaker name variants...")
    stats = resolve_name_variants(dry_run=not commit)

    if json_output:
        import json as json_mod

        typer.echo(json_mod.dumps(stats, indent=2, default=str))
        return

    typer.echo(f"\nEntities with voiceprints: {stats['entities_with_voiceprints']}")
    typer.echo(f"Pairs compared: {stats['pairs_compared']}")
    typer.echo(f"High-similarity pairs: {len(stats['matches_found'])}")

    if stats["auto_merged"]:
        typer.echo(f"\nAuto-merged ({len(stats['auto_merged'])}):")
        for merge in stats["auto_merged"]:
            typer.echo(
                f"  {merge['alias']} -> {merge['canonical']} ({merge['similarity']})"
            )

    if stats["ambiguous"]:
        typer.echo(f"\nAmbiguous ({len(stats['ambiguous'])}):")
        for amb in stats["ambiguous"]:
            candidates = ", ".join(
                f"{c['name']} ({c['similarity']})" for c in amb["candidates"]
            )
            typer.echo(f"  {amb['name']}: {candidates}")

    if stats["errors"]:
        typer.echo(f"\nErrors ({len(stats['errors'])}):", err=True)
        for err in stats["errors"]:
            typer.echo(f"  {err}", err=True)


@app.command("attribute-segment")
def attribute_segment_cmd(
    day: str = typer.Argument(..., help="Day in YYYYMMDD format."),
    stream: str = typer.Argument(..., help="Stream name."),
    segment: str = typer.Argument(..., help="Segment key (HHMMSS_LEN)."),
    commit: bool = typer.Option(
        False, "--commit", help="Persist speaker labels and voiceprint accumulation."
    ),
    save: bool = typer.Option(
        True, "--save/--no-save", help="Write speaker_labels.json."
    ),
    accumulate: bool = typer.Option(
        True,
        "--accumulate/--no-accumulate",
        help="Run voiceprint accumulation.",
    ),
    json_output: bool = typer.Option(
        False, "--json", help="Output full result as JSON."
    ),
) -> None:
    """Run speaker attribution (Layers 1-3) on a single segment.

    Classifies each sentence using owner detection, structural heuristics,
    and acoustic voiceprint matching.  Optionally writes speaker_labels.json
    and accumulates high-confidence voiceprints.
    """
    import json as json_mod

    from solstone.apps.speakers.attribution import (
        accumulate_voiceprints,
        attribute_segment,
        save_speaker_labels,
    )
    from solstone.think.utils import segment_path

    result = attribute_segment(day, stream, segment)

    if not commit and not json_output:
        typer.echo("REPORT ONLY — pass --commit to persist.\n")

    if result.get("error"):
        typer.echo(f"Error: {result['error']}", err=True)
        raise typer.Exit(1)

    labels = result.get("labels", [])
    unmatched = result.get("unmatched", [])
    source = result.get("source")
    metadata = result.get("metadata", {})

    if json_output:
        typer.echo(json_mod.dumps(result, indent=2))
    else:
        resolved = sum(1 for lab in labels if lab["speaker"] is not None)
        typer.echo(f"Sentences: {len(labels)}")
        typer.echo(f"Resolved:  {resolved}")
        typer.echo(f"Unmatched: {len(unmatched)}")

        methods: dict[str, int] = {}
        for lab in labels:
            m = lab.get("method") or "unmatched"
            methods[m] = methods.get(m, 0) + 1
        typer.echo("\nBy method:")
        for method, count in sorted(methods.items()):
            typer.echo(f"  {method}: {count}")

    if commit and save:
        seg_dir = segment_path(day, segment, stream)
        out_path = save_speaker_labels(seg_dir, labels, metadata)
        if not json_output:
            typer.echo(f"\nWrote: {out_path}")

    if commit and accumulate and source:
        saved = accumulate_voiceprints(day, stream, segment, labels, source)
        if saved and not json_output:
            typer.echo("\nAccumulated voiceprints:")
            for eid, count in saved.items():
                typer.echo(f"  {eid}: {count} embeddings")


@app.command("backfill")
def backfill(
    commit: bool = typer.Option(
        False,
        "--commit",
        help="Persist results. Without this flag the command only reports what would happen.",
    ),
    json_output: bool = typer.Option(
        False, "--json", help="Output full result as JSON."
    ),
) -> None:
    """Run speaker attribution across all segments with embeddings.

    Processes segments oldest-first for progressive voiceprint building.
    Skips segments that already have speaker_labels.json (safe to re-run).
    """
    import time

    from solstone.apps.speakers.attribution import backfill_segments

    if not commit and not json_output:
        typer.echo("REPORT ONLY — pass --commit to persist.\n")

    if not json_output:
        typer.echo("Scanning journal for segments with embeddings...")

    start = time.monotonic()
    last_day = ""

    def on_progress(
        processed: int, total: int, day: str, stream: str, seg_key: str
    ) -> None:
        nonlocal last_day
        if day != last_day:
            typer.echo(f"\n  {day} ", nl=False)
            last_day = day
        typer.echo(".", nl=False)
        if processed % 100 == 0 or processed == total:
            typer.echo(f" [{processed}/{total}]", nl=False)

    stats = backfill_segments(
        dry_run=not commit,
        progress_callback=None if not commit or json_output else on_progress,
    )

    elapsed = time.monotonic() - start

    if json_output:
        import json as json_mod

        typer.echo(json_mod.dumps(stats, indent=2, default=str))
        return

    typer.echo("\n")
    typer.echo(f"Total segments scanned:    {stats['total_segments']}")
    typer.echo(f"With embeddings:           {stats['total_eligible']}")
    typer.echo(f"Without embeddings:        {stats['skipped_no_embed']}")
    typer.echo(f"Already labeled (skipped): {stats['already_labeled']}")
    typer.echo(f"Processed this run:        {stats['processed']}")
    typer.echo(f"Elapsed:                   {elapsed:.1f}s")

    speakers = stats.get("speakers_seen", {})
    if speakers:
        typer.echo(f"\nSpeakers identified ({len(speakers)}):")
        sorted_speakers = sorted(speakers.items(), key=lambda x: x[1], reverse=True)
        for eid, count in sorted_speakers[:20]:
            typer.echo(f"  {eid}: {count} attributions")
        if len(sorted_speakers) > 20:
            typer.echo(f"  ... and {len(sorted_speakers) - 20} more")

    if stats["errors"]:
        typer.echo(f"\nErrors ({len(stats['errors'])}):", err=True)
        for err in stats["errors"][:10]:
            typer.echo(f"  {err}", err=True)
        if len(stats["errors"]) > 10:
            typer.echo(f"  ... and {len(stats['errors']) - 10} more", err=True)


@app.command("backfill-last-seen")
def backfill_last_seen_cmd(
    commit: bool = typer.Option(
        False,
        "--commit",
        help="Persist results. Without this flag the command only reports what would happen.",
    ),
    json_output: bool = typer.Option(
        False, "--json", help="Output full result as JSON."
    ),
) -> None:
    """Backfill last_seen_ts on existing voiceprint metadata rows."""
    from solstone.apps.speakers.attribution import backfill_last_seen

    if not commit and not json_output:
        typer.echo("REPORT ONLY — pass --commit to persist.\n")

    stats = backfill_last_seen(dry_run=not commit)

    if json_output:
        import json as json_mod

        typer.echo(json_mod.dumps(stats, indent=2, default=str))
        return

    typer.echo(f"Speaker label files read: {stats['labels_read']}")
    typer.echo(f"Entities seen:            {stats['entities_seen']}")
    typer.echo(f"Voiceprint rows scanned:  {stats['rows_scanned']}")
    typer.echo(f"Rows pending:             {stats['rows_pending']}")
    typer.echo(f"Rows written:             {stats['rows_written']}")

    pending = stats.get("pending", {})
    if pending:
        typer.echo("\nPending by entity:")
        for entity_id, item in pending.items():
            typer.echo(f"  {entity_id}: {item['rows']}")

    if stats.get("errors"):
        typer.echo("\nErrors:", err=True)
        for error in stats["errors"]:
            typer.echo(f"  {error}", err=True)


@app.command()
def wipe(
    commit: bool = typer.Option(
        False,
        "--commit",
        help="Actually delete files. Without this flag the command only reports what would happen.",
    ),
    json_output: bool = typer.Option(
        False, "--json", help="Output full result as JSON."
    ),
) -> None:
    """Remove all legacy speaker artifacts from the journal (DESTRUCTIVE).

    DESTRUCTIVE. Without --commit, prints a report of what would be
    removed. With --commit, permanently deletes segment-embedding NPZs,
    speaker labels/corrections, per-entity voiceprints, owner centroids,
    and the owner-candidate snapshot.
    """
    import json as json_mod

    from solstone.apps.speakers.wipe import wipe_speaker_artifacts

    if not commit and not json_output:
        typer.echo("REPORT ONLY — pass --commit to persist.\n")

    report = wipe_speaker_artifacts(dry_run=not commit)

    if json_output:
        typer.echo(json_mod.dumps(report.to_dict(), indent=2, default=str))
        return

    typer.echo(
        f"segment_embeddings : {report.segment_embeddings.count} files "
        f"({report.segment_embeddings.bytes} B)"
    )
    typer.echo(
        f"speaker_labels     : {report.speaker_labels.count} files "
        f"({report.speaker_labels.bytes} B)"
    )
    typer.echo(
        f"speaker_corrections: {report.speaker_corrections.count} files "
        f"({report.speaker_corrections.bytes} B)"
    )
    typer.echo(
        f"entity_voiceprints : {report.entity_voiceprints.count} files "
        f"({report.entity_voiceprints.bytes} B)"
    )
    typer.echo(
        f"owner_centroids    : {report.owner_centroids.count} files "
        f"({report.owner_centroids.bytes} B)"
    )
    typer.echo(
        f"owner_candidate    : {report.owner_candidate.count} files "
        f"({report.owner_candidate.bytes} B)"
    )
    typer.echo(
        f"total              : {report.total_files} files ({report.total_bytes} B)"
    )


@app.command()
def discover(
    json_output: bool = typer.Option(
        False, "--json", help="Output full result as JSON."
    ),
) -> None:
    """Discover recurring unknown speakers across segments."""
    import json as json_mod

    from solstone.apps.speakers.discovery import discover_unknown_speakers

    result = discover_unknown_speakers()
    if json_output:
        typer.echo(json_mod.dumps(result, indent=2, default=str))
        return
    clusters = result.get("clusters", [])

    if not clusters:
        typer.echo("No recurring unknown speakers found.")
        raise typer.Exit()

    typer.echo(f"Found {len(clusters)} unknown speaker cluster(s):\n")
    for cluster in clusters:
        typer.echo(
            f"  Cluster {cluster['cluster_id']}: "
            f"{cluster['size']} samples across {cluster['segment_count']} segments"
        )
        for sample in cluster.get("samples", []):
            text_preview = (sample.get("text") or "")[:60]
            typer.echo(
                f"    - {sample['day']}/{sample['stream']}/{sample['segment_key']} "
                f"sid={sample['sentence_id']}: {text_preview}"
            )
        typer.echo()


@app.command()
def identify(
    cluster_id: int = typer.Argument(..., help="Cluster ID from discovery output."),
    name: str = typer.Argument(..., help="Speaker name to assign."),
    entity_id: str | None = typer.Option(
        None, "--entity-id", help="Link to existing entity ID instead of name matching."
    ),
) -> None:
    """Identify a discovered unknown speaker cluster."""
    import json

    from solstone.apps.speakers.discovery import identify_cluster

    result = identify_cluster(cluster_id, name, entity_id=entity_id)
    output = json.dumps(result, indent=2, default=str)
    if "error" in result:
        typer.echo(output, err=True)
        raise typer.Exit(1)
    typer.echo(output)


@app.command("merge-names")
def merge_names_cmd(
    alias: str = typer.Argument(..., help="Alias/variant speaker name to merge from."),
    canonical: str = typer.Argument(..., help="Canonical speaker name to merge into."),
) -> None:
    """Merge a speaker name variant into a canonical entity."""
    import json

    from solstone.apps.speakers.bootstrap import merge_names

    result = merge_names(alias, canonical)
    output = json.dumps(result, indent=2, default=str)
    if "error" in result:
        typer.echo(output, err=True)
        raise typer.Exit(1)
    typer.echo(output)


@app.command("link-import")
def link_import_cmd(
    name: str = typer.Argument(..., help="Import participant name to link."),
    entity_id: str = typer.Option(..., "--entity-id", help="Entity ID to link to."),
) -> None:
    """Link an import participant name as an aka on an existing entity."""
    import json

    from solstone.apps.speakers.bootstrap import link_import

    result = link_import(name, entity_id)
    output = json.dumps(result, indent=2, default=str)
    if "error" in result:
        typer.echo(output, err=True)
        raise typer.Exit(1)
    typer.echo(output)


@app.command("seed-from-imports")
def seed_from_imports_cmd(
    commit: bool = typer.Option(
        False,
        "--commit",
        help="Persist results. Without this flag the command only reports what would happen.",
    ),
    json_output: bool = typer.Option(
        False, "--json", help="Output full result as JSON."
    ),
) -> None:
    """Seed voiceprints from import segments with speaker-attributed transcripts.

    Scans import streams for segments with both conversation_transcript.jsonl
    (with speaker labels) and audio embeddings. Maps each embedding to a speaker
    via time-based alignment, matches speakers to existing entities, and saves
    embeddings as voiceprints with owner contamination guard.
    """
    from solstone.apps.speakers.bootstrap import seed_from_imports

    if not commit and not json_output:
        typer.echo("REPORT ONLY — pass --commit to persist.\n")

    if not json_output:
        typer.echo("Seeding voiceprints from import segments...")
    stats = seed_from_imports(dry_run=not commit)

    if "error" in stats:
        typer.echo(f"Error: {stats['error']}", err=True)
        raise typer.Exit(1)
    if json_output:
        import json as json_mod

        typer.echo(json_mod.dumps(stats, indent=2, default=str))
        return

    typer.echo(f"\nSegments scanned: {stats['segments_scanned']}")
    typer.echo(f"Segments with speakers: {stats['segments_with_speakers']}")
    typer.echo(f"Unique speakers: {len(stats['speakers_found'])}")
    typer.echo(f"Embeddings saved: {stats['embeddings_saved']}")
    typer.echo(f"Embeddings skipped (owner): {stats['embeddings_skipped_owner']}")
    typer.echo(
        f"Embeddings skipped (duplicate): {stats['embeddings_skipped_duplicate']}"
    )

    if stats["speakers_found"]:
        typer.echo("\nSpeakers by embedding count:")
        sorted_speakers = sorted(
            stats["speakers_found"].items(), key=lambda x: x[1], reverse=True
        )
        for name, count in sorted_speakers[:15]:
            typer.echo(f"  {name}: {count}")

    if stats["speakers_unmatched"]:
        typer.echo(f"\nUnmatched speakers ({len(stats['speakers_unmatched'])}):")
        for name in stats["speakers_unmatched"]:
            typer.echo(f"  {name}")

    if stats["errors"]:
        typer.echo(f"\nErrors ({len(stats['errors'])}):", err=True)
        for err in stats["errors"]:
            typer.echo(f"  {err}", err=True)


@app.command()
def suggest(
    limit: int = typer.Option(
        5, "--limit", "-n", help="Maximum suggestions to return."
    ),
    json_output: bool = typer.Option(False, "--json", help="Output as JSON array."),
) -> None:
    """Suggest speaker curation opportunities."""
    import json as json_mod

    from solstone.apps.speakers.suggest import suggest_opportunities

    results = suggest_opportunities(limit=limit)
    if json_output:
        typer.echo(json_mod.dumps(results, indent=2, default=str))
        return

    from solstone.apps.speakers.suggest import format_suggestions

    typer.echo(format_suggestions(results))


@app.command("detect")
def detect_cmd(
    json_output: bool = typer.Option(False, "--json", help="Output as JSON."),
) -> None:
    """Run owner voice candidate detection."""
    import json as json_mod

    from solstone.apps.speakers.owner import detect_owner_candidate

    result = detect_owner_candidate()
    typer.echo(json_mod.dumps(result, indent=2, default=str))


@app.command("confirm-owner")
def confirm_owner_cmd(
    backfill_after: bool = typer.Option(
        True,
        "--backfill/--no-backfill",
        help="Run attribution backfill after confirming.",
    ),
    json_output: bool = typer.Option(False, "--json", help="Output as JSON."),
) -> None:
    """Confirm the owner voice candidate and save the centroid.

    By default, automatically runs attribution backfill on all segments
    after saving the centroid.
    """
    import json as json_mod

    from solstone.apps.speakers.owner import confirm_owner_candidate

    result = confirm_owner_candidate()
    if "error" in result:
        typer.echo(json_mod.dumps(result, indent=2), err=True)
        raise typer.Exit(1)

    if not json_output:
        typer.echo(
            f"Owner centroid confirmed (principal: {result['principal_id']}, "
            f"cluster_size: {result['cluster_size']})"
        )

    if backfill_after:
        from solstone.apps.speakers.attribution import backfill_segments

        if not json_output:
            typer.echo("Running attribution backfill...")

        stats = backfill_segments(dry_run=False)

        if json_output:
            result["backfill"] = stats
        else:
            typer.echo(
                f"Backfill complete: {stats['processed']} segments processed, "
                f"{stats['already_labeled']} already labeled"
            )

    if json_output:
        typer.echo(json_mod.dumps(result, indent=2, default=str))


@app.command("reject-owner")
def reject_owner_cmd() -> None:
    """Reject the owner voice candidate and enter 14-day cooldown."""
    import json as json_mod

    from solstone.apps.speakers.owner import reject_owner_candidate

    result = reject_owner_candidate()
    typer.echo(json_mod.dumps(result, indent=2, default=str))


@app.command("owner-ready")
def owner_ready_cmd() -> None:
    """Check if owner voice detection should be surfaced to the user."""
    import json as json_mod

    from solstone.think.awareness import owner_detection_ready

    result = owner_detection_ready()
    typer.echo(json_mod.dumps(result, indent=2, default=str))
