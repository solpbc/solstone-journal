// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::*;

const SEGMENT: &str = "chronicle/20261024/default/090000_300";

struct Fixture {
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let fixture = Self { root };
        for facet in ["alpha", "beta"] {
            fs::create_dir_all(fixture.path().join("facets").join(facet)).unwrap();
        }
        fixture.entity("friar", "Friar", &["alpha"]);
        fixture.entity("ana", "Ana Lima", &["beta"]);
        fs::create_dir_all(fixture.segment()).unwrap();
        fs::write(
            fixture.segment().join("audio.jsonl"),
            "{\"raw\":\"audio.flac\"}\n\
             {\"start\":\"00:00:01\",\"speaker\":1,\"text\":\"ask Fryer to call anna\"}\n\
             not json\n\
             {\"start\":\"00:00:02\",\"speaker\":2,\"text\":\"misheard\",\"corrected\":\"Fryer again\"}\n",
        )
        .unwrap();
        fixture
    }

    fn path(&self) -> &Path {
        self.root.path()
    }

    fn segment(&self) -> PathBuf {
        self.path().join(SEGMENT)
    }

    fn entity(&self, id: &str, name: &str, facets: &[&str]) {
        let dir = self.path().join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("entity.json"),
            json!({"id": id, "name": name, "aka": ["Fryer"]}).to_string(),
        )
        .unwrap();
        for facet in facets {
            self.link(facet, id, false);
        }
    }

    fn link(&self, facet: &str, id: &str, detached: bool) {
        let link = self
            .path()
            .join("facets")
            .join(facet)
            .join("entities")
            .join(id);
        fs::create_dir_all(&link).unwrap();
        fs::write(
            link.join("entity.json"),
            json!({"entity_id": id, "detached": detached}).to_string(),
        )
        .unwrap();
    }

    fn identity(&self, id: &str, value: serde_json::Value) {
        fs::write(
            self.path().join("entities").join(id).join("entity.json"),
            value.to_string(),
        )
        .unwrap();
    }

    fn save(&self, entries: Vec<SidecarEntry>) {
        let sidecar = Sidecar::new(
            "test-1",
            SidecarSource {
                file: "audio.jsonl".to_owned(),
                sha256: source_sha256(&self.segment(), "audio.jsonl").unwrap(),
            },
            "rev",
            1_700_000_000_000,
            entries,
        );
        write(&self.segment(), &sidecar).unwrap();
    }

    fn live(&self, viewer: &Viewer) -> Vec<String> {
        live_entries(self.path(), &self.segment(), viewer)
            .into_iter()
            .map(|entry| format!("{}:{}", entry.index, entry.name))
            .collect()
    }
}

fn entry(sentence: u64, start: usize, heard: &str, id: &str, name: &str) -> SidecarEntry {
    SidecarEntry {
        sentence,
        start_byte: start,
        end_byte: start + heard.len(),
        heard: heard.to_owned(),
        entity_id: id.to_owned(),
        name: name.to_owned(),
    }
}

fn standard_entries() -> Vec<SidecarEntry> {
    vec![
        entry(1, 4, "Fryer", "friar", "Friar"),
        entry(1, 18, "anna", "ana", "Ana Lima"),
        entry(2, 0, "Fryer", "friar", "Friar"),
    ]
}

fn facets(names: &[&str]) -> Viewer {
    Viewer::Agent {
        facets: names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<BTreeSet<_>>(),
        entities: true,
    }
}

