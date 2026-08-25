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
    pub launcher_group: AppLauncherGroup,
    pub launcher_rank: u8,
    pub rail_group: Option<RailGroup>,
    pub rail_rank: u8,
    pub date_nav: Option<DateNav>,
    pub has_background: bool,
    pub converted: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AppLauncherGroup {
    YourJournal,
    Understand,
    Manage,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RailGroup {
    Primary,
    Management,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct DateNav {
    pub allow_future: bool,
    pub step: Option<&'static str>,
    pub unit: DateNavUnit,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(untagged)]
pub enum DateNavUnit {
    Content {
        one: &'static str,
        other: &'static str,
        none: &'static str,
    },
    Currency {
        kind: &'static str,
    },
}

const fn content_date_nav(one: &'static str, other: &'static str, none: &'static str) -> DateNav {
    DateNav {
        allow_future: false,
        step: None,
        unit: DateNavUnit::Content { one, other, none },
    }
}

pub static APP_REGISTRY: &[AppDefinition] = &[
    AppDefinition {
        name: "activities",
        icon: "📅",
        label: "activities",
        lucide_icon: "calendar-days",
        launcher_group: AppLauncherGroup::Understand,
        launcher_rank: 5,
        rail_group: None,
        rail_rank: 0,
        date_nav: Some(DateNav {
            allow_future: true,
            step: None,
            unit: DateNavUnit::Content {
                one: "activity",
                other: "activities",
                none: "no activities",
            },
        }),
        has_background: false,
        converted: false,
    },
    AppDefinition {
        name: "backup",
        icon: "🛡️",
        label: "backup",
        lucide_icon: "history",
        launcher_group: AppLauncherGroup::Manage,
        launcher_rank: 2,
        rail_group: None,
        rail_rank: 0,
        date_nav: None,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "body",
        icon: "🫀",
        label: "body",
        lucide_icon: "heart-pulse",
        launcher_group: AppLauncherGroup::YourJournal,
        launcher_rank: 4,
        rail_group: None,
        rail_rank: 0,
        date_nav: Some(content_date_nav("reading", "readings", "no readings")),
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "curation",
        icon: "✨",
        label: "curation",
        lucide_icon: "wand-sparkles",
        launcher_group: AppLauncherGroup::Understand,
        launcher_rank: 4,
        rail_group: None,
        rail_rank: 0,
        date_nav: None,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "entities",
        icon: "📇",
        label: "entities",
        lucide_icon: "contact",
        launcher_group: AppLauncherGroup::Understand,
        launcher_rank: 1,
        rail_group: Some(RailGroup::Primary),
        rail_rank: 3,
        date_nav: None,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "health",
        icon: "🩺",
        label: "health",
        lucide_icon: "stethoscope",
        launcher_group: AppLauncherGroup::Manage,
        launcher_rank: 3,
        rail_group: None,
        rail_rank: 0,
        date_nav: None,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "home",
        icon: "🏠",
        label: "home",
        lucide_icon: "house",
        launcher_group: AppLauncherGroup::YourJournal,
        launcher_rank: 0,
        rail_group: Some(RailGroup::Primary),
        rail_rank: 0,
        date_nav: None,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "import",
        icon: "📥",
        label: "import",
        lucide_icon: "import",
        launcher_group: AppLauncherGroup::Manage,
        launcher_rank: 0,
        rail_group: Some(RailGroup::Management),
        rail_rank: 0,
        date_nav: None,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "network",
        icon: "🔗",
        label: "network",
        lucide_icon: "network",
        launcher_group: AppLauncherGroup::Manage,
        launcher_rank: 1,
        rail_group: None,
        rail_rank: 0,
        date_nav: None,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "news",
        icon: "📰",
        label: "newsletters",
        lucide_icon: "newspaper",
        launcher_group: AppLauncherGroup::YourJournal,
        launcher_rank: 5,
        rail_group: None,
        rail_rank: 0,
        date_nav: Some(content_date_nav(
            "newsletter",
            "newsletters",
            "no newsletters",
        )),
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "search",
        icon: "🔍",
        label: "search",
        lucide_icon: "search",
        launcher_group: AppLauncherGroup::Understand,
        launcher_rank: 0,
        rail_group: Some(RailGroup::Primary),
        rail_rank: 2,
        date_nav: None,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "settings",
        icon: "⚙️",
        label: "settings",
        lucide_icon: "settings",
        launcher_group: AppLauncherGroup::Manage,
        launcher_rank: 5,
        rail_group: Some(RailGroup::Management),
        rail_rank: 1,
        date_nav: None,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "speakers",
        icon: "🎙️",
        label: "speakers",
        lucide_icon: "mic-vocal",
        launcher_group: AppLauncherGroup::YourJournal,
        launcher_rank: 3,
        rail_group: None,
        rail_rank: 0,
        date_nav: Some(content_date_nav("segment", "segments", "no segments")),
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "stats",
        icon: "📊",
        label: "stats",
        lucide_icon: "chart-column",
        launcher_group: AppLauncherGroup::Understand,
        launcher_rank: 3,
        rail_group: None,
        rail_rank: 0,
        date_nav: None,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "support",
        icon: "🛟",
        label: "support",
        lucide_icon: "life-buoy",
        launcher_group: AppLauncherGroup::Manage,
        launcher_rank: 4,
        rail_group: None,
        rail_rank: 0,
        date_nav: None,
        has_background: true,
        converted: true,
    },
    AppDefinition {
        name: "thinking",
        icon: "🧠",
        label: "thinking",
        lucide_icon: "brain",
        launcher_group: AppLauncherGroup::Understand,
        launcher_rank: 2,
        rail_group: Some(RailGroup::Primary),
        rail_rank: 4,
        date_nav: None,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "timeline",
        icon: "🕰️",
        label: "timeline",
        lucide_icon: "calendar-range",
        launcher_group: AppLauncherGroup::YourJournal,
        launcher_rank: 1,
        rail_group: Some(RailGroup::Primary),
        rail_rank: 1,
        date_nav: Some(content_date_nav("segment", "segments", "no segments")),
        has_background: true,
        converted: true,
    },
    AppDefinition {
        name: "transcripts",
        icon: "📜",
        label: "transcripts",
        lucide_icon: "scroll-text",
        launcher_group: AppLauncherGroup::YourJournal,
        launcher_rank: 2,
        rail_group: None,
        rail_rank: 0,
        date_nav: Some(content_date_nav("segment", "segments", "no segments")),
        has_background: false,
        converted: true,
    },
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
    pub icon: &'static str,
    pub icon_svg: Option<String>,
    pub label: &'static str,
    pub launcher_group: AppLauncherGroup,
    pub launcher_rank: u8,
    pub name: &'static str,
    pub rail_group: Option<RailGroup>,
    pub rail_rank: u8,
    pub workspace_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShellSettings {
    pub reporting_enabled: bool,
}

pub fn known_app(name: &str) -> Option<&'static AppDefinition> {
    APP_REGISTRY.iter().find(|app| app.name == name)
}

pub fn shell_payload() -> ShellPayload {
    let icons: HashMap<String, String> = serde_json::from_slice(
        assets::lookup("/static/icons/lucide.json")
            .expect("embedded lucide icon catalogue")
            .bytes,
    )
    .expect("embedded lucide icon catalogue parses");
    ShellPayload {
        apps: APP_REGISTRY
            .iter()
            .map(|app| ShellApp {
                app_bar: true,
                background_url: app
                    .has_background
                    .then(|| format!("/app/{}/background", app.name)),
                date_nav: app.date_nav,
                icon: app.icon,
                icon_svg: icons.get(app.lucide_icon).cloned(),
                label: app.label,
                launcher_group: app.launcher_group,
                launcher_rank: app.launcher_rank,
                name: app.name,
                rail_group: app.rail_group,
                rail_rank: app.rail_rank,
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
    use std::{collections::BTreeSet, fs};

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    use super::{APP_REGISTRY, known_app, shell_payload};

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
        assert!(
            APP_REGISTRY
                .iter()
                .any(|app| app.name == "body" && app.converted)
        );
    }

    #[test]
    fn network_is_a_converted_native_registry_entry() {
        assert!(
            APP_REGISTRY
                .iter()
                .any(|app| app.name == "network" && app.converted)
        );
    }

    #[test]
    #[test]
    fn devices_is_removed_from_the_registry() {
        assert!(!APP_REGISTRY.iter().any(|app| app.name == "devices"));
        assert!(known_app("devices").is_none());
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
        assert!(stats[0].converted);
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
        assert!(thinking[0].converted);
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
