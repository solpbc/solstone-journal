// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use crate::aggregate::handler_for;
use crate::command::{CommandContext, CommandOutput};
use crate::error::ClientError;
use crate::seam::HttpTransport;
use crate::transport::{ApiRequest, HttpResponse, SseRequest, SseStream, UploadRequest};
use pulldown_cmark::{Event, Options, Parser, Tag};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

struct Replies(RefCell<VecDeque<(&'static str, Value)>>);
impl HttpTransport for Replies {
    fn request(&self, request: ApiRequest) -> Result<HttpResponse, ClientError> {
        let (path, value) = self
            .0
            .borrow_mut()
            .pop_front()
            .expect("unexpected HTTP request");
        assert_eq!(request.path, path);
        Ok(HttpResponse {
            status: 200,
            headers: vec![],
            body: value.to_string().into_bytes(),
            policy: request.policy,
        })
    }
    fn upload(&self, _: UploadRequest) -> Result<HttpResponse, ClientError> {
        panic!("unexpected upload")
    }
    fn open_sse(&self, _: SseRequest) -> Result<SseStream, ClientError> {
        panic!("unexpected stream")
    }
}
fn run(path: &[&str], args: &[&str], responses: Vec<(&'static str, Value)>) -> CommandOutput {
    let transport = Replies(RefCell::new(responses.into()));
    let args = args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>();
    let (_, handler) = handler_for(path).expect("registered command");
    let output = handler(CommandContext {
        args: &args,
        env: &BTreeMap::new(),
        stdin: "",
        transport: &transport,
        clock: None,
        files: None,
        build_identity: None,
        client_item_ids: None,
        notification_sink: None,
        link_pairing: None,
        link_serve: None,
        link_status_probe: None,
    });
    assert!(transport.0.borrow().is_empty(), "unconsumed responses");
    output
}
fn active(text: &str) -> bool {
    Parser::new_ext(text, Options::all()).any(|event| {
        matches!(
            event,
            Event::Start(Tag::Image { .. } | Tag::Link { .. })
                | Event::Html(_)
                | Event::InlineHtml(_)
        )
    })
}
fn visible(text: &str) -> String {
    Parser::new_ext(text, Options::all())
        .filter_map(|event| match event {
            Event::Text(value) | Event::Code(value) => Some(value.into_string()),
            Event::SoftBreak | Event::HardBreak | Event::End(_) => Some(" ".into()),
            _ => None,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn inert(text: &str, original: &str) {
    assert!(!active(text), "active markup: {text:?}");
    assert_eq!(
        visible(text),
        original.split_whitespace().collect::<Vec<_>>().join(" ")
    );
}
const LEGACY: &str = "![cover](https://example.test/img.png) [open](https://example.test/) <img src='https://example.test/x'> 日本語";
#[test]
fn legacy_image_markup_is_literal_in_search_fields_and_raw_json() {
    assert!(active(LEGACY), "positive control");
    for flags in [vec!["needle"], vec!["needle", "--json"]] {
        let output = run(
            &["journal", "search"],
            &flags,
            vec![(
                "/app/search/api/search",
                json!({
                    "days": [{"day":"20250205", "results":[{"text": LEGACY, "record":LEGACY,
                    "path":"20250205/import.text/011131_0/A&B[one]_transcript.md", "entry_id":12,
                    "facet":"work-project", "stream":"import-one", "idx":0}]}], "has_more":false
                }),
            )],
        );
        assert_eq!(output.exit, 0);
        assert!(
            !active(&output.stdout),
            "active raw JSON: {}",
            output.stdout
        );
        let body: Value = serde_json::from_str(&output.stdout).unwrap();
        let item = &body["days"][0]["results"][0];
        inert(item["text"].as_str().unwrap(), LEGACY);
        inert(item["record"].as_str().unwrap(), LEGACY);
        assert_eq!(
            item["path"],
            "20250205/import.text/011131_0/A&B[one]_transcript.md"
        );
        assert_eq!(item["entry_id"], 12);
        assert_eq!(item["facet"], "work-project");
        assert_eq!(body["has_more"], false);
    }
}

#[test]
fn split_json_fields_cannot_compose_a_link_in_raw_stdout() {
    let output = run(
        &["journal", "facets"],
        &[],
        vec![(
            "/app/settings/api/facets",
            json!({
                "items": ["[label", "](https://example.test/)", "![cover][id]", "[id]: https://example.test/x"],
                "path": "20250205/import-one/screen.jsonl", "offset": 0, "cursor": null
            }),
        )],
    );
    assert_eq!(output.exit, 0);
    assert!(!active(&output.stdout), "{}", output.stdout);
    let body: Value = serde_json::from_str(&output.stdout).unwrap();
    for (value, original) in body["items"].as_array().unwrap().iter().zip([
        "[label",
        "](https://example.test/)",
        "![cover][id]",
        "[id]: https://example.test/x",
    ]) {
        inert(value.as_str().unwrap(), original);
    }
    assert_eq!(body["path"], "20250205/import-one/screen.jsonl");
    assert_eq!(body["offset"], 0);
    assert!(body["cursor"].is_null());
}

#[test]
fn transcript_pages_are_literal_after_every_truncation_boundary() {
    let texts = [
        LEGACY,
        "~~~\n![cover](https://example.test/x)\n~~~",
        "![cover][id]\n\n[id]: https://example.test/x",
        "\\![cover](https://example.test/x)",
        "&lt;img src=x&gt;",
    ];
    for text in texts {
        for limit in 0..=text.len() {
            let limit_arg = limit.to_string();
            let output = run(
                &["transcripts", "read"],
                &["20250205", "--max", &limit_arg],
                vec![(
                    "/app/transcripts/api/read/20250205",
                    json!({"markdown":text}),
                )],
            );
            assert_eq!(output.exit, 0);
            let mut end = if limit == 0 { text.len() } else { limit };
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let prefix = &text[..end];
            assert!(!active(&output.stdout));
            if prefix.contains(['[', '<', '&']) {
                inert(&output.stdout, prefix);
            } else {
                assert_eq!(output.stdout.trim_end(), prefix.trim_end());
            }
            assert_eq!(output.stderr.is_empty(), limit == 0 || limit == text.len());
            assert!(!active(&output.stderr));
        }
    }
}

#[test]
fn activities_are_literal_in_plain_and_nested_json_records() {
    for json_mode in [false, true] {
        let args = if json_mode {
            vec!["--day", "20250205", "--json"]
        } else {
            vec!["--day", "20250205"]
        };
        let output = run(
            &["activities", "list"],
            &args,
            vec![(
                "/app/activities/api/day/20250205/records",
                json!({"items":[{
                    "markdown":LEGACY,
                    "record":{"id":"meeting-one", "day":"20250205", "title":LEGACY, "details":{"notes":[LEGACY]}, "active_entities":[LEGACY], "created_at":10}
                }]}),
            )],
        );
        assert_eq!(output.exit, 0);
        assert!(!active(&output.stdout));
        if json_mode {
            let body: Value = serde_json::from_str(&output.stdout).unwrap();
            inert(body[0]["title"].as_str().unwrap(), LEGACY);
            inert(body[0]["details"]["notes"][0].as_str().unwrap(), LEGACY);
            inert(body[0]["active_entities"][0].as_str().unwrap(), LEGACY);
            assert_eq!(body[0]["id"], "meeting-one");
            assert_eq!(body[0]["created_at"], 10);
        } else {
            inert(&output.stdout, LEGACY);
        }
    }
}

#[test]
fn entity_search_and_observation_pages_are_literal() {
    let output = run(
        &["entities", "search"],
        &["--query", "needle"],
        vec![(
            "/app/entities/api/search",
            json!({
                "items":[{"name":LEGACY,"description":LEGACY,"type":"person","facets":[LEGACY]}]
            }),
        )],
    );
    assert_eq!(output.exit, 0);
    assert!(!active(&output.stdout));
    assert!(visible(&output.stdout).contains(LEGACY));
    for flags in [
        vec![],
        vec!["--all"],
        vec!["--json"],
        vec!["--all", "--json"],
    ] {
        let mut args = vec!["needle", "--facet", "work-project"];
        args.extend(&flags);
        let output = run(
            &["entities", "observations"],
            &args,
            vec![
                (
                    "/app/entities/api/work-project/resolve",
                    json!({"resolved":{"name":LEGACY}}),
                ),
                (
                    "/app/entities/api/work-project/observations",
                    json!({"total":1,"has_more":false,"items":[{"id":1,"content":LEGACY}]}),
                ),
            ],
        );
        assert_eq!(output.exit, 0);
        assert!(!active(&output.stdout));
        if flags.contains(&"--json") {
            let body: Value = serde_json::from_str(&output.stdout).unwrap();
            inert(body["items"][0]["content"].as_str().unwrap(), LEGACY);
            assert_eq!(body["items"][0]["id"], 1);
        } else {
            assert!(visible(&output.stdout).contains(LEGACY));
        }
    }
}

#[test]
fn entity_network_history_and_resolution_errors_are_literal_on_both_streams() {
    for command in ["network", "history"] {
        for json_mode in [false, true] {
            let args = if json_mode {
                vec!["needle", "--json"]
            } else {
                vec!["needle"]
            };
            let route = if command == "network" {
                "/app/entities/api/network"
            } else {
                "/app/entities/api/history"
            };
            let body = json!({"entity_id":"person-one", "peer_id":"person-two", "peer_name":LEGACY,
                "total":1,"total_neighbors":1,"offset":0,
                "neighbors":[{"entity_id":"person-two","name":LEGACY,"evidence":[{"label":LEGACY}]}],
                "evidence":[{"label":LEGACY,"path":"20250205/import-one/screen.jsonl"}]
            });
            let output = run(&["entities", command], &args, vec![(route, body)]);
            assert_eq!(output.exit, 0);
            assert!(!active(&output.stdout));
            if json_mode {
                let body: Value = serde_json::from_str(&output.stdout).unwrap();
                inert(body["evidence"][0]["label"].as_str().unwrap(), LEGACY);
                inert(body["neighbors"][0]["name"].as_str().unwrap(), LEGACY);
                assert_eq!(body["entity_id"], "person-one");
            } else {
                assert!(visible(&output.stdout).contains(LEGACY));
            }
            let output = run(
                &["entities", command],
                &args,
                vec![(
                    route,
                    json!({
                        "resolved":null,"query":"needle","candidates":[{"id":"person-one","name":LEGACY}]
                    }),
                )],
            );
            assert_eq!(output.exit, 1);
            assert!(!active(&output.stdout));
            assert!(!active(&output.stderr));
            if json_mode {
                assert!(output.stderr.is_empty());
                let body: Value = serde_json::from_str(&output.stdout).unwrap();
                inert(body["candidates"][0]["name"].as_str().unwrap(), LEGACY);
            } else {
                assert!(output.stdout.is_empty());
                assert!(visible(&output.stderr).contains(LEGACY));
            }
        }
    }
    for body in [
        json!({"blocked":true,"blocked_name":LEGACY}),
        json!({"candidates":[{"name":LEGACY}]}),
    ] {
        let output = run(
            &["entities", "observations"],
            &["needle", "--facet", "work-project"],
            vec![("/app/entities/api/work-project/resolve", body)],
        );
        assert_eq!(output.exit, 1);
        assert!(output.stdout.is_empty());
        assert!(!active(&output.stderr));
        assert!(visible(&output.stderr).contains(LEGACY));
    }
}

#[test]
fn entity_list_and_empty_result_names_are_literal() {
    for (args, route, body) in [
        (
            vec!["--facet", "work"],
            "/app/entities/api/work",
            json!({"attached":[{"name":LEGACY,"description":LEGACY}]}),
        ),
        (
            vec!["--facet", "work", "--day", "20250205"],
            "/app/entities/api/work/detected",
            json!({"items":[{"name":LEGACY,"description":LEGACY}]}),
        ),
    ] {
        let output = run(&["entities", "list"], &args, vec![(route, body)]);
        assert_eq!(output.exit, 0);
        assert!(!active(&output.stdout));
        assert!(visible(&output.stdout).contains(LEGACY));
    }
    let output = run(
        &["entities", "observations"],
        &["needle", "--facet", "work"],
        vec![
            (
                "/app/entities/api/work/resolve",
                json!({"resolved":{"name":LEGACY}}),
            ),
            (
                "/app/entities/api/work/observations",
                json!({"total":0,"items":[]}),
            ),
        ],
    );
    assert_eq!(output.exit, 0);
    assert!(!active(&output.stdout));
    assert!(visible(&output.stdout).contains(LEGACY));
}

#[test]
fn json_syntax_escaping_preserves_quoted_references_and_numeric_values() {
    let path = "20250205/import.text/011131_0/A&[B]\\\"_transcript.md";
    let output = run(
        &["journal", "facets"],
        &[],
        vec![(
            "/app/settings/api/facets",
            json!({
                "id":"entity[one]&two", "path":path, "facet":"work[one]", "stream":"device&one",
                "source_refs":[{"path":path}], "title":LEGACY,
                "max":u64::MAX,"min":i64::MIN,"decimal":1.25,"empty":"", "null":null,"bool":true
            }),
        )],
    );
    assert!(!active(&output.stdout));
    let body: Value = serde_json::from_str(&output.stdout).unwrap();
    assert_eq!(body["id"], "entity[one]&two");
    assert_eq!(body["path"], path);
    assert_eq!(body["source_refs"][0]["path"], path);
    assert_eq!(body["facet"], "work[one]");
    assert_eq!(body["stream"], "device&one");
    assert_eq!(body["max"], u64::MAX);
    assert_eq!(body["min"], i64::MIN);
    assert_eq!(body["decimal"], 1.25);
    assert_eq!(body["empty"], "");
    assert!(body["null"].is_null());
    assert_eq!(body["bool"], true);
    inert(body["title"].as_str().unwrap(), LEGACY);
}
