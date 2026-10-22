// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;

use serde_json::{Value, json};
use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};

use crate::dispatch::{McpProbeError, run_mcp_probe};
use crate::permissions::{
    ConnectionReadSnapshot, PermissionDecision, PermissionStore, ReadPermission, ReadScope,
};

const CONNECTION: &str = "bearer:screen-text-test";
const FACET_A: &str = "123e4567-e89b-42d3-a456-426614174000";
const FACET_B: &str = "123e4567-e89b-42d3-a456-426614174001";
const DAY: &str = "20260914";
const STREAM: &str = "default";
const SEGMENT: &str = "090000_300";

fn fixture() -> tempfile::TempDir {
    let journal = tempfile::Builder::new()
        .prefix("solstone-mcp-screentext-")
        .tempdir_in(crate::test_scratch())
        .unwrap();
    for (name, id, title) in [("alpha", FACET_A, "Alpha"), ("beta", FACET_B, "Beta")] {
        let directory = journal.path().join("facets").join(name);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("facet.json"),
            json!({"id": id, "title": title, "description": format!("{title} details")})
                .to_string(),
        )
        .unwrap();
    }
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    fs::create_dir_all(segment.join("talents")).unwrap();
    fs::write(
        segment.join("talents/facets.json"),
        "[{\"facet\":\"alpha\"}]",
    )
    .unwrap();
    journal
}

fn probe(
    journal: &tempfile::TempDir,
    tool: &str,
    arguments: Value,
) -> Result<Value, McpProbeError> {
    run_mcp_probe(journal.path(), CONNECTION, tool, &arguments)
}

#[test]
fn screen_text_only_advertises_search_and_fetch_not_transcripts() {
    let journal = fixture();
    let snapshot = ConnectionReadSnapshot {
        categories: [AdmittedCategory::ScreenText].into_iter().collect(),
        scope: ConnectionScope::WholeJournal,
        generation: 1,
    };
    let tools = crate::registry::advertised_tools_list(
        journal.path(),
        &PermissionDecision::Snapshot(snapshot),
    );
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"search"));
    assert!(names.contains(&"fetch"));
    assert!(!names.contains(&"list_transcripts"));
    assert!(!names.contains(&"get_transcript"));

    let search_tool = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "search")
        .unwrap();
    assert_eq!(
        search_tool["inputSchema"]["properties"]["category"]["enum"],
        json!(["screen_text"])
    );
}

#[test]
fn screen_text_three_category_grant_reveals_nothing_and_limits_chosen_facet() {
    let journal = fixture();
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    fs::write(
        segment.join("talents/screen_text.md"),
        "## 00:12 · terminal\nsecret_screentext_marker\n",
    )
    .unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    // 1. Transcripts + entities + facets grant: reveals no snippet, count, reference, or body
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "transcripts".to_owned(),
                    "entities".to_owned(),
                    "facets".to_owned(),
                ],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let res = probe(
        &journal,
        "search",
        json!({"query": "secret_screentext_marker"}),
    )
    .unwrap();
    assert_eq!(res["results"].as_array().unwrap().len(), 0);

    // 2. Grant screen_text under facet beta: segment is in alpha, so beta reveals nothing
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["screen_text".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_B.to_owned()],
                },
            },
        )
        .unwrap();
    let res_beta = probe(
        &journal,
        "search",
        json!({"query": "secret_screentext_marker"}),
    )
    .unwrap();
    assert_eq!(res_beta["results"].as_array().unwrap().len(), 0);

    // 3. Grant screen_text under facet alpha: reveals the hit
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["screen_text".to_owned()],
                scope: ReadScope::Facets {
                    ids: vec![FACET_A.to_owned()],
                },
            },
        )
        .unwrap();
    let res_alpha = probe(
        &journal,
        "search",
        json!({"query": "secret_screentext_marker"}),
    )
    .unwrap();
    let results = res_alpha["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    let hit = &results[0];
    let reference = hit["reference"].as_str().unwrap().to_owned();

    // Now test fetch under 3-category grant: category refusal
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec![
                    "transcripts".to_owned(),
                    "entities".to_owned(),
                    "facets".to_owned(),
                ],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let fetch_res = probe(&journal, "fetch", json!({"reference": reference}));
    assert!(fetch_res.is_err());
}

