# Removal Sites Inventory

Inventory of every non-test, non-scratch, non-atomic-tmp destructive removal (`shutil.rmtree`, `Path.unlink`, `os.remove`, `os.unlink`) in production code.

> Atomic-tmp exclusion heuristic:
> same-directory temp sibling created inside the same function for atomic replacement of one target file, promoted via `os.replace`/`rename`, with `unlink` only in the exception cleanup branch. Do not exclude directory deletes, named domain paths, or rollback deletes of non-temp targets.

## Methodology

- Scope: every non-test, non-scratch, non-atomic-tmp destructive removal (`shutil.rmtree`, `Path.unlink`, `os.remove`, `os.unlink`) in production code.
- Grep command: `rg -n 'shutil\.rmtree|\.unlink\(|os\.remove|os\.unlink' --type py`
- Exclusion filter: `tests/`, `scratch/`, `.venv/`, `tmp/`, `observers/`
- Atomic-tmp exclusion heuristic:

  > same-directory temp sibling created inside the same function for atomic replacement of one target file, promoted via `os.replace`/`rename`, with `unlink` only in the exception cleanup branch. Do not exclude directory deletes, named domain paths, or rollback deletes of non-temp targets.

- Reference model: `solstone/think/retention.py`
  - scope-narrow docstring at `:4-19`
  - deletion gate at `:75-253`
  - per-file stream-hashed SHA-256 at `:563-576`
  - dry-run support at `:478-498`, `:577-579`, `:599`
  - failed-extraction block at `:523-540`
  - retention log at `:624-625`, `:664-683`
- Write-owner table pointer: `CLAUDE.md` / `AGENTS.md` §7 L2
- Importer convention: importers audit destructive operations via `log_app_action(app='import', ...)` per repo convention (`solstone/think/importers/journal_source_cli.py:40, 75, 230, 250`).
- Raw grep noise removed manually: nested app test hit at `solstone/apps/observer/tests/test_routes.py:1008` and regex literals in `scripts/check_layer_hygiene.py:54-55`.

## Classification Legend

- `✅` matches the retention reference model closely enough to serve as the template.
- `⚠️` has partial safety coverage or is intentionally out of scope for this sweep.
- `❌` remains a destructive gap after applying the exclusion heuristic.

## think/retention (reference)

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/think/retention.py:578` | raw media files in gate-eligible segments | retention purge on eligible segments | `resolve_segment_gate()` plus retention-policy eligibility | per-segment `write_prune_audit()` record plus `_write_retention_log()` summary | yes | `✅` | historical reference template for this sweep (superseded — `purge()` removed in the completed raw-media migration; see `solstone/think/offload.py` for the current Python raw-media deletion reference) |

## think/offload

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/think/offload_restore.py:535` | recorded media files in a segment restore attempt | restore-on-demand verification failure after restic exits successfully | `resolve_segment_dir()` without mkdir plus validated offload-ledger basenames; rollback only visits `segment_dir / file.name` entries that are still files | `record_restore_result(status="error", reason=...)` records the failed attempt; verified successes append a durable restore event instead | no | `⚠️` | rollback-only delete returns a backup-only segment to ledger state; bounded to attempted files, never a blanket segment wipe |

## think/log_retention

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/think/log_retention.py:682,684,686` | dated operational log/cache files and dirs from the retention allowlist | operational log/cache pruning | class-specific scanners feed `_delete_target()` only with dated allowlist paths under the journal | `write_prune_audit()` plus structured result errors | yes | `✅` | declared operational-log/cache pruning owner with dry-run and audit outcome surfacing |

## think/entities

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/think/entities/journal.py:344,350` | `facets/*/entities/<id>/` rel dirs and `entities/<id>/` including `history/` | `delete_journal_entity()` | entity must exist, must not be principal, and each target must exist as a directory | yes (route: `solstone/apps/entities/routes.py:910-918`) | no | `⚠️` | helper itself is unaudited, but the production route is audited; deferred follow-up |
| `solstone/think/entities/merge.py:989,994,999,1997` | source facet rel dirs, discovery cache, source entity dir, plus rollback restoration of every touched owner file | `merge_entity(..., commit=True)` | strict preflight loads malformed-sensitive inputs; delete paths come from the fresh merge plan; relative manifest paths are validated with `contained_path()` before use | yes (`solstone/think/entities/merge.py:1265-1289`; audit row includes `merge_id`) | yes (`commit=False`, byte-read-only) | `✅` | strict prepare/apply/publish flow snapshots exact pre-operation bytes, rolls back on phase failure, and no longer writes resume markers or degraded-success audit rows |

