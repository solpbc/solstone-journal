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
    pub date_nav: Option<DateNav>,
    pub facets_enabled: bool,
    pub has_background: bool,
    pub converted: bool,
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
        date_nav: Some(DateNav {
            allow_future: true,
            step: None,
            unit: DateNavUnit::Content {
                one: "activity",
                other: "activities",
                none: "no activities",
            },
        }),
        facets_enabled: true,
        has_background: false,
        converted: false,
    },
    AppDefinition {
        name: "backup",
        icon: "🛡️",
        label: "backup",
        lucide_icon: "history",
        date_nav: None,
        facets_enabled: false,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "body",
        icon: "🫀",
        label: "body",
        lucide_icon: "heart-pulse",
        date_nav: Some(content_date_nav("reading", "readings", "no readings")),
        facets_enabled: false,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "chat",
        icon: "💬",
        label: "chat",
        lucide_icon: "message-circle",
        date_nav: Some(content_date_nav("message", "messages", "no messages")),
        facets_enabled: true,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "curation",
        icon: "✨",
        label: "curation",
        lucide_icon: "wand-sparkles",
        date_nav: None,
        facets_enabled: false,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "entities",
        icon: "📇",
        label: "entities",
        lucide_icon: "contact",
        date_nav: None,
        facets_enabled: true,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "health",
        icon: "🩺",
        label: "health",
        lucide_icon: "stethoscope",
        date_nav: None,
        facets_enabled: false,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "home",
        icon: "🏠",
        label: "home",
        lucide_icon: "house",
        date_nav: None,
        facets_enabled: false,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "import",
        icon: "📥",
        label: "import",
        lucide_icon: "import",
        date_nav: None,
        facets_enabled: true,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "network",
        icon: "🔗",
        label: "network",
        lucide_icon: "network",
        date_nav: None,
        facets_enabled: false,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "news",
        icon: "📰",
        label: "newsletters",
        lucide_icon: "newspaper",
        date_nav: Some(content_date_nav(
            "newsletter",
            "newsletters",
            "no newsletters",
        )),
        facets_enabled: false,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "search",
        icon: "🔍",
        label: "search",
        lucide_icon: "search",
        date_nav: None,
        facets_enabled: true,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "settings",
        icon: "⚙️",
        label: "settings",
        lucide_icon: "settings",
        date_nav: None,
        facets_enabled: true,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "speakers",
        icon: "🎙️",
        label: "speakers",
        lucide_icon: "mic-vocal",
        date_nav: Some(content_date_nav("segment", "segments", "no segments")),
        facets_enabled: false,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "stats",
        icon: "📊",
        label: "stats",
        lucide_icon: "chart-column",
        date_nav: None,
        facets_enabled: false,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "support",
        icon: "🛟",
        label: "support",
        lucide_icon: "life-buoy",
        date_nav: None,
        facets_enabled: false,
        has_background: true,
        converted: true,
    },
    AppDefinition {
        name: "thinking",
        icon: "🧠",
        label: "thinking",
        lucide_icon: "brain",
        date_nav: None,
        facets_enabled: false,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "timeline",
        icon: "🕰️",
        label: "timeline",
        lucide_icon: "calendar-range",
        date_nav: Some(content_date_nav("segment", "segments", "no segments")),
        facets_enabled: false,
        has_background: true,
        converted: true,
    },
    AppDefinition {
        name: "transcripts",
        icon: "📜",
        label: "transcripts",
        lucide_icon: "scroll-text",
        date_nav: Some(content_date_nav("segment", "segments", "no segments")),
        facets_enabled: false,
        has_background: false,
        converted: true,
    },
];

#[derive(Debug, Clone, Serialize)]
pub struct ShellPayload {
    pub apps: Vec<ShellApp>,
    pub chat_bar: ChatBar,
    pub facets: Vec<serde_json::Value>,
    pub selected_facet: Option<serde_json::Value>,
    pub settings: ShellSettings,
    pub version: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct ShellApp {
    pub app_bar: bool,
    pub background_url: Option<String>,
    pub date_nav: Option<DateNav>,
    pub facets_enabled: bool,
    pub icon: &'static str,
    pub icon_svg: Option<String>,
    pub label: &'static str,
    pub name: &'static str,
    pub starred: bool,
    pub workspace_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatBar {
    pub attention: Option<serde_json::Value>,
    pub placeholder: &'static str,
    pub sol_request: Option<serde_json::Value>,
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
                facets_enabled: app.facets_enabled,
                icon: app.icon,
                icon_svg: icons.get(app.lucide_icon).cloned(),
                label: app.label,
                name: app.name,
                starred: false,
                workspace_url: format!("/app/{}/workspace", app.name),
            })
            .collect(),
        chat_bar: ChatBar {
            attention: None,
            placeholder: "send a message…",
            sol_request: None,
        },
        facets: Vec::new(),
        selected_facet: None,
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
    fn devices_is_removed_from_the_registry() {
        assert!(!APP_REGISTRY.iter().any(|app| app.name == "devices"));
        assert!(known_app("devices").is_none());
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
