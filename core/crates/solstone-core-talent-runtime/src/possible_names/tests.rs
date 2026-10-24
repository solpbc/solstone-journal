// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value, json};
use solstone_core_generate::{
    ClientError, GenerateExecutor, GenerateRequest, GenerateResponse, GeneratedResponse,
};
use solstone_core_possible_names::CandidateEntity;

use super::retrieval::{Action, Route, Statement, compact, find_actions, select};
use super::*;
use crate::prepare::RuntimePaths;

fn entity(
    id: &str,
    name: &str,
    kind: &str,
    aka: &[&str],
    facets: &[(&str, &str)],
) -> CandidateEntity {
    CandidateEntity {
        entity_id: id.to_owned(),
        name: name.to_owned(),
        entity_type: kind.to_owned(),
        aka: aka.iter().map(|aka| (*aka).to_owned()).collect(),
        facets: facets
            .iter()
            .map(|(facet, description)| ((*facet).to_owned(), (*description).to_owned()))
            .collect(),
    }
}

fn statements(texts: &[&str]) -> Vec<Statement> {
    texts
        .iter()
        .enumerate()
        .map(|(index, text)| Statement {
            sentence: index as u64 + 1,
            text: (*text).to_owned(),
        })
        .collect()
}

fn find(texts: &[&str], entities: &[CandidateEntity]) -> Vec<Action> {
    find_actions(&statements(texts), entities, "", &[])
}

fn only(actions: &[Action], id: &str) -> Action {
    actions
        .iter()
        .find(|action| action.entity_id == id)
        .unwrap_or_else(|| panic!("no action for {id}: {actions:?}"))
        .clone()
}

#[test]
fn compact_keys_fold_like_python() {
    assert_eq!(compact("Straße"), "strasse");
    assert_eq!(
        compact("Ｊｅｒｅｍｉｅ O'Brien-Smith"),
        "jeremieobriensmith"
    );
    assert_eq!(compact("ﬁle №5"), "fileno5");
    assert_eq!(compact("Rene\u{301}e"), "renée");
    assert_eq!(compact("v\u{301}"), "v");
    assert_eq!(compact("ΣΊΣΥΦΟΣ"), "σίσυφοσ");
    assert_eq!(super::retrieval::casefold("İSTANBUL"), "i\u{307}stanbul");
}

#[test]
fn sound_keys_match_the_evaluated_prototype() {
    for (word, key) in [
        ("Fryer", "FR"),
        ("Friar", "FR"),
        ("Cognova", "KGNF"),
        ("Kognova", "KGNF"),
        ("Balthasar", "BL0SR"),
        ("Jeremie", "JRM"),
        ("Sunstone", "SNSTN"),
        ("Kathryn", "K0RN"),
        ("Catherine", "K0RN"),
        ("Philip", "FLP"),
        ("Fillip", "FLP"),
        ("Thompson", "0MPSN"),
        ("Tomson", "TMSN"),
        ("Knight", "NGT"),
        ("Xavier", "SFR"),
        ("Yvonne", "YFN"),
        ("Émile", "AML"),
        ("", ""),
        ("42", ""),
    ] {
        assert_eq!(phon::key(word), key, "{word}");
    }
}

#[test]
fn spans_stop_at_newlines_and_punctuation_and_record_their_features() {
    let entities = [
        entity("jl", "Jean-Luc Picard", "Person", &[], &[]),
        entity("ada", "Ada Please", "", &[], &[]),
        entity("nl", "North Lake", "", &[], &[]),
        entity("o", "O’Neil Hall", "", &[], &[]),
    ];
    let actions = find(
        &[
            "we met Jean-Luc Picard today",
            "ask Ada, please",
            "the North\nLake",
            "Then. O’Neil hall was empty",
        ],
        &entities,
    );
    let jl = only(&actions, "jl");
    assert_eq!(
        (jl.sentence, jl.start_byte, jl.end_byte, jl.words),
        (1, 7, 22, 2)
    );
    assert_eq!(jl.heard, "Jean-Luc Picard");
    assert!(jl.cap && !jl.sentence_start && jl.all_words_cap);
    assert!(actions.iter().all(|action| action.entity_id != "ada"));
    assert!(actions.iter().all(|action| action.entity_id != "nl"));
    let hall = only(&actions, "o");
    assert_eq!(hall.heard, "O’Neil hall");
    assert!(hall.cap && hall.sentence_start && !hall.all_words_cap);
    assert_eq!(hall.start_byte, "Then. ".len());
}