## think/importers

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/think/importers/shared.py:361` | existing `imports/<timestamp>/` directory | `_setup_import(..., force=True)` | fixed `journal/imports/<timestamp>` path and `import_dir.exists()` gate | yes (`solstone/think/importers/shared.py:351-357`) | yes | `✅` | fixed in this sweep: per-file manifest is hashed and logged before `rmtree` |
| `solstone/think/importers/plaud.py:212` | temporary download file | Plaud download write failure | exact `NamedTemporaryFile` path created in the same function | no | no | `⚠️` | temp download cleanup, not a journal-domain delete |

## think/facets

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/think/facets.py:907` | `facets/<name>/` directory | `delete_facet()` | facet path resolves under `journal/facets`, with existing-facet checks before delete | yes (`solstone/think/facets.py:899-906`) | no | `⚠️` | audited write-owner delete path; deferred rather than expanded in this design |

## think/identity

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/think/identity.py:350` | newly created identity file | rollback on history-append failure in `_write_identity_locked()` | target is scoped to the locked identity dir and only removed on exception after create | no | no | `⚠️` | rollback delete of a just-created file, not a steady-state delete path |

## think/tools

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/think/tools/call.py:402` | source facet entity dir | facet merge when destination already has the entity | source/dest facets are validated and source dir must exist | yes (`solstone/think/tools/call.py:441-450`) | no | `⚠️` | audited merge flow, but the larger facet-merge bundle is deferred |

## solstone/apps/entities

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/apps/entities/call.py:179` | source facet entity dir | `sol call entities move --merge` when destination already has the entity | source facet, destination facet, entity resolution, and source dir existence are all checked first | yes (`solstone/apps/entities/call.py:184-193`) | no | `⚠️` | audited write-owner CLI path; deferred rather than widened in this sweep |

## solstone/apps/speakers

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/apps/speakers/routes.py:313` | `entities/<id>/voiceprints.npz` | `api_correct_attribution()` when `_remove_voiceprint()` removes the NPZ's last matching row | entity memory path must resolve, NPZ must exist, and the `(day, segment_key, source, sentence_id)` metadata tuple must match before unlink | yes (`voiceprint_removal` records `not_found`, `rewritten`, or `unlinked`) | no | `✅` | fixed in this sweep: audit payload now records every removal outcome without asserting a removal when nothing matched |
| `solstone/apps/speakers/discovery.py:494` | `awareness/discovery_clusters.json` | `identify_unknown_speaker()` completes | fixed awareness cache path | no | no | `⚠️` | cache cleanup after identification; out of scope for this sweep |
| `solstone/apps/speakers/owner.py:419,446` | owner-candidate NPZ | owner candidate confirm/reject flows | fixed candidate path under awareness state | no (state update only at `solstone/apps/speakers/owner.py:421-428,447-452`) | no | `⚠️` | awareness candidate lifecycle cleanup, not a journal-domain delete |

## solstone/apps/transcripts

Out of scope for this sweep; keep visible because it is a destructive journal-domain route owned by a separate transcript bundle.

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/apps/transcripts/routes.py:521` | segment directory under `chronicle/<day>/<stream>/` | `DELETE /api/segment/...` | day regex, segment-key validation, existence check, and `commonpath` containment check | yes (`solstone/apps/transcripts/routes.py:524-529`) | no | `⚠️` | destructive transcript route owned by a separate bundle; tracked but out of scope here |

## solstone/apps/import

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/apps/import/routes.py:224,250` | request temp files used for timestamp detection and staged upload copy | import upload request handling | both paths come from `NamedTemporaryFile` in the same request | no | no | `⚠️` | request-scoped temp cleanup, not persisted journal deletion |
| `solstone/apps/import/call.py:278,279` | staged config diff files | final config-review resolution | fixed paths under the resolved import-review `state_dir` | yes (`solstone/apps/import/call.py:281-290`) | no | `⚠️` | review-state cleanup after explicit operator resolution |
| `solstone/apps/import/call.py:401,437,452` | staged entity review file | merge/create/skip entity review resolution | `staged_path` must exist under `state_dir/entities/staged` | yes (`solstone/apps/import/call.py:402-463`) | no | `⚠️` | review-state cleanup after explicit operator resolution |
| `solstone/apps/import/call.py:507,583,605` | staged facet review file | skip/apply facet review resolution | `staged_path` must exist under `state_dir/facets/staged` | yes (`solstone/apps/import/call.py:508-615`) | no | `⚠️` | review-state cleanup after explicit operator resolution |