#[test]
fn the_writer_publishes_what_the_reader_reads_and_refuses_off_contract_files() {
    let fixture = Fixture::new();
    assert!(read(&fixture.segment()).unwrap().is_none());
    fixture.save(standard_entries());
    let sidecar = read(&fixture.segment()).unwrap().unwrap();
    assert_eq!(sidecar.schema, POSSIBLE_NAMES_SCHEMA);
    assert_eq!(sidecar.entries, standard_entries());
    let on_disk: serde_json::Value =
        serde_json::from_slice(&fs::read(sidecar_path(&fixture.segment())).unwrap()).unwrap();
    assert_eq!(on_disk["source"]["file"], "audio.jsonl");
    assert_eq!(on_disk["entries"][0]["heard"], "Fryer");

    let mut wrong = sidecar.clone();
    wrong.entries[0].heard = "Fry".to_owned();
    assert!(matches!(
        write(&fixture.segment(), &wrong),
        Err(WriteError::Invalid(_))
    ));
    let mut wrong = sidecar.clone();
    wrong.source.file = "../audio.jsonl".to_owned();
    assert!(matches!(
        write(&fixture.segment(), &wrong),
        Err(WriteError::Invalid(_))
    ));
    let mut wrong = sidecar;
    wrong.source.sha256 = "ABC".to_owned();
    assert!(matches!(
        write(&fixture.segment(), &wrong),
        Err(WriteError::Invalid(_))
    ));
    assert!(source_sha256(&fixture.segment(), "talents/possible_names.json").is_err());

    fs::write(
        sidecar_path(&fixture.segment()),
        json!({"schema": "other"}).to_string(),
    )
    .unwrap();
    assert!(matches!(
        read(&fixture.segment()),
        Err(ReadError::Malformed(_))
    ));
    assert!(fixture.live(&Viewer::Owner).is_empty());
}

#[test]
fn valid_entries_are_live_for_the_owner_and_for_agents_who_see_a_facet() {
    let fixture = Fixture::new();
    fixture.save(standard_entries());
    assert_eq!(
        fixture.live(&Viewer::Owner),
        ["0:Friar", "1:Ana Lima", "2:Friar"]
    );
    assert_eq!(fixture.live(&facets(&["alpha"])), ["0:Friar", "2:Friar"]);
    assert_eq!(fixture.live(&facets(&["beta"])), ["1:Ana Lima"]);
    assert!(fixture.live(&facets(&[])).is_empty());
    assert!(fixture.live(&facets(&["gamma"])).is_empty());

    let entries = live_entries(fixture.path(), &fixture.segment(), &Viewer::Owner);
    let labels = transcript_labels(&entries).unwrap();
    assert_eq!(labels.file, "audio.jsonl");
    assert_eq!(
        labels.statements.get(&1).unwrap(),
        &vec![(9, "Friar".to_owned()), (22, "Ana Lima".to_owned())]
    );
    assert_eq!(
        labels.statements.get(&2).unwrap(),
        &vec![(5, "Friar".to_owned())]
    );
    assert!(transcript_labels(&[]).is_none());

    assert_eq!(
        indexed_entry(&entries, 1, "possible name: Ana Lima").map(|entry| entry.index),
        Some(1)
    );
    // A chunk indexed from an older list no longer stands for this entry.
    assert!(indexed_entry(&entries, 1, "possible name: Friar").is_none());
    assert!(indexed_entry(&entries, 7, "possible name: Friar").is_none());
}

#[test]
fn changed_transcript_bytes_hide_every_entry() {
    let fixture = Fixture::new();
    fixture.save(standard_entries());
    let transcript = fixture.segment().join("audio.jsonl");
    let mut bytes = fs::read(&transcript).unwrap();
    bytes.extend_from_slice(b"{\"start\":\"00:00:03\",\"text\":\"later\"}\n");
    fs::write(&transcript, bytes).unwrap();
    assert!(fixture.live(&Viewer::Owner).is_empty());
    fs::remove_file(&transcript).unwrap();
    assert!(fixture.live(&Viewer::Owner).is_empty());
}