#[test]
fn each_route_matches_what_it_names() {
    let entities = [
        entity("ada", "Ada Lovelace", "Person", &[], &[]),
        entity("bob", "Robert Smith", "Person", &["Bobby"], &[]),
        entity("cath", "Catherine", "Person", &[], &[]),
        entity("marg", "Margarethe", "", &[], &[]),
        entity("tool", "Lovelace Labs", "Company", &[], &[]),
        // A two-letter sound key is too short to be indexed.
        entity("friar", "Friar", "", &[], &[]),
    ];
    let actions = find(
        &[
            "we saw Ada Lovelace and Bobby",
            "then Ada called",
            "I asked Kathryn about Margarete and Fryer",
            "the kathryn was hot",
        ],
        &entities,
    );
    let route = |id| only(&actions, id).route;
    assert_eq!(route("ada"), Route::Canonical);
    assert_eq!(route("bob"), Route::Alias);
    assert_eq!(route("cath"), Route::Phonetic);
    assert_eq!(route("marg"), Route::Spelling);
    assert!(actions.iter().all(|action| action.entity_id != "tool"));
    assert!(actions.iter().all(|action| action.entity_id != "friar"));
    let cath = only(&actions, "cath");
    assert_eq!((cath.sentence, cath.heard.as_str()), (3, "Kathryn"));

    let first = find(&["then Ada called"], &entities);
    assert_eq!(only(&first, "ada").route, Route::First);
    // A lowercase span is never matched by sound or spelling.
    assert!(find(&["the kathryn was hot"], &entities).is_empty());
}

#[test]
fn the_gate_keeps_only_capitalized_mid_sentence_names_worth_offering() {
    let entities = [
        entity("sol", "Solstone", "Tool", &[], &[]),
        entity("soul", "Soul PBC", "", &[], &[]),
        entity("ada", "Ada Lovelace", "Person", &[], &[]),
        entity("tuck", "Friar Tuck", "Person", &[], &[]),
        entity("lin", "Linus Pauling", "Person", &[], &[]),
    ];
    let actions = find(
        &[
            "we ship Solstone and Sol PBC",
            "Ada Lovelace wrote it.",
            "ask Fryer tuck now",
            "with Linus today",
        ],
        &entities,
    );
    let reason = |id| only(&actions, id).drop_reason();
    assert_eq!(reason("sol"), Some("product"));
    assert_eq!(reason("soul"), Some("product"));
    assert_eq!(reason("ada"), Some("not_capitalized_mid_sentence"));
    assert_eq!(reason("tuck"), Some("not_every_word_capitalized"));
    // First names and sound matches are found but not offered (G2).
    assert_eq!(reason("lin"), Some("route_disabled"));

    let named = find_actions(
        &statements(&["we saw Ada Lovelace"]),
        &entities,
        &super::retrieval::casefold("Notes about ADA and lovelace."),
        &[],
    );
    assert_eq!(
        only(&named, "ada").drop_reason(),
        Some("already_in_derived_text")
    );
    let partial = find_actions(
        &statements(&["we saw Ada Lovelace"]),
        &entities,
        "notes about ada",
        &[],
    );
    assert_eq!(only(&partial, "ada").drop_reason(), None);
}

