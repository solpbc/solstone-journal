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
        converted: false,
    },
    AppDefinition {
        name: "body",
        icon: "🫀",
        label: "body",
        lucide_icon: "heart-pulse",
        date_nav: Some(content_date_nav("reading", "readings", "no readings")),
        facets_enabled: false,
        has_background: false,
        converted: false,
    },
    AppDefinition {
        name: "chat",
        icon: "💬",
        label: "chat",
        lucide_icon: "message-circle",
        date_nav: Some(content_date_nav("message", "messages", "no messages")),
        facets_enabled: true,
        has_background: false,
        converted: false,
    },
    AppDefinition {
        name: "curation",
        icon: "✨",
        label: "curation",
        lucide_icon: "wand-sparkles",
        date_nav: None,
        facets_enabled: false,
        has_background: false,
        converted: false,
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
        converted: false,
    },
    AppDefinition {
        name: "home",
        icon: "🏠",
        label: "home",
        lucide_icon: "house",
        date_nav: None,
        facets_enabled: false,
        has_background: false,
        converted: false,
    },
    AppDefinition {
        name: "import",
        icon: "📥",
        label: "import",
        lucide_icon: "import",
        date_nav: None,
        facets_enabled: true,
        has_background: false,
        converted: false,
    },
    AppDefinition {
        name: "network",
        icon: "🔗",
        label: "network",
        lucide_icon: "network",
        date_nav: None,
        facets_enabled: false,
        has_background: false,
        converted: false,
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
        converted: false,
    },
    AppDefinition {
        name: "devices",
        icon: "📡",
        label: "devices",
        lucide_icon: "antenna",
        date_nav: None,
        facets_enabled: false,
        has_background: false,
        converted: true,
    },
    AppDefinition {
        name: "reflections",
        icon: "🌙",
        label: "reflections",
        lucide_icon: "moon",
        date_nav: Some(DateNav {
            allow_future: false,
            step: Some("week"),
            unit: DateNavUnit::Content {
                one: "reflection",
                other: "reflections",
                none: "no reflection",
            },
        }),
        facets_enabled: false,
        has_background: false,
        converted: false,
    },
    AppDefinition {
        name: "search",
        icon: "🔍",
        label: "search",
        lucide_icon: "search",
        date_nav: None,
        facets_enabled: true,
        has_background: false,
        converted: false,
    },
    AppDefinition {
        name: "settings",
        icon: "⚙️",
        label: "settings",
        lucide_icon: "settings",
        date_nav: None,
        facets_enabled: true,
        has_background: false,
        converted: false,
    },
    AppDefinition {
        name: "sol",
        icon: "🦾",
        label: "sol",
        lucide_icon: "bot",
        date_nav: Some(content_date_nav("run", "runs", "no runs")),
        facets_enabled: true,
        has_background: false,
        converted: false,
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
        converted: false,
    },
    AppDefinition {
        name: "support",
        icon: "🛟",
        label: "support",
        lucide_icon: "life-buoy",
        date_nav: None,
        facets_enabled: false,
        has_background: true,
        converted: false,
    },
    AppDefinition {
        name: "thinking",
        icon: "🧠",
        label: "thinking",
        lucide_icon: "brain",
        date_nav: None,
        facets_enabled: false,
        has_background: false,
        converted: false,
    },
    AppDefinition {
        name: "timeline",
        icon: "🕰️",
        label: "timeline",
        lucide_icon: "calendar-range",
        date_nav: Some(content_date_nav("segment", "segments", "no segments")),
        facets_enabled: false,
        has_background: true,
        converted: false,
    },
    AppDefinition {
        name: "tokens",
        icon: "💰",
        label: "tokens",
        lucide_icon: "coins",
        date_nav: Some(DateNav {
            allow_future: false,
            step: None,
            unit: DateNavUnit::Currency { kind: "currency" },
        }),
        facets_enabled: false,
        has_background: false,
        converted: false,
    },
    AppDefinition {
        name: "transcripts",
        icon: "📜",
        label: "transcripts",
        lucide_icon: "scroll-text",
        date_nav: Some(content_date_nav("segment", "segments", "no segments")),
        facets_enabled: false,
        has_background: false,
        converted: false,
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