#[test]
fn an_entry_whose_words_moved_is_hidden_alone() {
    let fixture = Fixture::new();
    let mut entries = standard_entries();
    // Statement 2 renders `corrected`, so `text` offsets name nothing there.
    entries.push(entry(2, 0, "mishe", "friar", "Friar"));
    entries.push(entry(9, 0, "Fryer", "friar", "Friar"));
    entries.push(entry(1, 5, "Fryer", "friar", "Friar"));
    fixture.save(entries);
    assert_eq!(
        fixture.live(&Viewer::Owner),
        ["0:Friar", "1:Ana Lima", "2:Friar"]
    );
}

#[test]
fn rename_block_detach_merge_and_delete_hide_that_entity_only() {
    let fixture = Fixture::new();
    fixture.save(standard_entries());
    let only_ana = ["1:Ana Lima"];

    fixture.identity("friar", json!({"id": "friar", "name": "Friar Tuck"}));
    assert_eq!(fixture.live(&Viewer::Owner), only_ana);

    fixture.identity(
        "friar",
        json!({"id": "friar", "name": "Friar", "blocked": true}),
    );
    assert_eq!(fixture.live(&Viewer::Owner), only_ana);

    fixture.identity("friar", json!({"id": "friar", "name": "Friar"}));
    assert_eq!(fixture.live(&Viewer::Owner).len(), 3);

    fixture.link("alpha", "friar", true);
    assert_eq!(fixture.live(&Viewer::Owner), only_ana);
    // Attached elsewhere, an agent that sees only the detached facet loses it.
    fixture.link("beta", "friar", false);
    assert_eq!(fixture.live(&Viewer::Owner).len(), 3);
    assert!(fixture.live(&facets(&["alpha"])).is_empty());
    fs::remove_dir_all(fixture.path().join("facets/beta/entities/friar")).unwrap();
    fixture.link("alpha", "friar", false);

    // A merge or an owner delete records the id before anything is removed.
    fs::write(
        fixture.path().join("entities/retired.json"),
        json!({"ids": {"friar": {"state": "merged", "dir": "friar", "successor": "ana"}}})
            .to_string(),
    )
    .unwrap();
    assert_eq!(fixture.live(&Viewer::Owner), only_ana);
    fs::remove_file(fixture.path().join("entities/retired.json")).unwrap();

    fs::remove_dir_all(fixture.path().join("entities/friar")).unwrap();
    assert_eq!(fixture.live(&Viewer::Owner), only_ana);
}

#[test]
fn indexed_paths_name_their_segment() {
    let root = Path::new("/journal");
    assert_eq!(
        segment_dir_for_indexed_path(
            root,
            "20261024/default/090000_300/talents/possible_names.json"
        ),
        Some(root.join(SEGMENT))
    );
    assert_eq!(
        segment_dir_for_indexed_path(
            root,
            "chronicle/20261024/default/090000_300/talents/possible_names.json"
        ),
        Some(root.join(SEGMENT))
    );
    for rel in [
        "20261024/default/090000_300/talents/screen_text.md",
        "20261024/../090000_300/talents/possible_names.json",
        "20261024/talents/possible_names.json",
    ] {
        assert_eq!(segment_dir_for_indexed_path(root, rel), None, "{rel}");
    }
}