#[test]
fn a_first_name_needs_few_rivals() {
    let two = [
        entity("a1", "Ada Lovelace", "Person", &[], &[]),
        entity("a2", "Ada Byron", "Person", &[], &[]),
    ];
    let actions = find(&["call Ada now"], &two);
    assert_eq!(only(&actions, "a1").competitors, ["a2"]);
    assert_eq!(only(&actions, "a1").drop_reason(), Some("route_disabled"));
    let mut three = two.to_vec();
    three.push(entity("a3", "Ada King", "Person", &[], &[]));
    let actions = find(&["call Ada now"], &three);
    assert_eq!(
        only(&actions, "a1").drop_reason(),
        Some("first_name_rivals")
    );
    // Not a person, or a one-word name: no first name.
    let none = [
        entity("t", "Ada Tools", "Company", &[], &[]),
        entity("p", "Ada", "Person", &[], &[]),
    ];
    let actions = find(&["call Ada now"], &none);
    assert_eq!(actions.len(), 1);
    assert_eq!(only(&actions, "p").route, Route::Canonical);
}

#[test]
fn each_entity_keeps_its_best_occurrence() {
    let entities = [
        entity("ada", "Ada Lovelace", "Person", &["Countess"], &[]),
        entity("bob", "Bob Ross", "Person", &[], &[]),
    ];
    let actions = find(
        &[
            "Bob Ross painted.",
            "the Countess spoke",
            "later Ada Lovelace spoke",
            "and Bob Ross again",
            "and Bob Ross once more",
        ],
        &entities,
    );
    let ada = only(&actions, "ada");
    assert_eq!((ada.route, ada.sentence), (Route::Canonical, 3));
    // The sentence-start occurrence loses to the first mid-sentence one.
    let bob = only(&actions, "bob");
    assert_eq!((bob.sentence, bob.sentence_start), (4, false));
}

#[test]
fn twelve_candidates_are_kept_and_presented_in_transcript_order() {
    let names = [
        "Alder", "Birch", "Cedar", "Dogwood", "Elmwood", "Firwood", "Ginkgo", "Hazel", "Ironwood",
        "Juniper", "Kapok", "Larch", "Maple", "Nutmeg",
    ];
    let entities = names
        .iter()
        .map(|name| entity(&name.to_lowercase(), name, "", &[], &[]))
        .collect::<Vec<_>>();
    let text = format!("we saw {}", names.join(" and "));
    let mut actions = find(&[&text], &entities);
    // Give the earliest two a rival each: they sort after every lone match.
    for action in actions.iter_mut().take(2) {
        action.competitors = vec!["someone".to_owned()];
    }
    let kept = select(&actions);
    assert_eq!(kept.len(), 12);
    let ids = kept
        .iter()
        .map(|action| action.entity_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        [
            "cedar", "dogwood", "elmwood", "firwood", "ginkgo", "hazel", "ironwood", "juniper",
            "kapok", "larch", "maple", "nutmeg"
        ]
    );
    assert!(
        kept.windows(2)
            .all(|pair| pair[0].start_byte < pair[1].start_byte)
    );
}