#[test]
fn screen_text_forged_reference_and_stale_generation_reveal_nothing() {
    let journal = fixture();
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    fs::write(
        segment.join("talents/screen_text.md"),
        "## 00:12 · terminal\nforge_and_stale_marker\n",
    )
    .unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let store = PermissionStore::open(journal.path());
    store
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["screen_text".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    // Forged reference
    let forged = probe(
        &journal,
        "fetch",
        json!({"reference": "invalid_forged_ref"}),
    );
    assert!(forged.is_err());

    // Search to get a valid minted reference
    let s = probe(
        &journal,
        "search",
        json!({"query": "forge_and_stale_marker"}),
    )
    .unwrap();
    let valid_ref = s["results"][0]["reference"].as_str().unwrap();

    // Valid fetch succeeds
    let valid_fetch = probe(&journal, "fetch", json!({"reference": valid_ref}));
    assert!(valid_fetch.is_ok());

    // Increment permission generation by updating permissions
    store
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["screen_text".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    // Stale generation reference fails closed
    let stale_fetch = probe(&journal, "fetch", json!({"reference": valid_ref}));
    assert!(stale_fetch.is_err());
}

#[test]
fn screen_text_two_groups_search_citations_and_replace_before_reindex() {
    let journal = fixture();
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    let st_path = segment.join("talents/screen_text.md");
    fs::write(
        &st_path,
        "## 00:10 · code\nfirst group with citation marker alpha\n\n## 00:25 · terminal\nsecond group with citation marker beta\n",
    )
    .unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["screen_text".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    // Search group 1
    let s1 = probe(&journal, "search", json!({"query": "marker alpha"})).unwrap();
    let r1 = s1["results"].as_array().unwrap();
    assert_eq!(r1.len(), 1);
    let hit1 = &r1[0];
    assert_eq!(hit1["segment"], SEGMENT);
    assert_eq!(hit1["minute"], "00:10");
    assert!(hit1.get("group_offset_ms").is_none());
    let ref1 = hit1["reference"].as_str().unwrap().to_owned();

    // Search group 2
    let s2 = probe(&journal, "search", json!({"query": "marker beta"})).unwrap();
    let r2 = s2["results"].as_array().unwrap();
    assert_eq!(r2.len(), 1);
    let hit2 = &r2[0];
    assert_eq!(hit2["segment"], SEGMENT);
    assert_eq!(hit2["minute"], "00:25");
    assert!(hit2.get("group_offset_ms").is_none());

    // Replace file on disk before reindex
    fs::write(&st_path, "modified content without reindexing").unwrap();

    // Search drops hit because digest does not match current source
    let s_dropped = probe(&journal, "search", json!({"query": "marker alpha"})).unwrap();
    assert_eq!(s_dropped["results"].as_array().unwrap().len(), 0);

    // Fetch with previously minted reference fails because digest does not match current source
    let f_fail = probe(&journal, "fetch", json!({"reference": ref1}));
    assert!(f_fail.is_err());
}

#[test]
fn screen_text_page_concatenation_and_invalidation() {
    let journal = fixture();
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    let st_path = segment.join("talents/screen_text.md");

    // File with preamble, timed heading, and > 65536 bytes
    let mut original_text = String::from(
        "preamble unheaded lines before any group\nsecond preamble line\n## 00:10 · code\n",
    );
    for i in 0..2000 {
        original_text.push_str(&format!("line {i} repetitive text old_tail_marker_xyz\n"));
    }
    assert!(original_text.len() > 65_536);
    fs::write(&st_path, &original_text).unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let store = PermissionStore::open(journal.path());
    store
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["screen_text".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let s = probe(&journal, "search", json!({"query": "repetitive"})).unwrap();
    let fetch_ref = s["results"][0]["reference"].as_str().unwrap();

    // Page 1
    let p1 = probe(&journal, "fetch", json!({"reference": fetch_ref})).unwrap();
    let t1 = p1["text"].as_str().unwrap();
    assert!(t1.len() <= 65_536);
    assert!(t1.starts_with("preamble unheaded lines"));
    let next_ref = p1["next_reference"].as_str().unwrap();

    // Page 2
    let p2 = probe(&journal, "fetch", json!({"reference": next_ref})).unwrap();
    let t2 = p2["text"].as_str().unwrap();
    assert!(t2.len() <= 65_536);
    assert!(p2.get("next_reference").is_none());
    assert!(t2.contains("old_tail_marker_xyz"));

    // 1. Pages of an unchanged file concatenate to the original, including the preamble
    let concat = format!("{t1}{t2}");
    assert_eq!(concat, original_text);

    // 2. Replace the file, reindex through the normal index path, then the old next_reference
    // contains neither the old tail marker nor the new tail marker.
    let mut replaced_text = String::from("preamble new version\n## 00:20 · terminal\n");
    for i in 0..2000 {
        replaced_text.push_str(&format!("line {i} replaced text new_tail_marker_abc\n"));
    }
    fs::write(&st_path, &replaced_text).unwrap();
    let file = std::fs::File::options().write(true).open(&st_path).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    drop(file);
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    let old_res = probe(&journal, "fetch", json!({"reference": next_ref}));
    assert!(old_res.is_err());
    if let Ok(val) = old_res {
        let txt = val["text"].as_str().unwrap_or_default();
        assert!(!txt.contains("old_tail_marker_xyz"));
        assert!(!txt.contains("new_tail_marker_abc"));
    }

    // New search mints a new pin and can fetch the replacement
    let s_new = probe(&journal, "search", json!({"query": "replaced"})).unwrap();
    let new_fetch_ref = s_new["results"][0]["reference"].as_str().unwrap();
    let p1_new = probe(&journal, "fetch", json!({"reference": new_fetch_ref})).unwrap();
    let next_ref_new = p1_new["next_reference"].as_str().unwrap();
    let p2_new = probe(&journal, "fetch", json!({"reference": next_ref_new})).unwrap();
    assert!(
        p2_new["text"]
            .as_str()
            .unwrap()
            .contains("new_tail_marker_abc")
    );

    // 3. Delete the file between pages: no further bytes
    fs::remove_file(&st_path).unwrap();
    let del_res = probe(&journal, "fetch", json!({"reference": next_ref_new}));
    assert!(del_res.is_err());

    // Restore file and reindex for grant narrowing tests
    fs::write(&st_path, &replaced_text).unwrap();
    let file = std::fs::File::options().write(true).open(&st_path).unwrap();
    file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(10))
        .unwrap();
    drop(file);
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    let s_restored = probe(&journal, "search", json!({"query": "replaced"})).unwrap();
    let restored_ref = s_restored["results"][0]["reference"].as_str().unwrap();
    let p_restored = probe(&journal, "fetch", json!({"reference": restored_ref})).unwrap();
    let valid_next_ref = p_restored["next_reference"].as_str().unwrap();

    // 4. Narrow the grant so it no longer contains screen_text: no further bytes
    store
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["transcripts".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
    let narrow_res = probe(&journal, "fetch", json!({"reference": valid_next_ref}));
    assert!(narrow_res.is_err());

    // 5. Revoke grant (clear_permission): no further bytes
    store.clear_permission(CONNECTION).unwrap();
    let revoke_res = probe(&journal, "fetch", json!({"reference": valid_next_ref}));
    assert!(revoke_res.is_err());
}

#[test]
fn screen_text_large_multibyte_fixture_and_char_boundary_paging() {
    let journal = fixture();
    let segment = journal
        .path()
        .join("chronicle")
        .join(DAY)
        .join(STREAM)
        .join(SEGMENT);
    let st_path = segment.join("talents/screen_text.md");

    // Build > 4 MiB fixture with preamble and a 3-byte char across the 65536 boundary
    let mut fixture_content = String::with_capacity(4_500_000);
    fixture_content.push_str("preamble across the start\n## 00:12 · terminal\n");
    while fixture_content.len() < 65534 {
        fixture_content.push('a');
    }
    assert_eq!(fixture_content.len(), 65534);
    // Multibyte 3-byte character '€' (e2 82 ac) at byte index 65534
    // Slices 65534..65537, crossing the 65536 boundary!
    fixture_content.push('€');
    assert_eq!(fixture_content.len(), 65537);

    // Pad until > 4 MiB
    while fixture_content.len() < 4_200_000 {
        fixture_content.push_str("padded content line across several megabytes\n");
    }

    fs::write(&st_path, &fixture_content).unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();

    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: vec!["screen_text".to_owned()],
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();

    let s = probe(&journal, "search", json!({"query": "padded"})).unwrap();
    let mut current_ref = s["results"][0]["reference"].as_str().unwrap().to_string();

    // Page through the whole file via next_reference
    let mut collected = String::new();
    let mut page_count = 0;
    loop {
        page_count += 1;
        let p = crate::dispatch::dispatch_authenticated_tool_call(
            journal.path(),
            crate::dispatch::DispatchPrincipal {
                connection: CONNECTION,
                agent_identity: CONNECTION,
            },
            crate::jsonrpc::ToolName::Fetch,
            Some(&json!({"reference": current_ref})),
            chrono::DateTime::parse_from_rfc3339("2026-09-14T09:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        )
        .unwrap_or_else(|error| panic!("screen-text page {page_count}: {error:?}"))
        .value;
        let text = p["text"].as_str().unwrap();
        assert!(text.len() <= 65_536);
        collected.push_str(text);
        match p.get("next_reference").and_then(Value::as_str) {
            Some(next) => current_ref = next.to_string(),
            None => break,
        }
    }
    assert!(page_count > 60); // 4.2 MiB / 64 KiB > 64 pages
    assert_eq!(collected, fixture_content);
    assert!(collected.contains('€'));
    assert!(collected.starts_with("preamble across the start"));
}

fn grant_categories(journal: &tempfile::TempDir, categories: &[&str]) {
    PermissionStore::open(journal.path())
        .set_permission(
            CONNECTION,
            ReadPermission {
                categories: categories.iter().map(|value| (*value).to_owned()).collect(),
                scope: ReadScope::WholeJournal,
            },
        )
        .unwrap();
}

#[test]
#[cfg(feature = "test-hooks")]
fn screen_text_search_withholds_replaced_and_reindexed_hit_in_flight() {
    use crate::source_freeze_barrier::{FreezePoint, run_with_source_freeze_barrier};
    let journal = fixture();
    let source = journal.path().join(format!(
        "chronicle/{DAY}/{STREAM}/{SEGMENT}/talents/screen_text.md"
    ));
    fs::write(&source, "## 00:12 · terminal\n- priorwitness\n").unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    grant_categories(&journal, &["screen_text"]);
    let root = journal.path().to_path_buf();
    let (response, fired) = run_with_source_freeze_barrier(
        FreezePoint::Before,
        1,
        move || {
            let modified = fs::metadata(&source).unwrap().modified().unwrap();
            fs::write(&source, "## 00:20 · terminal\n- replacementwitness\n").unwrap();
            // Windows refuses set_times on a read-only handle.
            fs::OpenOptions::new()
                .write(true)
                .open(&source)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(modified))
                .unwrap();
            solstone_core_indexer_store::scan::scan_journal(&root, true).unwrap();
        },
        || probe(&journal, "search", json!({"query":"priorwitness"})),
    );
    assert!(fired);
    assert!(response.unwrap()["results"].as_array().unwrap().is_empty());
    let replacement = probe(&journal, "search", json!({"query":"replacementwitness"})).unwrap();
    assert_eq!(replacement["results"].as_array().unwrap().len(), 1);
}

#[test]
fn screen_text_fetch_and_continuation_obey_retained_segment_deletion_marker() {
    for marker_is_directory in [false, true] {
        let journal = fixture();
        let segment = journal
            .path()
            .join(format!("chronicle/{DAY}/{STREAM}/{SEGMENT}"));
        let source = segment.join("talents/screen_text.md");
        fs::write(
            &source,
            format!(
                "## 00:12 · terminal\n- deletionwitness\n{}",
                "- content\n".repeat(15_000)
            ),
        )
        .unwrap();
        solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
        grant_categories(&journal, &["screen_text"]);
        let search = probe(&journal, "search", json!({"query":"deletionwitness"})).unwrap();
        let reference = search["results"][0]["reference"].as_str().unwrap();
        let first = probe(&journal, "fetch", json!({"reference":reference})).unwrap();
        let next = first["next_reference"].as_str().unwrap();
        if marker_is_directory {
            fs::create_dir(segment.join("tombstone.json")).unwrap();
        } else {
            fs::write(segment.join("tombstone.json"), "{}").unwrap();
        }
        assert!(source.is_file());
        assert!(probe(&journal, "fetch", json!({"reference":reference})).is_err());
        assert!(probe(&journal, "fetch", json!({"reference":next})).is_err());
    }
}

#[test]
fn screen_text_entry_fetch_uses_registered_category_for_other_families() {
    for (relative, categories) in [
        (
            format!("chronicle/{DAY}/entities/{SEGMENT}/talents/terminal.md"),
            vec!["transcripts"],
        ),
        (
            "reflections/weekly/20260914.md".to_owned(),
            vec!["screen_text", "facets"],
        ),
    ] {
        let journal = fixture();
        let source = journal.path().join(relative);
        fs::create_dir_all(source.parent().unwrap()).unwrap();
        fs::write(&source, "# ordinary record\n\nregisteredcategorywitness\n").unwrap();
        solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
        grant_categories(&journal, &categories);
        let search = probe(
            &journal,
            "search",
            json!({"query":"registeredcategorywitness"}),
        )
        .unwrap();
        let reference = search["results"][0]["reference"].as_str().unwrap();
        let fetched = probe(&journal, "fetch", json!({"reference":reference})).unwrap();
        assert!(
            fetched["text"]
                .as_str()
                .unwrap()
                .contains("registeredcategorywitness")
        );
    }
}

#[test]
fn screen_text_word_crossing_chunk_limit_is_searchable_by_owner_and_agent() {
    let journal = fixture();
    let source = journal.path().join(format!(
        "chronicle/{DAY}/{STREAM}/{SEGMENT}/talents/screen_text.md"
    ));
    let mut record = "## 00:12 · terminal\n".to_owned();
    while record.len() + 2 <= 65_532 {
        record.push_str("x ");
    }
    while record.len() < 65_532 {
        record.push(' ');
    }
    record.push_str("boundarywitness end\n");
    fs::write(&source, record).unwrap();
    solstone_core_indexer_store::scan::scan_journal(journal.path(), true).unwrap();
    let owner = solstone_core_indexer_query::search(
        journal.path(),
        solstone_core_indexer_query::OwnerBoundary,
        &solstone_core_indexer_query::SearchRequest::new(
            "boundarywitness",
            solstone_core_indexer_query::Order::Relevance,
        ),
        chrono::NaiveDate::from_ymd_opt(2026, 9, 14).unwrap(),
    )
    .unwrap();
    assert_eq!(owner.results.len(), 1);
    assert_eq!(owner.results[0].metadata.group_offset_ms, Some(12_000));
    grant_categories(&journal, &["screen_text"]);
    let agent = probe(&journal, "search", json!({"query":"boundarywitness"})).unwrap();
    assert_eq!(agent["results"].as_array().unwrap().len(), 1);
    assert_eq!(agent["results"][0]["segment"], SEGMENT);
    assert_eq!(agent["results"][0]["minute"], "00:12");
}