#[test]
fn candidates_are_the_entities_a_reader_admits_with_their_relationships() {
    let fixture = Fixture::new();
    fixture.entity("gone", "Gone", &["alpha"]);
    fixture.entity("hidden", "Hidden", &["alpha"]);
    fixture.identity(
        "hidden",
        json!({"id": "hidden", "name": "Hidden", "blocked": true}),
    );
    fixture.entity("loose", "Loose", &[]);
    fixture.entity("cut", "Cut", &["beta"]);
    fixture.link("beta", "cut", true);
    fixture.identity(
        "ana",
        json!({"id": "ana", "name": "Ana Lima", "type": "Person", "aka": ["Ana", " ", 3, "A. Lima"]}),
    );
    fixture.link("alpha", "ana", false);
    fs::write(
        fixture.path().join("facets/beta/entities/ana/entity.json"),
        json!({"entity_id": "ana", "description": "Runs the lab"}).to_string(),
    )
    .unwrap();
    fs::write(
        fixture.path().join("entities/retired.json"),
        json!({"ids": {"gone": {"state": "deleted", "dir": "gone"}}}).to_string(),
    )
    .unwrap();

    let candidates = load_candidate_entities(fixture.path()).unwrap();
    assert_eq!(
        candidates
            .iter()
            .map(|entity| entity.entity_id.as_str())
            .collect::<Vec<_>>(),
        ["ana", "friar"]
    );
    let ana = &candidates[0];
    assert_eq!(ana.name, "Ana Lima");
    assert_eq!(ana.entity_type, "Person");
    assert_eq!(ana.aka, ["Ana", "A. Lima"]);
    assert_eq!(
        ana.facets,
        BTreeMap::from([
            ("alpha".to_owned(), String::new()),
            ("beta".to_owned(), "Runs the lab".to_owned()),
        ])
    );
    assert_eq!(candidates[1].entity_type, "");
}

#[test]
fn the_addressed_transcript_is_audio_or_the_only_speech_file() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(SEGMENT);
    fs::create_dir_all(&dir).unwrap();
    assert_eq!(speech_transcript(&dir), None);
    fs::write(dir.join("screen.jsonl"), "{}\n").unwrap();
    fs::write(dir.join("mic_audio.jsonl"), "{}\n").unwrap();
    assert_eq!(speech_transcript(&dir).as_deref(), Some("mic_audio.jsonl"));
    fs::write(dir.join("sys_audio.jsonl"), "{}\n").unwrap();
    assert_eq!(speech_transcript(&dir), None);
    fs::write(dir.join("audio.jsonl"), "{}\n").unwrap();
    assert_eq!(speech_transcript(&dir).as_deref(), Some("audio.jsonl"));

    let chat = root
        .path()
        .join("chronicle/20261024/import.chatgpt/090000_300");
    fs::create_dir_all(&chat).unwrap();
    fs::write(chat.join("conversation_transcript.jsonl"), "{}\n").unwrap();
    assert_eq!(speech_transcript(&chat), None);
}

#[test]
fn an_agent_without_the_entities_category_is_shown_none() {
    let fixture = Fixture::new();
    fixture.save(standard_entries());
    assert_eq!(fixture.live(&facets(&["alpha", "beta"])).len(), 3);
    let without = Viewer::Agent {
        facets: ["alpha", "beta"].map(str::to_owned).into(),
        entities: false,
    };
    assert!(fixture.live(&without).is_empty());
    let registry = Registry::new(fixture.path());
    assert!(!registry.admits("friar", "Friar", &without));
    assert!(registry.admits("friar", "Friar", &facets(&["alpha"])));
    assert!(registry.admits("friar", "Friar", &Viewer::Owner));
}

#[test]
fn a_label_that_only_repeats_its_words_is_not_shown_beside_them() {
    let entry = |heard: &str, name: &str| LiveEntry {
        index: 0,
        file: "audio.jsonl".to_owned(),
        sentence: 1,
        start_byte: 0,
        end_byte: heard.len(),
        heard: heard.to_owned(),
        entity_id: "e".to_owned(),
        name: name.to_owned(),
    };
    for (heard, name) in [
        ("ChatGPT", "ChatGPT"),
        ("Chat GPT", "ChatGPT"),
        ("open-ai", "OpenAI"),
        ("Élan", "élan"),
    ] {
        assert!(repeats_heard(&entry(heard, name)), "{heard}");
        assert!(
            transcript_labels(&[entry(heard, name)]).is_none(),
            "{heard}"
        );
    }
    let shown = transcript_labels(&[entry("ChatGPT", "ChatGPT"), entry("Fryer", "Friar")]).unwrap();
    assert_eq!(
        shown.statements.get(&1).unwrap(),
        &vec![(5, "Friar".to_owned())]
    );
}