#[test]
fn the_packet_renders_exactly_as_evaluated() {
    let entities = [
        entity(
            "cath",
            "Catherine",
            "Person",
            &[],
            &[("work", "Cousin from Ohio")],
        ),
        entity(
            "marg",
            "Margarethe",
            "Person",
            &["Grete", "Gretl", "Marga", "Maggie", "Meg"],
            &[("home", "Aunt from Bremen"), ("work", "")],
        ),
        entity("kat", "Kathryn", "", &[], &[("home", "")]),
    ];
    let statements = statements(&[
        "Hello there.",
        "I asked Kathryn about Margarete and the plan.",
    ]);
    let candidates = select(&find_actions(&statements, &entities, "", &[]));
    let registry = entities
        .iter()
        .map(|entity| (entity.entity_id.as_str(), entity))
        .collect::<BTreeMap<_, _>>();
    let packet = packet::render(&statements, &["work".to_owned()], &candidates, &registry);
    assert_eq!(
        packet,
        "This recording is filed under: work\n\
         \n\
         TRANSCRIPT\n\
         [1] Hello there.\n\
         [2] I asked [[Kathryn]] about [[Margarete]] and the plan.\n\
         \n\
         CANDIDATES\n\
         N0: \"Kathryn\"\n   \
         matched [[Kathryn]] in line 2: same normalized spelling as the name\n   \
         filed under: home\n\
         N1: \"Margarethe\" (Person)\n   \
         matched [[Margarete]] in line 2: close spelling of the name\n   \
         also known as: Grete, Gretl, Marga, Maggie\n   \
         filed under: home, work\n   \
         note: Aunt from Bremen"
    );
    assert_eq!(
        packet::render(&statements[..1], &[], &[], &registry),
        "This recording is filed under: unknown\n\nTRANSCRIPT\n[1] Hello there.\n\nCANDIDATES\n"
    );
}

#[test]
fn a_note_is_cut_to_two_hundred_characters() {
    let note = "é".repeat(250);
    let entities = [entity("friar", "Friar", "", &[], &[("work", &note)])];
    let statements = statements(&["ask Friar now"]);
    let registry = BTreeMap::from([("friar", &entities[0])]);
    let packet = packet::render(
        &statements,
        &[],
        &select(&find_actions(&statements, &entities, "", &[])),
        &registry,
    );
    assert!(packet.ends_with(&format!("note: {}", "é".repeat(200))));
}