## solstone/apps/support

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/apps/support/routes.py:171` | uploaded attachment temp file | support attachment upload completes or fails | exact temp path created for the request | no | no | `⚠️` | request temp cleanup, not journal-domain deletion |

## observe

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/observe/observer_client.py:39` | files inside a draft capture directory | `cleanup_draft()` | iterates only files already inside the draft directory | no | no | `⚠️` | draft temp cleanup on the observe side |
| `core/crates/solstone-core-sense/src/batch.rs:250` | derived output files for a segment | `delete_outputs()` during reprocess cleanup | delete only when the file matches the requested reprocess type and a corresponding source exists | stdout diagnostics | yes | `⚠️` | native `journal sense` cleanup has dry-run support but not retention-style logging |
| `solstone/observe/transcribe/main.py:546,682` | raw/audio capture files that fail VAD thresholds | transcription filtering | delete is gated by VAD outcome on the source file | no (callosum event only) | no | `⚠️` | observe-side source filtering, not part of this journal-domain sweep |
| `solstone/observe/transcribe/whisper.py:231` | temporary audio upload file | Whisper transcription request teardown | exact temp path plus `exists()` check | no | no | `⚠️` | request temp cleanup |

## IPC/health

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/think/callosum.py:59,91` | `health/callosum.sock` | callosum server start/stop | fixed socket path and `exists()` checks | no | no | `⚠️` | IPC socket cleanup is out of scope for journal-domain parity |
| `solstone/think/supervisor.py:870` | `health/callosum.sock` | supervisor pre-start stale-socket cleanup | fixed `server.socket_path` plus `exists()` check | no | no | `⚠️` | IPC socket race prevention, out of scope |
| `solstone/think/heartbeat.py:89,99,125` | heartbeat PID file | stale/corrupt PID cleanup and final teardown | fixed PID path with stale/corrupt guards | no (logger only) | no | `⚠️` | service lifecycle cleanup, not journal-domain deletion |
| `solstone/think/service.py:199,215` | installed service plist/unit file | service uninstall | fixed platform-specific install path and `exists()` check | no | no | `⚠️` | installed-service artifact cleanup, out of scope |
| `solstone/think/install_guard.py:147,168` | owned `sol` alias symlink | install/uninstall guard | alias ownership is checked before unlink | no | no | `⚠️` | user-bin alias management, not journal-domain deletion |

## maint

| file:line | target | trigger | path validation | audit log | dry-run | class | why |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `solstone/apps/observer/maint/000_migrate_remote_to_observer.py:38` | legacy observer source file | one-shot remote-to-observer migration | migration resolves the legacy source path before delete | no | no | `⚠️` | shipped maint migration; one-shot historical cleanup |
| `solstone/apps/settings/maint/002_restructure_stream_dirs.py:122` | legacy segment directory | one-shot stream-dir restructuring migration | delete happens only after migration work on that segment dir | no | no | `⚠️` | shipped maint migration; one-shot historical cleanup |
| `solstone/apps/sol/maint/000_migrate_agent_layout.py:46` | legacy agent layout file | one-shot agent-layout migration | migration resolves the legacy source before unlink | no | no | `⚠️` | shipped maint migration; one-shot historical cleanup |
| `solstone/apps/sol/maint/001_migrate_agent_run_logs.py:92` | legacy agent run-log file | one-shot run-log migration | delete follows successful migration of that log file | no | no | `⚠️` | shipped maint migration; one-shot historical cleanup |
| `solstone/apps/sol/maint/002_migrate_chronicle.py:77,91` | legacy chronicle day dir and legacy SQLite db | one-shot chronicle migration | delete follows successful day/db migration | no | no | `⚠️` | shipped maint migration; one-shot historical cleanup |

## Deferred Follow-ups

- `solstone/apps/entities/call.py:179` — audited write-owner move path; defer to a broader entities deletion parity pass.
- `solstone/think/facets.py:907` — audited write-owner delete path; not a named gap for this sweep.
- `solstone/think/entities/journal.py:369,375` — production route coverage exists, but helper-local parity remains deferred.
- `solstone/think/entities/merge.py:520,536,697,702` — audited, commit-gated merge workflow; too broad for this change.
- `solstone/think/tools/call.py:402` — audited facet-merge flow; broader merge semantics make it a defer.
- No `❌` rows remain after B1 and B2 in this sweep.
