// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::HashMap;

use serde::Serialize;

use crate::assets;

#[derive(Debug, Clone, Copy)]
pub struct AppDefinition {
    pub name: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
    pub lucide_icon: &'static str,
    pub pinnable: bool,
    pub focus: Option<Focus>,
    pub description: &'static str,
    pub source: &'static str,
    pub keywords: &'static [&'static str],
    pub rail_group: Option<RailGroup>,
    pub rail_rank: u8,
    pub date_nav: Option<DateNav>,
    pub has_background: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Focus {
    pub group: FocusGroup,
    pub rank: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FocusGroup {
    Keeps,
    Shape,
    Runs,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RailGroup {
    Management,
}

const fn focus(group: FocusGroup, rank: u8) -> Option<Focus> {
    Some(Focus { group, rank })
}

const fn view(
    name: &'static str,
    icon: &'static str,
    label: &'static str,
    lucide_icon: &'static str,
    pinnable: bool,
    focus: Option<Focus>,
    description: &'static str,
    source: &'static str,
    keywords: &'static [&'static str],
    rail_group: Option<RailGroup>,
    rail_rank: u8,
    date_nav: Option<DateNav>,
) -> AppDefinition {
    AppDefinition {
        name,
        icon,
        label,
        lucide_icon,
        pinnable,
        focus,
        description,
        source,
        keywords,
        rail_group,
        rail_rank,
        date_nav,
        has_background: false,
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct DateNav {
    pub allow_future: bool,
    pub step: Option<&'static str>,
    pub unit: DateNavUnit,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct DateNavUnit {
    pub one: &'static str,
    pub other: &'static str,
    pub none: &'static str,
}

const fn content_date_nav(one: &'static str, other: &'static str, none: &'static str) -> DateNav {
    DateNav {
        allow_future: false,
        step: None,
        unit: DateNavUnit { one, other, none },
    }
}

/// Agents are how an owner puts their journal to work, and nothing else in
/// the journal links to them, so they keep a management rail slot.
pub static AGENTS_APP: AppDefinition = view(
    "agents",
    "🤖",
    "agents",
    "bot",
    false,
    focus(FocusGroup::Runs, 3),
    "connect an agent to read and search your journal, within what you choose",
    "agents you connect",
    &["claude", "chatgpt", "mcp", "connect", "solstone.me"],
    Some(RailGroup::Management),
    3,
    None,
);

pub static APP_REGISTRY: &[AppDefinition] = &[
    view(
        "activities",
        "📅",
        "activities",
        "calendar-days",
        true,
        focus(FocusGroup::Keeps, 1),
        "what you did each day, sorted by facet",
        "written up by your journal from what you shared",
        &["day", "calendar", "facets", "did"],
        None,
        0,
        Some(DateNav {
            allow_future: true,
            step: None,
            unit: DateNavUnit {
                one: "activity",
                other: "activities",
                none: "no activities",
            },
        }),
    ),
    view(
        "backup",
        "🛡️",
        "backup",
        "history",
        true,
        focus(FocusGroup::Runs, 4),
        "an encrypted copy of your journal that only you can read",
        "your own storage, or storage sol pbc operates",
        &["restore", "key", "encrypted", "copy"],
        None,
        0,
        None,
    ),
    view(
        "body",
        "🫀",
        "body",
        "heart-pulse",
        true,
        focus(FocusGroup::Keeps, 4),
        "sleep, workouts and heart readings",
        "workouts, from a Strava download on the import page",
        &[
            "health", "sleep", "workout", "strava", "oura", "apple", "heart",
        ],
        None,
        0,
        Some(content_date_nav("reading", "readings", "no readings")),
    ),
    view(
        "curation",
        "✨",
        "suggestions",
        "wand-sparkles",
        true,
        focus(FocusGroup::Shape, 2),
        "changes your journal suggests, for you to accept or skip",
        "suggested by your journal as it works",
        &["curation", "review", "suggestions", "duplicates"],
        None,
        0,
        None,
    ),
    view(
        "entities",
        "📇",
        "entities",
        "contact",
        true,
        focus(FocusGroup::Shape, 0),
        "the people, companies, projects and tools your journal keeps, to edit, merge or hide",
        "names your journal found in what it keeps",
        &[
            "people",
            "names",
            "companies",
            "projects",
            "tools",
            "contacts",
        ],
        None,
        0,
        None,
    ),
    view(
        "health",
        "🩺",
        "health",
        "stethoscope",
        true,
        focus(FocusGroup::Runs, 1),
        "whether your journal is keeping up: processing, devices, errors",
        "your journal's own checks",
        &["status", "errors", "processing", "working", "problem"],
        None,
        0,
        None,
    ),
    view(
        "home",
        "🏠",
        "home",
        "house",
        false,
        focus(FocusGroup::Keeps, 0),
        "today and yesterday, written up, and anything your journal needs from you",
        "written by your journal from everything in it",
        &["today", "yesterday", "briefing", "week"],
        None,
        0,
        None,
    ),
    view(
        "import",
        "📥",
        "import",
        "import",
        false,
        focus(FocusGroup::Runs, 5),
        "bring in chats, calendars, notes, audio and workouts",
        "files you bring in",
        &[
            "upload", "chatgpt", "claude", "gemini", "calendar", "notes", "strava", "file",
        ],
        Some(RailGroup::Management),
        0,
        None,
    ),
    view(
        "map",
        "🗺️",
        "map",
        "map",
        true,
        focus(FocusGroup::Keeps, 3),
        "where you were, day by day, and the places you name",
        "from the solstone app on your phone, once location is on there",
        &["location", "places", "where", "gps"],
        None,
        0,
        Some(content_date_nav("record", "records", "no location")),
    ),
    view(
        "more",
        "⋯",
        "more",
        "ellipsis",
        false,
        None,
        "",
        "",
        &[],
        None,
        0,
        None,
    ),
    view(
        "network",
        "🔗",
        "network",
        "network",
        false,
        focus(FocusGroup::Runs, 2),
        "pair a phone or computer, and how your devices reach your journal",
        "your devices",
        &[
            "devices", "pair", "phone", "computer", "private", "link", "qr",
        ],
        Some(RailGroup::Management),
        2,
        None,
    ),
    view(
        "search",
        "🔍",
        "search",
        "search",
        true,
        focus(FocusGroup::Keeps, 5),
        "find words and phrases anywhere in your journal",
        "everything your journal keeps",
        &["find", "look"],
        None,
        0,
        None,
    ),
    view(
        "settings",
        "⚙️",
        "settings",
        "settings",
        false,
        focus(FocusGroup::Runs, 7),
        "your profile, keys, transcription, storage and facets",
        "",
        &[
            "profile",
            "facets",
            "keys",
            "storage",
            "retention",
            "transcription",
        ],
        Some(RailGroup::Management),
        1,
        None,
    ),
    view(
        "speakers",
        "🎙️",
        "speakers",
        "mic-vocal",
        true,
        focus(FocusGroup::Shape, 1),
        "the voices in your audio: teach yours, name others, fix who said what",
        "voices in the audio you shared",
        &["voice", "voices", "who", "said", "speaker"],
        None,
        0,
        Some(content_date_nav("segment", "segments", "no segments")),
    ),
    view(
        "stats",
        "📊",
        "stats",
        "chart-column",
        true,
        focus(FocusGroup::Runs, 6),
        "how much your journal holds, and how much processing it has done",
        "your journal's own counts",
        &["usage", "tokens", "disk", "hours", "numbers"],
        None,
        0,
        None,
    ),
    view(
        "support",
        "🛟",
        "support",
        "life-buoy",
        true,
        focus(FocusGroup::Runs, 8),
        "make a report, or track one you made",
        "",
        &["help", "report", "bug", "problem"],
        None,
        0,
        None,
    ),
    view(
        "thinking",
        "🧠",
        "thinking",
        "brain",
        true,
        focus(FocusGroup::Runs, 0),
        "where the thinking happens, and every run",
        "on your journal's computer, on confidential processing operated by sol pbc, or with your own model",
        &[
            "model",
            "ai",
            "processing",
            "local",
            "claude",
            "gemini",
            "gpt",
            "runs",
        ],
        None,
        0,
        None,
    ),
    view(
        "transcripts",
        "📜",
        "transcripts",
        "scroll-text",
        true,
        focus(FocusGroup::Keeps, 2),
        "what was said and on screen, moment by moment, with the audio",
        "from your mic and screen through the solstone app, and from imports",
        &["audio", "screen", "recording", "said", "heard", "listen"],
        None,
        0,
        Some(content_date_nav("segment", "segments", "no segments")),
    ),
];

#[derive(Debug, Clone, Serialize)]
pub struct ShellPayload {
    pub apps: Vec<ShellApp>,
    pub settings: ShellSettings,
    pub version: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShellApp {
    pub app_bar: bool,
    pub background_url: Option<String>,
    pub date_nav: Option<DateNav>,
    pub description: &'static str,
    pub focus: Option<Focus>,
    pub icon: &'static str,
    pub icon_svg: Option<String>,
    pub keywords: &'static [&'static str],
    pub label: &'static str,
    pub name: &'static str,
    pub pinnable: bool,
    pub rail_group: Option<RailGroup>,
    pub rail_rank: u8,
    pub source: &'static str,
    pub workspace_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShellSettings {
    pub reporting_enabled: bool,
}

pub fn known_app(name: &str) -> Option<&'static AppDefinition> {
    APP_REGISTRY.iter().find(|app| app.name == name)
}

/// Every app one router serves: the registry, plus the agents app when its
/// owner routes are mounted. The shell payload, the session gate and the
/// bare-path redirect all read this one list, so an app that is mounted only
/// sometimes cannot be offered in the shell and missing from the redirect.
pub fn served_apps(include_agents: bool) -> impl Iterator<Item = &'static AppDefinition> {
    APP_REGISTRY
        .iter()
        .chain(include_agents.then_some(&AGENTS_APP))
}

pub fn served_app(name: &str, include_agents: bool) -> Option<&'static AppDefinition> {
    served_apps(include_agents).find(|app| app.name == name)
}

pub fn shell_payload() -> ShellPayload {
    shell_payload_with_agents(false)
}

pub fn shell_payload_with_agents(include_agents: bool) -> ShellPayload {
    let icons: HashMap<String, String> = serde_json::from_slice(
        assets::lookup("/static/icons/lucide.json")
            .expect("embedded lucide icon catalogue")
            .bytes,
    )
    .expect("embedded lucide icon catalogue parses");
    ShellPayload {
        apps: served_apps(include_agents)
            .map(|app| ShellApp {
                app_bar: true,
                background_url: app
                    .has_background
                    .then(|| format!("/app/{}/background", app.name)),
                date_nav: app.date_nav,
                description: app.description,
                focus: app.focus,
                icon: app.icon,
                icon_svg: icons.get(app.lucide_icon).cloned(),
                keywords: app.keywords,
                label: app.label,
                name: app.name,
                pinnable: app.pinnable,
                rail_group: app.rail_group,
                rail_rank: app.rail_rank,
                source: app.source,
                workspace_url: format!("/app/{}/workspace", app.name),
            })
            .collect(),
        settings: ShellSettings {
            reporting_enabled: true,
        },
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    use super::{
        APP_REGISTRY, FocusGroup, RailGroup, known_app, shell_payload, shell_payload_with_agents,
    };

    #[test]
    fn agents_app_is_only_exposed_when_the_owner_routes_are_present() {
        assert!(!shell_payload().apps.iter().any(|app| app.name == "agents"));
        assert!(known_app("agents").is_none());
        let enabled = shell_payload_with_agents(true);
        assert_eq!(
            enabled
                .apps
                .iter()
                .filter(|app| app.name == "agents")
                .count(),
            1
        );
    }

    struct EstablishedJournal(tempfile::TempDir);

    impl EstablishedJournal {
        fn new() -> Self {
            let dir = tempfile::TempDir::new_in("/var/tmp").expect("journal root");
            fs::create_dir(dir.path().join("config")).expect("config directory");
            fs::write(
                dir.path().join("config/journal.json"),
                br#"{"setup":{"completed_at":1767225600}}"#,
            )
            .expect("journal config");
            Self(dir)
        }

        fn path(&self) -> &std::path::Path {
            self.0.path()
        }
    }

    #[test]
    fn body_remains_a_converted_native_registry_entry() {
        assert!(APP_REGISTRY.iter().any(|app| app.name == "body"));
    }

    #[test]
    fn network_is_a_converted_native_registry_entry() {
        assert!(APP_REGISTRY.iter().any(|app| app.name == "network"));
    }

    #[test]
    fn management_rail_apps_are_ordered_and_network_focus_is_runs() {
        let mut management_apps: Vec<_> = APP_REGISTRY
            .iter()
            .filter(|app| matches!(app.rail_group, Some(RailGroup::Management)))
            .collect();
        management_apps.sort_by_key(|app| app.rail_rank);
        let names: Vec<&str> = management_apps.iter().map(|app| app.name).collect();
        assert_eq!(names, &["import", "settings", "network"]);

        let network = APP_REGISTRY
            .iter()
            .find(|app| app.name == "network")
            .expect("network app is present");
        assert_eq!(
            network.focus.map(|focus| focus.group),
            Some(FocusGroup::Runs)
        );
    }

    #[test]
    fn more_app_is_served_and_chat_news_devices_stay_absent() {
        let more = known_app("more").expect("more is served");
        assert_eq!(more.label, "more");
        assert!(!more.pinnable);
        assert!(more.focus.is_none());
        assert!(more.rail_group.is_none());
        assert!(known_app("chat").is_none());
        assert!(known_app("news").is_none());
        assert!(known_app("devices").is_none());
        let payload = shell_payload();
        assert_eq!(
            payload.apps.iter().filter(|app| app.name == "more").count(),
            1
        );
    }

    #[test]
    fn shell_payload_serializes_new_fields_and_omits_top_level_pinned_cache() {
        let encoded = serde_json::to_value(shell_payload_with_agents(true))
            .expect("shell payload serializes");
        assert!(encoded.get("pinned").is_none());
        let apps = encoded["apps"].as_array().expect("apps array");
        let mut grouped: std::collections::BTreeMap<&str, Vec<(&str, u64)>> =
            std::collections::BTreeMap::new();
        for app in apps {
            let name = app["name"].as_str().expect("name");
            assert!(app["pinnable"].is_boolean(), "{name} pinnable");
            assert!(app["description"].is_string(), "{name} description");
            assert!(app["source"].is_string(), "{name} source");
            let keywords = app["keywords"].as_array().expect("keywords");
            assert!(keywords.iter().all(|keyword| keyword.is_string()));
            match app.get("focus") {
                Some(serde_json::Value::Null) => assert_eq!(name, "more"),
                Some(focus) => {
                    let group = focus["group"].as_str().expect("focus group");
                    let rank = focus["rank"].as_u64().expect("focus rank");
                    assert!(matches!(group, "keeps" | "shape" | "runs"), "{name}");
                    grouped.entry(group).or_default().push((name, rank));
                }
                None => panic!("{name} has no focus"),
            }
            if name == "more" {
                assert_eq!(app["description"], "");
                assert_eq!(app["pinnable"], false);
                assert!(app["rail_group"].is_null());
            } else {
                assert!(!app["description"].as_str().expect("description").is_empty());
            }
            assert!(app.get("launcher_group").is_none());
            assert!(app.get("launcher_rank").is_none());
        }
        for (group, names) in [
            (
                "keeps",
                &["home", "activities", "transcripts", "map", "body", "search"][..],
            ),
            ("shape", &["entities", "speakers", "curation"][..]),
            (
                "runs",
                &[
                    "thinking", "health", "network", "agents", "backup", "import", "stats",
                    "settings", "support",
                ][..],
            ),
        ] {
            let mut rows = grouped.remove(group).expect(group);
            rows.sort_by_key(|(_, rank)| *rank);
            let ordered: Vec<&str> = rows.iter().map(|(name, _)| *name).collect();
            let ranks: Vec<u64> = rows.iter().map(|(_, rank)| *rank).collect();
            assert_eq!(ordered, names, "{group}");
            assert_eq!(
                ranks,
                (0..names.len() as u64).collect::<Vec<_>>(),
                "{group}"
            );
        }
        assert!(grouped.is_empty());
        let curation = apps.iter().find(|app| app["name"] == "curation").unwrap();
        assert_eq!(curation["label"], "suggestions");
    }

    #[test]
    fn devices_is_removed_from_the_registry() {
        assert!(!APP_REGISTRY.iter().any(|app| app.name == "devices"));
        assert!(known_app("devices").is_none());
    }

    #[test]
    fn news_is_removed_from_the_registry() {
        assert!(!APP_REGISTRY.iter().any(|app| app.name == "news"));
        assert!(known_app("news").is_none());
    }

    #[test]
    fn chat_is_removed_from_the_registry() {
        assert!(!APP_REGISTRY.iter().any(|app| app.name == "chat"));
        assert!(known_app("chat").is_none());
    }

    #[test]
    fn shell_payload_omits_chat_app_and_chat_bar() {
        let payload = shell_payload();
        assert!(!payload.apps.iter().any(|app| {
            app.name == "chat" || app.label == "chat" || app.workspace_url.contains("chat")
        }));
        let encoded = serde_json::to_value(&payload).expect("shell payload serializes");
        assert!(encoded.get("chat_bar").is_none());
    }

    #[test]
    fn shell_payload_omits_devices_entirely() {
        let payload = shell_payload();
        assert!(!payload.apps.iter().any(|app| {
            app.name == "devices" || app.label == "devices" || app.workspace_url.contains("devices")
        }));
    }

    #[test]
    fn stats_is_converted_and_tokens_is_removed_from_the_registry() {
        let stats: Vec<_> = APP_REGISTRY
            .iter()
            .filter(|app| app.name == "stats")
            .collect();
        assert_eq!(stats.len(), 1);
        assert!(!APP_REGISTRY.iter().any(|app| app.name == "tokens"));
    }

    #[test]
    fn shell_payload_lists_stats_once_and_omits_tokens_entirely() {
        let payload = shell_payload();
        let stats: Vec<_> = payload
            .apps
            .iter()
            .filter(|app| app.name == "stats")
            .collect();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].workspace_url, "/app/stats/workspace");
        assert!(!payload.apps.iter().any(|app| {
            app.name == "tokens" || app.label == "tokens" || app.workspace_url.contains("tokens")
        }));
    }

    #[test]
    fn thinking_is_converted_and_sol_is_removed_from_the_registry() {
        let thinking: Vec<_> = APP_REGISTRY
            .iter()
            .filter(|app| app.name == "thinking")
            .collect();
        assert_eq!(thinking.len(), 1);
        assert!(known_app("sol").is_none());
    }

    #[test]
    fn shell_payload_lists_thinking_once_and_omits_sol_entirely() {
        let payload = shell_payload();
        let thinking: Vec<_> = payload
            .apps
            .iter()
            .filter(|app| app.name == "thinking")
            .collect();
        assert_eq!(thinking.len(), 1);
        assert!(!payload.apps.iter().any(|app| {
            app.name == "sol" || app.label == "sol" || app.workspace_url.contains("sol")
        }));
    }

    #[tokio::test]
    async fn stats_paths_are_native_and_tokens_paths_fall_back_to_unknown_app_404() {
        let journal = EstablishedJournal::new();
        let app = crate::router(journal.path().to_path_buf());

        for path in [
            "/app/stats/",
            "/app/stats/workspace",
            "/app/stats/api/usage?day=20260809",
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert!(
                response.status().is_success(),
                "{path}: {}",
                response.status()
            );
        }

        let mut first_404_body = None;
        for path in [
            "/app/stats/not-a-native-route",
            "/app/tokens/",
            "/app/tokens/workspace",
            "/app/tokens/api/usage?day=20260809",
        ] {
            let response = app
                .clone()
                .oneshot(Request::get(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(
                response.headers().get(header::CONTENT_TYPE).unwrap(),
                "text/html; charset=utf-8",
                "{path}"
            );
            assert!(response.headers().get(header::LOCATION).is_none(), "{path}");
            let body = to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec();
            assert!(
                serde_json::from_slice::<serde_json::Value>(&body).is_err(),
                "{path} must not be the typed JSON refusal"
            );
            if let Some(expected) = &first_404_body {
                assert_eq!(&body, expected, "{path}");
            } else {
                first_404_body = Some(body);
            }
        }
    }
}