#[test]
fn the_schema_and_parser_require_one_answer_per_candidate() {
    assert_eq!(
        packet::schema(2),
        json!({
            "type": "object",
            "properties": {
                "N0": {"type": "string", "enum": ["yes", "no"]},
                "N1": {"type": "string", "enum": ["yes", "no"]},
            },
            "required": ["N0", "N1"],
            "additionalProperties": false,
        })
    );
    let schema = packet::schema(12);
    let order = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(order[10..], ["N10", "N11"]);
    assert_eq!(packet::parse(r#"{"N0":"yes","N1":"no"}"#, 2), Ok(vec![0]));
    assert_eq!(
        packet::parse(r#"{"N1":"yes","N0":"yes"}"#, 2),
        Ok(vec![0, 1])
    );
    for bad in [
        r#"{"N0":"yes"}"#,
        r#"{"N0":"yes","N1":"no","N2":"no"}"#,
        r#"{"N0":"maybe","N1":"no"}"#,
        r#"{"N0":true,"N1":"no"}"#,
        r#"["N0"]"#,
        "yes",
    ] {
        assert!(packet::parse(bad, 2).is_err(), "{bad}");
    }
}

#[test]
fn the_talent_version_follows_the_prompt() {
    let version = talent_version("prompt");
    assert!(version.starts_with("possible-names-v1-"));
    assert_eq!(version.len(), "possible-names-v1-".len() + 12);
    assert_eq!(version, talent_version("prompt"));
    assert_ne!(version, talent_version("prompt changed"));
}

#[test]
fn statements_are_numbered_as_speaker_labels_number_them() {
    let transcript = "{\"raw\":\"audio.flac\"}\n\
        {\"start\":\"00:00:01\",\"text\":\"one\"}\n\
        not json\n\
        {\"start\":\"00:00:02\",\"text\":\"misheard\",\"corrected\":\"two\"}\n\
        {\"start\":\"00:00:03\",\"text\":null}\n";
    assert_eq!(
        read_statements(transcript),
        [
            Statement {
                sentence: 1,
                text: "one".to_owned()
            },
            Statement {
                sentence: 2,
                text: "two".to_owned()
            },
            Statement {
                sentence: 3,
                text: String::new()
            },
        ]
    );
}

// The shipped talent, run through `execute_request` with a scripted model.

const DAY: &str = "20261024";
const SEGMENT: &str = "090000_300";
const PROMPT_HEAD: &str = "You check possible names in a speech-to-text transcript.";

struct Scripted {
    text: Mutex<String>,
    requests: Mutex<Vec<GenerateRequest>>,
}

impl Scripted {
    fn new(text: &str) -> Self {
        Self {
            text: Mutex::new(text.to_owned()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

impl GenerateExecutor for Scripted {
    fn execute(&self, request: &GenerateRequest) -> Result<GenerateResponse, ClientError> {
        self.requests.lock().unwrap().push(request.clone());
        Ok(GenerateResponse::Generated(Box::new(GeneratedResponse {
            id: None,
            text: self.text.lock().unwrap().clone(),
            model: "invented-model".to_owned(),
            usage: json!({"input_tokens": 10, "output_tokens": 5, "total_tokens": 15}),
            finish_reason: "stop".to_owned(),
            thinking: None,
            schema_validation: Some(json!({"valid": true, "errors": []})),
            input_budget: None,
            request_budget: None,
            inference: None,
            hints_applied: Vec::new(),
        })))
    }
}

struct Journal {
    _root: tempfile::TempDir,
    paths: RuntimePaths,
    context: ExecutionContext,
}

impl Journal {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let journal = root.path().join("journal");
        fs::create_dir_all(journal.join("config")).unwrap();
        fs::write(
            journal.join("config/journal.json"),
            r#"{"providers":{"active":{"provider":"test","model":"test-model"}}}"#,
        )
        .unwrap();
        let apps = root.path().join("apps/entities/talent");
        fs::create_dir_all(&apps).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../payload/solstone/apps/entities/talent/possible_names.md"),
            apps.join("possible_names.md"),
        )
        .unwrap();
        fs::create_dir_all(root.path().join("talent")).unwrap();
        let paths = RuntimePaths {
            talent_root: root.path().join("talent"),
            apps_root: root.path().join("apps"),
            templates_dir: root.path().join("templates"),
        };
        let fixture = Self {
            paths,
            context: ExecutionContext { journal },
            _root: root,
        };
        fixture.entity("marg", "Margarethe", &[("work", "Aunt from Bremen")]);
        fixture.entity("ada", "Ada Lovelace", &[("work", "")]);
        fixture.transcript(&["I asked Margarete about it.", "Ada Lovelace came."]);
        fs::write(
            fixture.segment().join("talents/facets.json"),
            r#"[{"facet":"work"},{"facet":""},"bad"]"#,
        )
        .unwrap();
        fixture
    }

    fn journal(&self) -> &Path {
        &self.context.journal
    }

    fn segment(&self) -> PathBuf {
        self.journal().join("chronicle").join(DAY).join(SEGMENT)
    }

    fn transcript(&self, texts: &[&str]) {
        fs::create_dir_all(self.segment().join("talents")).unwrap();
        let mut lines = String::from("{\"raw\": \"audio.flac\"}\n");
        for (index, text) in texts.iter().enumerate() {
            lines.push_str(&format!(
                "{}\n",
                json!({"start": format!("00:00:0{index}"), "speaker": 1, "text": text})
            ));
        }
        fs::write(self.segment().join("audio.jsonl"), lines).unwrap();
    }

    fn entity(&self, id: &str, name: &str, facets: &[(&str, &str)]) {
        let dir = self.journal().join("entities").join(id);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("entity.json"),
            json!({"id": id, "name": name, "type": "Person"}).to_string(),
        )
        .unwrap();
        for (facet, description) in facets {
            let link = self
                .journal()
                .join("facets")
                .join(facet)
                .join("entities")
                .join(id);
            fs::create_dir_all(&link).unwrap();
            fs::write(
                link.join("entity.json"),
                json!({"entity_id": id, "description": description}).to_string(),
            )
            .unwrap();
        }
    }

    fn run(&self, model: &Scripted) -> RuntimeOutcome {
        let request = Map::from_iter([
            ("name".to_owned(), json!(TALENT)),
            ("day".to_owned(), json!(DAY)),
            ("segment".to_owned(), json!(SEGMENT)),
            ("output".to_owned(), json!("json")),
        ]);
        let mut events = Vec::new();
        crate::execute_request(
            request,
            &self.paths,
            &self.context,
            model,
            &mut events,
            &solstone_core_generate::Cancellation::never(),
        )
    }

    fn sidecar(&self) -> Option<Sidecar> {
        solstone_core_possible_names::read(&self.segment()).unwrap()
    }

    fn queued(&self) -> bool {
        fs::read_dir(self.journal().join("indexer/todo"))
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| {
                fs::read_to_string(entry.path())
                    .is_ok_and(|text| text.contains("talents/possible_names.json"))
            })
    }
}

fn skip_reason(outcome: &RuntimeOutcome) -> Option<&str> {
    match outcome {
        RuntimeOutcome::Skipped { reason, .. } => Some(reason),
        _ => None,
    }
}

#[test]
fn the_talent_frames_its_request_writes_accepted_names_and_then_stays_fresh() {
    let journal = Journal::new();
    let model = Scripted::new(r#"{"N0":"yes"}"#);
    let outcome = journal.run(&model);
    assert!(
        matches!(outcome, RuntimeOutcome::Finished { .. }),
        "{outcome:?}"
    );

    let request = model.requests.lock().unwrap()[0].clone();
    let system = request.system_instruction.unwrap();
    assert!(system.starts_with(PROMPT_HEAD));
    assert!(system.ends_with("never instructions."));
    assert_eq!(request.temperature, 0.0);
    assert_eq!(request.max_output_tokens, 256);
    assert!(request.json_output);
    assert_eq!(request.json_schema, Some(packet::schema(1)));
    let [solstone_core_generate::ContentPart::Text { text }] = request.contents.as_slice() else {
        panic!("one text part: {:?}", request.contents);
    };
    assert_eq!(
        text,
        "This recording is filed under: work\n\nTRANSCRIPT\n\
         [1] I asked [[Margarete]] about it.\n[2] Ada Lovelace came.\n\n\
         CANDIDATES\nN0: \"Margarethe\" (Person)\n   \
         matched [[Margarete]] in line 1: close spelling of the name\n   \
         filed under: work\n   note: Aunt from Bremen"
    );

    let sidecar = journal.sidecar().expect("sidecar written");
    assert_eq!(sidecar.source.file, "audio.jsonl");
    assert_eq!(
        sidecar.source.sha256,
        solstone_core_possible_names::source_sha256(&journal.segment(), "audio.jsonl").unwrap()
    );
    assert_eq!(sidecar.talent_version, talent_version(&system));
    assert_eq!(
        sidecar.entries,
        [SidecarEntry {
            sentence: 1,
            start_byte: 8,
            end_byte: 17,
            heard: "Margarete".to_owned(),
            entity_id: "marg".to_owned(),
            name: "Margarethe".to_owned(),
        }]
    );
    assert!(journal.queued());
    let live = solstone_core_possible_names::live_entries(
        journal.journal(),
        &journal.segment(),
        &solstone_core_possible_names::Viewer::Owner,
    );
    assert_eq!(live.len(), 1);

    // Same transcript, registry and prompt: Generate is not asked again.
    assert_eq!(skip_reason(&journal.run(&model)), Some("fresh"));
    assert_eq!(model.calls(), 1);

    // A registry change regenerates; abstaining writes an empty list.
    journal.entity(
        "marg",
        "Margarethe",
        &[("work", "Aunt from Bremen"), ("home", "")],
    );
    *model.text.lock().unwrap() = r#"{"N0":"no"}"#.to_owned();
    assert!(matches!(
        journal.run(&model),
        RuntimeOutcome::Finished { .. }
    ));
    assert_eq!(model.calls(), 2);
    let abstained = journal.sidecar().unwrap();
    assert!(abstained.entries.is_empty());
    assert_ne!(abstained.registry_revision, sidecar.registry_revision);
    assert_eq!(skip_reason(&journal.run(&model)), Some("fresh"));

    // Re-filing the segment regenerates even with the same transcript.
    fs::write(
        journal.segment().join("talents/facets.json"),
        r#"[{"facet":"home"}]"#,
    )
    .unwrap();
    assert!(matches!(
        journal.run(&model),
        RuntimeOutcome::Finished { .. }
    ));
    assert_eq!(model.calls(), 3);
    assert_ne!(
        journal.sidecar().unwrap().registry_revision,
        abstained.registry_revision
    );
}

#[test]
fn a_malformed_answer_fails_without_writing() {
    let journal = Journal::new();
    for answer in [
        r#"{"N0":"perhaps"}"#,
        r#"{"N0":"yes","N1":"no"}"#,
        "not json",
    ] {
        let outcome = journal.run(&Scripted::new(answer));
        assert!(
            matches!(&outcome, RuntimeOutcome::StageFailed(error) if error.phase == "parse"),
            "{answer}: {outcome:?}"
        );
        assert!(journal.sidecar().is_none());
    }
}

#[test]
fn no_candidates_skips_generate_and_retires_an_older_list() {
    let journal = Journal::new();
    let model = Scripted::new(r#"{"N0":"yes"}"#);
    journal.run(&model);
    assert_eq!(journal.sidecar().unwrap().entries.len(), 1);

    journal.transcript(&["Nothing to see here."]);
    assert_eq!(skip_reason(&journal.run(&model)), Some("no_candidates"));
    assert_eq!(model.calls(), 1);
    let retired = journal.sidecar().unwrap();
    assert!(retired.entries.is_empty());
    assert_eq!(skip_reason(&journal.run(&model)), Some("fresh"));

    let empty = Journal::new();
    empty.transcript(&["Nothing to see here."]);
    assert_eq!(skip_reason(&empty.run(&model)), Some("no_candidates"));
    assert!(empty.sidecar().is_none());

    fs::remove_file(empty.segment().join("audio.jsonl")).unwrap();
    assert_eq!(skip_reason(&empty.run(&model)), Some("no_transcript"));
    assert_eq!(model.calls(), 1);
}

#[test]
fn the_packet_preview_writes_nothing() {
    let journal = Journal::new();
    let model = Scripted::new(r#"{"N0":"yes"}"#);
    journal.run(&model);
    journal.transcript(&["Nothing to see here."]);
    let request = solstone_core_talent_cli::preview::PreviewRequest {
        name: TALENT.to_owned(),
        day: Some(DAY.to_owned()),
        segment: Some(SEGMENT.to_owned()),
        facet: None,
        activity: None,
    };
    let _ = crate::assemble::assemble_prompt_preview(&request, &journal.paths, &journal.context);
    assert_eq!(journal.sidecar().unwrap().entries.len(), 1);
}

#[test]
fn the_evaluation_packet_reports_what_a_run_would_send() {
    let journal = Journal::new();
    let report = inspect_packet(&journal.segment(), &journal.paths).unwrap();
    assert_eq!(report["decision"], "generate");
    assert_eq!(report["fresh"], false);
    assert_eq!(report["temperature"], 0);
    assert_eq!(report["max_output_tokens"], 256);
    assert!(
        report["system_instruction"]
            .as_str()
            .unwrap()
            .starts_with(PROMPT_HEAD)
    );
    assert_eq!(report["json_schema"], packet::schema(1));
    assert_eq!(report["candidates"][0]["id"], "N0");
    assert_eq!(report["candidates"][0]["action"]["route"], "spelling");
    assert_eq!(report["pool"][1]["action"]["entity_id"], "ada");
    assert_eq!(report["pool"][1]["dropped"], "not_capitalized_mid_sentence");
    assert!(journal.sidecar().is_none());
    assert!(!journal.queued());
}

#[test]
fn retrieval_and_packet_reproduce_the_evaluated_prototype() {
    let fixture: Value =
        serde_json::from_str(include_str!("prototype_fixture.json")).expect("fixture");
    let entities: Vec<CandidateEntity> = fixture["entities"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entity| CandidateEntity {
            entity_id: entity["entity_id"].as_str().unwrap().to_owned(),
            name: entity["name"].as_str().unwrap().to_owned(),
            entity_type: entity["type"].as_str().unwrap().to_owned(),
            aka: serde_json::from_value(entity["aka"].clone()).unwrap(),
            facets: serde_json::from_value(entity["facets"].clone()).unwrap(),
        })
        .collect();
    let texts = fixture["statements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|text| text.as_str().unwrap())
        .collect::<Vec<_>>();
    let statements = statements(&texts);
    let facets: Vec<String> = serde_json::from_value(fixture["segment_facets"].clone()).unwrap();
    let derived = super::retrieval::casefold(fixture["derived_text"].as_str().unwrap());

    let pool = find_actions(&statements, &entities, &derived, &facets);
    let mut got = pool
        .iter()
        .map(|action| {
            json!({
                "entity_id": action.entity_id,
                "route": action.route,
                "sentence": action.sentence,
                "start_byte": action.start_byte,
                "heard": action.heard,
                "competitors": action.competitors,
                "cap": action.cap,
                "sentence_start": action.sentence_start,
                "all_words_cap": action.all_words_cap,
                "in_segment_derived": action.in_segment_derived,
                "kept": action.drop_reason().is_none(),
            })
        })
        .collect::<Vec<_>>();
    let key = |row: &Value| {
        (
            row["entity_id"].as_str().unwrap().to_owned(),
            row["route"].as_str().unwrap().to_owned(),
        )
    };
    got.sort_by_key(key);
    assert_eq!(Value::Array(got), fixture["expected_pool"]);

    let candidates = select(&pool);
    let registry = entities
        .iter()
        .map(|entity| (entity.entity_id.as_str(), entity))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        packet::render(&statements, &facets, &candidates, &registry),
        fixture["expected_packet"].as_str().unwrap()
    );
    assert_eq!(packet::schema(candidates.len()), fixture["expected_schema"]);
}

#[test]
fn an_alias_needs_its_entity_filed_under_a_segment_facet() {
    let entities = [
        entity(
            "bob",
            "Robert Smith",
            "Person",
            &["Bobby"],
            &[("travel", ""), ("work", "")],
        ),
        entity("ann", "Ann Lee", "Person", &["Annie"], &[("home", "")]),
    ];
    let spoken = statements(&["we called Bobby and Annie today"]);
    let gate = |facets: &[&str]| {
        let facets = facets
            .iter()
            .map(|facet| (*facet).to_owned())
            .collect::<Vec<_>>();
        find_actions(&spoken, &entities, "", &facets)
            .into_iter()
            .map(|action| (action.entity_id.clone(), action.drop_reason()))
            .collect::<BTreeMap<_, _>>()
    };
    let outside = Some("alias_outside_segment_facets");
    assert_eq!(gate(&["work"])["bob"], None);
    assert_eq!(gate(&["work"])["ann"], outside);
    assert_eq!(gate(&["home", "travel"])["bob"], None);
    assert_eq!(gate(&[])["bob"], outside);
    // The facet rule runs last: a disabled route keeps its own reason.
    let first = [entity("ann", "Ann Lee", "Person", &[], &[("home", "")])];
    let actions = find_actions(&statements(&["we called Ann today"]), &first, "", &[]);
    assert_eq!(only(&actions, "ann").drop_reason(), Some("route_disabled"));
}
