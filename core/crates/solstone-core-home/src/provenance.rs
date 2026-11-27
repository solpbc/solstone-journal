// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::path::Path;

use solstone_core_segment::{StreamAllocationBase, list_stream_records};
use solstone_core_sol_link::Platform;
use solstone_core_sol_link::client_description_store::read_descriptions;
use solstone_core_sol_link::ledger::{AuthorizedClientsRead, ClientEntry, read_authorized_clients};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceWord {
    Watch,
    Terminal,
    Phone,
    Tablet,
    Computer,
    Mac,
    Iphone,
    Ipad,
    Browser,
}

impl DeviceWord {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Watch => "watch",
            Self::Terminal => "terminal",
            Self::Phone => "phone",
            Self::Tablet => "tablet",
            Self::Computer => "computer",
            Self::Mac => "mac",
            Self::Iphone => "iphone",
            Self::Ipad => "ipad",
            Self::Browser => "browser",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WhatWord {
    Audio,
    Screen,
    Camera,
    Location,
    Calendar,
    Notes,
    Screenshots,
    Photos,
}

impl WhatWord {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Screen => "screen",
            Self::Camera => "camera",
            Self::Location => "location",
            Self::Calendar => "calendar",
            Self::Notes => "Obsidian notes",
            Self::Screenshots => "screenshots",
            Self::Photos => "photos",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProvenancePart {
    Import {
        source: String,
    },
    Photos,
    Agent,
    Bound {
        device: Option<DeviceWord>,
        what: Option<WhatWord>,
        identity: Option<String>,
    },
    Unbound {
        device: DeviceWord,
        identity: Option<String>,
    },
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DeviceGroup {
    Named(DeviceWord),
    Generic,
}

impl ProvenancePart {
    pub fn device_word(&self) -> Option<DeviceWord> {
        match self {
            Self::Bound { device, .. } => *device,
            Self::Unbound { device, .. } => Some(*device),
            _ => None,
        }
    }

    fn device_group(&self) -> Option<DeviceGroup> {
        match self {
            Self::Bound {
                device: Some(device),
                ..
            }
            | Self::Unbound { device, .. } => Some(DeviceGroup::Named(*device)),
            Self::Bound { device: None, .. } => Some(DeviceGroup::Generic),
            _ => None,
        }
    }

    pub fn identity(&self) -> Option<&str> {
        match self {
            Self::Bound { identity, .. } => identity.as_deref(),
            Self::Unbound { identity, .. } => identity.as_deref(),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LabelTableStatus {
    Failed,
    Partial,
    Complete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamLabelRow {
    pub name: String,
    pub part: ProvenancePart,
    pub created_at: u64,
    /// True when `part.identity()` came from an explicit owner-set
    /// `owner_label` in `link/client-descriptions.json` rather than an
    /// auto-reported client name, ledger label, or unbound host stem.
    pub owner_named: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StreamLabelTable {
    pub status: LabelTableStatus,
    pub rows: Vec<StreamLabelRow>,
}

pub fn load_stream_labels(journal: &Path) -> StreamLabelTable {
    let records = match list_stream_records(journal) {
        Ok(records) => records,
        Err(_) => {
            return StreamLabelTable {
                status: LabelTableStatus::Failed,
                rows: Vec::new(),
            };
        }
    };

    let mut partial = false;

    let descriptions = match read_descriptions(journal) {
        Ok(descriptions) => descriptions,
        Err(_) => {
            partial = true;
            BTreeMap::new()
        }
    };

    let ledger_path = journal.join("link/authorized_clients.json");
    let ledger_clients: BTreeMap<String, ClientEntry> = match read_authorized_clients(&ledger_path)
    {
        AuthorizedClientsRead::Present(clients) => clients
            .into_iter()
            .map(|entry| (entry.fingerprint.clone(), entry))
            .collect(),
        AuthorizedClientsRead::Missing => BTreeMap::new(),
        AuthorizedClientsRead::Unreadable
        | AuthorizedClientsRead::Malformed
        | AuthorizedClientsRead::DuplicateCid => {
            partial = true;
            BTreeMap::new()
        }
    };

    let mut rows = Vec::with_capacity(records.len());
    for (stem, record) in records {
        let (part, owner_named) =
            classify_record_with_origin(&stem, &record, &descriptions, &ledger_clients);
        rows.push(StreamLabelRow {
            name: stem,
            part,
            created_at: record.created_at,
            owner_named,
        });
    }

    StreamLabelTable {
        status: if partial {
            LabelTableStatus::Partial
        } else {
            LabelTableStatus::Complete
        },
        rows,
    }
}

#[cfg(test)]
fn classify_record(
    stem: &str,
    record: &solstone_core_segment::StreamRecord,
    descriptions: &BTreeMap<
        String,
        solstone_core_sol_link::client_description::StoredClientDescription,
    >,
    ledger_clients: &BTreeMap<String, ClientEntry>,
) -> ProvenancePart {
    classify_record_with_origin(stem, record, descriptions, ledger_clients).0
}

fn classify_record_with_origin(
    stem: &str,
    record: &solstone_core_segment::StreamRecord,
    descriptions: &BTreeMap<
        String,
        solstone_core_sol_link::client_description::StoredClientDescription,
    >,
    ledger_clients: &BTreeMap<String, ClientEntry>,
) -> (ProvenancePart, bool) {
    // 1. Key import.<source> -> Import { source }
    if let Some(source) = stem.strip_prefix("import.") {
        return (
            ProvenancePart::Import {
                source: if stem.starts_with(solstone_core_format::content::granola::STREAM_PREFIX) {
                    "granola".to_owned()
                } else {
                    source.to_owned()
                },
            },
            false,
        );
    }

    // 2. record.kind == "agent_memory" -> Agent
    if record.kind == "agent_memory" {
        return (ProvenancePart::Agent, false);
    }

    // 3. Bound when cid and source are both Some (Some("") is bound)
    if let (Some(cid), Some(source)) = (record.cid.as_deref(), record.source.as_deref()) {
        let description = descriptions.get(cid);
        let ledger_entry = ledger_clients.get(cid);

        let (device, what, identity, owner_named) =
            classify_bound_source(source, record, description, ledger_entry);
        return (
            ProvenancePart::Bound {
                device,
                what,
                identity,
            },
            owner_named,
        );
    }

    // 3b. Synced photo-library stream (`photos` or source == "photos" without a single device cid).
    if stem == solstone_core_segment::PHOTOS_SOURCE
        || record.source.as_deref() == Some(solstone_core_segment::PHOTOS_SOURCE)
    {
        return (ProvenancePart::Photos, false);
    }

    // 4. Otherwise: unbound
    if record.kind == "browser" {
        return (
            ProvenancePart::Unbound {
                device: DeviceWord::Browser,
                identity: None,
            },
            false,
        );
    }

    if !stem.contains('.') && stem != "_default" {
        let description = record.cid.as_deref().and_then(|cid| descriptions.get(cid));
        let ledger_entry = record
            .cid
            .as_deref()
            .and_then(|cid| ledger_clients.get(cid));
        let platform = unbound_platform(record, description, ledger_entry);
        let device = match platform.as_deref() {
            Some("macos") => Some(DeviceWord::Mac),
            Some("linux") | Some("windows") => Some(DeviceWord::Computer),
            _ => None,
        };
        if let Some(device) = device {
            let ident = stem.trim();
            let identity = if ident.is_empty() {
                None
            } else {
                Some(ident.to_owned())
            };
            return (ProvenancePart::Unbound { device, identity }, false);
        }
        return (ProvenancePart::Unknown, false);
    }

    let dots = stem.chars().filter(|&c| c == '.').count();
    if dots == 1 {
        let Some((host, qualifier)) = stem.split_once('.') else {
            return (ProvenancePart::Unknown, false);
        };
        let device = match qualifier {
            "tmux" => Some(DeviceWord::Terminal),
            "watch" => Some(DeviceWord::Watch),
            "mobile" | "phone" => Some(DeviceWord::Phone),
            "browser" => Some(DeviceWord::Browser),
            _ => None,
        };
        if let Some(device) = device {
            let host = host.trim();
            let identity = if host.is_empty() {
                None
            } else {
                Some(host.to_owned())
            };
            return (ProvenancePart::Unbound { device, identity }, false);
        }
        return (ProvenancePart::Unknown, false);
    }

    // 5. _default and anything else -> Unknown
    (ProvenancePart::Unknown, false)
}

/// A read text fact, trimmed and lowercased; an empty value was not reported.
fn read_fact(value: Option<&str>) -> Option<String> {
    value
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

pub(crate) fn resolve_client_platform(
    reported_platform: Option<&str>,
    ledger_platform: Option<Platform>,
    allocation_platform_input: Option<&str>,
) -> Option<String> {
    read_fact(reported_platform)
        .or_else(|| ledger_platform.and_then(|plat| read_fact(Some(plat.as_wire()))))
        .or_else(|| read_fact(allocation_platform_input))
}

/// The platform a bound stream's facts report, first of: the client's reported
/// description, the pairing ledger hint, the record's allocation when it was
/// named from the platform.
fn bound_platform(
    record: &solstone_core_segment::StreamRecord,
    description: Option<&solstone_core_sol_link::client_description::StoredClientDescription>,
    ledger_entry: Option<&ClientEntry>,
) -> Option<String> {
    let reported = description
        .and_then(|d| d.reported.as_ref())
        .and_then(|r| r.platform.as_deref());
    let ledger_platform = ledger_entry.and_then(|e| e.platform);
    let allocation_platform_input = record
        .allocation
        .as_ref()
        .filter(|a| a.base == StreamAllocationBase::Platform)
        .map(|a| a.base_input.as_str());
    resolve_client_platform(reported, ledger_platform, allocation_platform_input)
}

/// An unbound record's platform: the bound sources when it carries a cid, then
/// the platform the older writer recorded on the record itself.
fn unbound_platform(
    record: &solstone_core_segment::StreamRecord,
    description: Option<&solstone_core_sol_link::client_description::StoredClientDescription>,
    ledger_entry: Option<&ClientEntry>,
) -> Option<String> {
    bound_platform(record, description, ledger_entry)
        .or_else(|| read_fact(record.platform.as_deref()))
}

fn classify_bound_source(
    source: &str,
    record: &solstone_core_segment::StreamRecord,
    description: Option<&solstone_core_sol_link::client_description::StoredClientDescription>,
    ledger_entry: Option<&ClientEntry>,
) -> (Option<DeviceWord>, Option<WhatWord>, Option<String>, bool) {
    let (identity, owner_named) = bound_identity(description, ledger_entry);

    let (device, what) = match source {
        "watch-audio" => (Some(DeviceWord::Watch), Some(WhatWord::Audio)),
        "tmux" => (Some(DeviceWord::Terminal), None),
        source if source.starts_with("browser") => (Some(DeviceWord::Browser), None),
        // An empty source is what ingest stores when a client names none: the
        // device's own recording, with nothing read about what it recorded.
        "" | "audio" | "screen" | "camera" | "location" | "calendar" | "obsidian"
        | "screenshots" | "photos" | "mobile-segment" | "mobile" => {
            let device_word = resolve_platform_device_word(record, description, ledger_entry);
            let what_word = match source {
                "audio" => Some(WhatWord::Audio),
                "screen" => Some(WhatWord::Screen),
                "camera" => Some(WhatWord::Camera),
                "location" => Some(WhatWord::Location),
                "calendar" => Some(WhatWord::Calendar),
                "obsidian" => Some(WhatWord::Notes),
                "screenshots" => Some(WhatWord::Screenshots),
                "photos" => Some(WhatWord::Photos),
                _ => None,
            };
            (device_word, what_word)
        }
        _ => (None, None),
    };

    (device, what, identity, owner_named)
}

fn resolve_platform_device_word(
    record: &solstone_core_segment::StreamRecord,
    description: Option<&solstone_core_sol_link::client_description::StoredClientDescription>,
    ledger_entry: Option<&ClientEntry>,
) -> Option<DeviceWord> {
    let platform = bound_platform(record, description, ledger_entry);

    let device_type = read_fact(
        description
            .and_then(|d| d.reported.as_ref())
            .and_then(|r| r.device_type.as_deref()),
    );
    let device_type = device_type.as_deref();

    match platform.as_deref() {
        Some("macos") => Some(DeviceWord::Mac),
        // An iPad's app reports its platform as `ipados`.
        Some("ios") | Some("ipados") => match device_type {
            Some("tablet") => Some(DeviceWord::Ipad),
            Some("phone") => Some(DeviceWord::Iphone),
            _ => None,
        },
        Some("android") => match device_type {
            Some("tablet") => Some(DeviceWord::Tablet),
            Some("phone") => Some(DeviceWord::Phone),
            _ => None,
        },
        Some("windows") | Some("linux") => match device_type {
            Some("terminal") => Some(DeviceWord::Terminal),
            Some("phone") => Some(DeviceWord::Phone),
            Some("tablet") => Some(DeviceWord::Tablet),
            _ => Some(DeviceWord::Computer),
        },
        _ => match device_type {
            Some("desktop") | Some("laptop") => Some(DeviceWord::Computer),
            Some("phone") => Some(DeviceWord::Phone),
            Some("tablet") => Some(DeviceWord::Tablet),
            Some("terminal") => Some(DeviceWord::Terminal),
            _ => None,
        },
    }
}

fn bound_identity(
    description: Option<&solstone_core_sol_link::client_description::StoredClientDescription>,
    ledger_entry: Option<&ClientEntry>,
) -> (Option<String>, bool) {
    if let Some(owner) = description
        .and_then(|d| d.owner_label.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return (Some(owner.to_owned()), true);
    }

    if let Some(reported_name) = description
        .and_then(|d| d.reported.as_ref())
        .and_then(|r| r.name.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return (Some(reported_name.to_owned()), false);
    }

    if let Some(entry) = ledger_entry {
        let base = entry.base_label().trim();
        if !base.is_empty() {
            if entry.label_ordinal != 1 {
                return (Some(format!("{base} {}", entry.label_ordinal)), false);
            } else {
                return (Some(base.to_owned()), false);
            }
        }
    }

    (None, false)
}

pub fn resolve_stream_label(table: &StreamLabelTable, name: &str) -> ProvenancePart {
    if table.status == LabelTableStatus::Failed {
        return ProvenancePart::Unknown;
    }
    table
        .rows
        .iter()
        .find(|row| row.name == name)
        .map(|row| row.part.clone())
        .unwrap_or(ProvenancePart::Unknown)
}

pub fn provenance_label(part: &ProvenancePart, parenthetical: Option<&str>) -> String {
    match part {
        ProvenancePart::Import { source } => {
            let words = match source.as_str() {
                "apple_health" => "Apple Health".to_owned(),
                "oura" => "Oura".to_owned(),
                "garmin" => "Garmin".to_owned(),
                "strava" => "Strava".to_owned(),
                "chatgpt" => "ChatGPT".to_owned(),
                "claude" => "Claude".to_owned(),
                "gemini" => "Gemini".to_owned(),
                "kindle" => "Kindle".to_owned(),
                "obsidian" => "notes".to_owned(),
                "evernote" => "Evernote".to_owned(),
                "granola" => "Granola".to_owned(),
                "email" => "Email".to_owned(),
                "ics" => "calendar".to_owned(),
                "document" => "documents".to_owned(),
                "image" => "images".to_owned(),
                "audio" => "audio".to_owned(),
                "journal_archive" => "another journal".to_owned(),
                _ => source
                    .split(['_', '-'])
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_lowercase())
                    .collect::<Vec<_>>()
                    .join(" "),
            };
            format!("{words} · imported")
        }
        ProvenancePart::Photos => "photos".to_owned(),
        ProvenancePart::Agent => "your agent".to_owned(),
        ProvenancePart::Bound {
            device: Some(device),
            what,
            ..
        } => {
            let mut label = format!("your {}", device.as_str());
            if let Some(paren) = parenthetical {
                label.push_str(&format!(" ({paren})"));
            }
            if let Some(what) = what {
                label.push_str(&format!(" · {}", what.as_str()));
            }
            label
        }
        ProvenancePart::Bound {
            device: None, what, ..
        } => {
            let mut label = "your device".to_owned();
            if let Some(paren) = parenthetical {
                label.push_str(&format!(" ({paren})"));
            }
            if let Some(what) = what {
                label.push_str(&format!(" · {}", what.as_str()));
            }
            label
        }
        ProvenancePart::Unbound { device, .. } => {
            let mut label = format!("your {}", device.as_str());
            if let Some(paren) = parenthetical {
                label.push_str(&format!(" ({paren})"));
            }
            label
        }
        ProvenancePart::Unknown => "source unknown".to_owned(),
    }
}

/// What tells two agents apart for the owner: the start of the key its stream
/// is named for. An agent's own name is self-chosen, so it can't do this job.
fn agent_short_key(stream: &str) -> Option<&str> {
    let key = stream.strip_prefix("agent-memory-")?;
    let short = key.get(..4)?;
    short
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit())
        .then_some(short)
}

/// Render one stream row's provenance label.
///
/// When a bound stream has no recognized device word (`device: None`) and the
/// owner explicitly set `owner_label`, surface the owner's chosen label directly
/// instead of wrapping it under generic `"your device"`.
fn render_row_label(row: &StreamLabelRow, parenthetical: Option<&str>) -> String {
    if row.owner_named
        && let ProvenancePart::Bound {
            device: None, what, ..
        } = &row.part
        && let Some(owner_label) = parenthetical
    {
        let mut label = owner_label.to_owned();
        if let Some(what) = what {
            label.push_str(&format!(" · {}", what.as_str()));
        }
        return label;
    }
    provenance_label(&row.part, parenthetical)
}

pub fn rendered_stream_labels(table: &StreamLabelTable) -> Option<BTreeMap<String, String>> {
    if table.status == LabelTableStatus::Failed {
        return None;
    }

    let mut result = BTreeMap::new();
    for row in &table.rows {
        let parenthetical = compute_parenthetical(row, &table.rows);
        let mut rendered = render_row_label(row, parenthetical);
        if row.part == ProvenancePart::Agent
            && let Some(key) = agent_short_key(&row.name)
        {
            rendered.push_str(" · ");
            rendered.push_str(key);
        }
        result.insert(row.name.clone(), rendered);
    }

    Some(result)
}

/// Agent-facing provenance label for one stream.
///
/// The second element is `true` when multiple streams intentionally share this
/// source word (for example `import.text` and `import.image`, or imported and
/// bound Obsidian streams) and must not be disambiguated with `(1)`/`(2)`.
fn agent_provenance_label(row: &StreamLabelRow, parenthetical: Option<&str>) -> (String, bool) {
    match &row.part {
        ProvenancePart::Import { source } => match source.as_str() {
            "text" | "image" => ("imported file".to_owned(), true),
            "obsidian" => ("Obsidian".to_owned(), true),
            "evernote" => ("Evernote".to_owned(), true),
            "document" => ("PDF".to_owned(), true),
            _ => (provenance_label(&row.part, parenthetical), false),
        },
        ProvenancePart::Photos => ("photos".to_owned(), true),
        ProvenancePart::Bound {
            what: Some(WhatWord::Notes),
            ..
        } => ("Obsidian".to_owned(), true),
        _ => (render_row_label(row, parenthetical), false),
    }
}

/// Labels for an agent: the owner's words, free of any stream name.
///
/// Unified import sources (`imported file`, `Obsidian`, `Evernote`, `PDF`)
/// share one source label across their backing streams so narrowing by source
/// matches all of them without synthetic numbering. Explicit owner labels on
/// generic (`device: None`) streams surface directly so distinct streams never
/// collapse into `"your device (1)/(2)"`. Other streams that would still read
/// alike are numbered in the order they were created (`"your mac · audio (2)"`).
/// An auto-reported or unbound client name that contains a stream name is
/// dropped. A failed table yields no labels.
pub fn agent_stream_labels(table: &StreamLabelTable) -> BTreeMap<String, String> {
    if table.status == LabelTableStatus::Failed {
        return BTreeMap::new();
    }
    let names: Vec<String> = table
        .rows
        .iter()
        .map(|row| row.name.to_lowercase())
        .collect();
    let mut rows: Vec<(&StreamLabelRow, String, bool)> = table
        .rows
        .iter()
        .map(|row| {
            let parenthetical = compute_parenthetical(row, &table.rows).filter(|identity| {
                // Explicit owner-set labels are the owner's own words and must
                // not be discarded merely because an English word happens to
                // match another stream stem; auto-reported/unbound names are
                // still filtered so host/stream names never leak to an agent.
                if row.owner_named {
                    return true;
                }
                let identity = identity.to_lowercase();
                !names.iter().any(|name| identity.contains(name.as_str()))
            });
            let (label, shared) = agent_provenance_label(row, parenthetical);
            (row, label, shared)
        })
        .collect();
    rows.sort_by(|(a, _, _), (b, _, _)| a.created_at.cmp(&b.created_at).then(a.name.cmp(&b.name)));

    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for (_, label, shared) in &rows {
        if !shared {
            *counts.entry(label.clone()).or_default() += 1;
        }
    }
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    rows.into_iter()
        .map(|(row, label, shared)| {
            let label = if !shared && counts.get(&label).copied().unwrap_or(0) > 1 {
                let ordinal = seen.entry(label.clone()).or_default();
                *ordinal += 1;
                format!("{label} ({ordinal})")
            } else {
                label
            };
            (row.name.clone(), label)
        })
        .collect()
}

/// What a label says, as parts a page can compare without reading its words:
/// the what word, and whether it names an import.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct StreamLabelSays {
    pub what: Option<&'static str>,
    pub imported: bool,
}

pub fn rendered_stream_says(table: &StreamLabelTable) -> Option<BTreeMap<String, StreamLabelSays>> {
    if table.status == LabelTableStatus::Failed {
        return None;
    }
    Some(
        table
            .rows
            .iter()
            .map(|row| {
                let says = match &row.part {
                    ProvenancePart::Import { .. } => StreamLabelSays {
                        what: None,
                        imported: true,
                    },
                    ProvenancePart::Photos => StreamLabelSays {
                        what: Some("photos"),
                        imported: false,
                    },
                    ProvenancePart::Bound { what, .. } => StreamLabelSays {
                        what: what.map(|what| what.as_str()),
                        imported: false,
                    },
                    _ => StreamLabelSays {
                        what: None,
                        imported: false,
                    },
                };
                (row.name.clone(), says)
            })
            .collect(),
    )
}

/// Guard against leaking FQDNs/hostnames (`.`) or filesystem paths (`/`, `\`).
fn is_safe_identity(identity: &str) -> bool {
    !identity.contains('.') && !identity.contains('/') && !identity.contains('\\')
}

fn compute_parenthetical<'a>(
    row: &'a StreamLabelRow,
    all_rows: &'a [StreamLabelRow],
) -> Option<&'a str> {
    let group = row.part.device_group()?;
    let identity = row.part.identity()?;

    if !is_safe_identity(identity) {
        return None;
    }

    // When a bound stream has no recognized device word and the owner explicitly
    // named it, keep the owner's label even without a peer stream.
    if group == DeviceGroup::Generic && row.owner_named {
        return Some(identity);
    }

    let has_differentiating_peer = all_rows.iter().any(|other| {
        other.name != row.name
            && other.part.device_group() == Some(group)
            && other
                .part
                .identity()
                .is_some_and(|other_id| other_id != identity && is_safe_identity(other_id))
    });

    if has_differentiating_peer {
        Some(identity)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn per_note_granola_streams_keep_one_source_identity() {
        let record:solstone_core_segment::StreamRecord=serde_json::from_value(json!({"name":"any","kind":"import","host":null,"platform":null,"created_at":1,"last_day":null,"last_segment":null,"seq":1})).unwrap();
        for stem in [
            "import.granola.not_1d3tmYTlCICgjy",
            "import.granola.not_2d3tmYTlCICgjy",
        ] {
            assert_eq!(
                classify_record(stem, &record, &BTreeMap::new(), &BTreeMap::new()),
                ProvenancePart::Import {
                    source: "granola".into()
                }
            );
        }
    }

    fn make_journal() -> TempDir {
        TempDir::new().expect("temp dir creates")
    }

    fn write_file(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    fn write_stream(
        root: &Path,
        name: &str,
        kind: &str,
        cid: Option<&str>,
        source: Option<&str>,
        platform: Option<&str>,
    ) {
        let mut obj = json!({
            "name": name,
            "kind": kind,
            "host": null,
            "platform": platform,
            "created_at": 1,
            "last_day": null,
            "last_segment": null,
            "seq": 1
        });
        if let Some(cid) = cid {
            obj.as_object_mut()
                .unwrap()
                .insert("cid".into(), json!(cid));
        }
        if let Some(source) = source {
            obj.as_object_mut()
                .unwrap()
                .insert("source".into(), json!(source));
        }
        write_file(root, &format!("streams/{name}.json"), &obj.to_string());
    }

    #[test]
    fn missing_streams_directory_is_complete_and_empty() {
        let root = make_journal();
        let table = load_stream_labels(root.path());
        assert_eq!(table.status, LabelTableStatus::Complete);
        assert!(table.rows.is_empty());
    }

    #[test]
    fn one_malformed_record_is_failed_and_empty() {
        let root = make_journal();
        write_file(root.path(), "streams/broken.json", "{not json");
        let table = load_stream_labels(root.path());
        assert_eq!(table.status, LabelTableStatus::Failed);
        assert!(table.rows.is_empty());
        assert_eq!(rendered_stream_labels(&table), None);
    }

    #[test]
    fn description_file_corrupt_is_partial_with_records_classified() {
        let root = make_journal();
        write_stream(
            root.path(),
            "s1",
            "observer",
            Some("sha256:1111"),
            Some("watch-audio"),
            None,
        );
        write_file(root.path(), "link/client-descriptions.json", "{");
        let table = load_stream_labels(root.path());
        assert_eq!(table.status, LabelTableStatus::Partial);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(
            table.rows[0].part,
            ProvenancePart::Bound {
                device: Some(DeviceWord::Watch),
                what: Some(WhatWord::Audio),
                identity: None,
            }
        );
    }

    #[test]
    fn missing_descriptions_and_missing_ledger_is_complete() {
        let root = make_journal();
        write_stream(
            root.path(),
            "s1",
            "observer",
            Some("sha256:1111"),
            Some("tmux"),
            None,
        );
        let table = load_stream_labels(root.path());
        assert_eq!(table.status, LabelTableStatus::Complete);
        assert_eq!(table.rows.len(), 1);
    }

    #[test]
    fn accessory_source_is_bound_with_no_device_or_what_word() {
        let root = make_journal();
        write_stream(
            root.path(),
            "omi",
            "observer",
            Some("sha256:1111"),
            Some("omi-audio"),
            None,
        );
        let table = load_stream_labels(root.path());
        assert_eq!(table.status, LabelTableStatus::Complete);
        assert_eq!(
            table.rows[0].part,
            ProvenancePart::Bound {
                device: None,
                what: None,
                identity: None,
            }
        );
    }

    #[test]
    fn dot_free_fedora_with_no_platform_is_unknown() {
        let root = make_journal();
        write_stream(root.path(), "fedora", "observer", None, None, None);
        let table = load_stream_labels(root.path());
        assert_eq!(table.status, LabelTableStatus::Complete);
        assert_eq!(table.rows[0].part, ProvenancePart::Unknown);
    }

    #[test]
    fn rokid_glasses_is_unknown() {
        let root = make_journal();
        write_stream(
            root.path(),
            "rokid-rg-glasses.glasses",
            "observer",
            None,
            None,
            None,
        );
        let table = load_stream_labels(root.path());
        assert_eq!(table.status, LabelTableStatus::Complete);
        assert_eq!(table.rows[0].part, ProvenancePart::Unknown);
    }

    #[test]
    fn fedora_tmux_is_unbound_terminal_identity_fedora() {
        let root = make_journal();
        write_stream(root.path(), "fedora.tmux", "observer", None, None, None);
        let table = load_stream_labels(root.path());
        assert_eq!(table.status, LabelTableStatus::Complete);
        assert_eq!(
            table.rows[0].part,
            ProvenancePart::Unbound {
                device: DeviceWord::Terminal,
                identity: Some("fedora".to_owned()),
            }
        );
    }

    #[test]
    fn mobile_segment_with_no_platform_and_no_type_has_no_device_word() {
        let root = make_journal();
        write_stream(
            root.path(),
            "s1",
            "observer",
            Some("sha256:1111"),
            Some("mobile-segment"),
            None,
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            table.rows[0].part,
            ProvenancePart::Bound {
                device: None,
                what: None,
                identity: None,
            }
        );
    }

    #[test]
    fn linux_reported_desktop_screen_is_computer_screen_fedora() {
        let root = make_journal();
        write_stream(
            root.path(),
            "device_2",
            "observer",
            Some("sha256:1111"),
            Some("screen"),
            None,
        );
        let desc = json!({
            "sha256:1111": {
                "protocol_version": 1, "revision": 1, "owner_label": null, "updated_at": null,
                "reported": {
                    "name": "fedora", "platform": "linux", "device_type": "desktop",
                    "app_id": null, "app_version": null
                }
            }
        });
        write_file(
            root.path(),
            "link/client-descriptions.json",
            &desc.to_string(),
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            table.rows[0].part,
            ProvenancePart::Bound {
                device: Some(DeviceWord::Computer),
                what: Some(WhatWord::Screen),
                identity: Some("fedora".to_owned()),
            }
        );
    }

    #[test]
    fn ios_and_android_permutations() {
        let root = make_journal();
        let make_desc = |_cid: &str, plat: &str, dtype: Option<&str>| {
            json!({
                "protocol_version": 1, "revision": 1, "owner_label": null, "updated_at": null,
                "reported": {
                    "name": "dev", "platform": plat, "device_type": dtype,
                    "app_id": null, "app_version": null
                }
            })
        };
        let desc = json!({
            "sha256:i1": make_desc("sha256:i1", "ios", Some("tablet")),
            "sha256:i2": make_desc("sha256:i2", "ios", Some("phone")),
            "sha256:i3": make_desc("sha256:i3", "ios", None),
            "sha256:i4": make_desc("sha256:i4", "ipados", Some("tablet")),
            "sha256:a1": make_desc("sha256:a1", "android", Some("tablet")),
            "sha256:a2": make_desc("sha256:a2", "android", Some("phone")),
            "sha256:a3": make_desc("sha256:a3", "android", None),
        });
        write_file(
            root.path(),
            "link/client-descriptions.json",
            &desc.to_string(),
        );
        write_stream(
            root.path(),
            "s_i1",
            "observer",
            Some("sha256:i1"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "s_i2",
            "observer",
            Some("sha256:i2"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "s_i3",
            "observer",
            Some("sha256:i3"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "s_i4",
            "observer",
            Some("sha256:i4"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "s_a1",
            "observer",
            Some("sha256:a1"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "s_a2",
            "observer",
            Some("sha256:a2"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "s_a3",
            "observer",
            Some("sha256:a3"),
            Some("screen"),
            None,
        );

        let table = load_stream_labels(root.path());
        let part_of = |name: &str| resolve_stream_label(&table, name);
        assert_eq!(part_of("s_i1").device_word(), Some(DeviceWord::Ipad));
        assert_eq!(part_of("s_i2").device_word(), Some(DeviceWord::Iphone));
        assert_eq!(part_of("s_i3").device_word(), None);
        assert_eq!(part_of("s_i4").device_word(), Some(DeviceWord::Ipad));
        assert_eq!(part_of("s_a1").device_word(), Some(DeviceWord::Tablet));
        assert_eq!(part_of("s_a2").device_word(), Some(DeviceWord::Phone));
        assert_eq!(part_of("s_a3").device_word(), None);
    }

    #[test]
    fn windows_and_linux_permutations() {
        let root = make_journal();
        let make_desc = |dtype: Option<&str>| {
            json!({
                "protocol_version": 1, "revision": 1, "owner_label": null, "updated_at": null,
                "reported": {
                    "name": "dev", "platform": "windows", "device_type": dtype,
                    "app_id": null, "app_version": null
                }
            })
        };
        let desc = json!({
            "sha256:w1": make_desc(Some("terminal")),
            "sha256:w2": make_desc(Some("phone")),
            "sha256:w3": make_desc(Some("tablet")),
            "sha256:w4": make_desc(Some("desktop")),
            "sha256:w5": make_desc(None),
        });
        write_file(
            root.path(),
            "link/client-descriptions.json",
            &desc.to_string(),
        );
        write_stream(
            root.path(),
            "s_w1",
            "observer",
            Some("sha256:w1"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "s_w2",
            "observer",
            Some("sha256:w2"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "s_w3",
            "observer",
            Some("sha256:w3"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "s_w4",
            "observer",
            Some("sha256:w4"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "s_w5",
            "observer",
            Some("sha256:w5"),
            Some("screen"),
            None,
        );

        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "s_w1").device_word(),
            Some(DeviceWord::Terminal)
        );
        assert_eq!(
            resolve_stream_label(&table, "s_w2").device_word(),
            Some(DeviceWord::Phone)
        );
        assert_eq!(
            resolve_stream_label(&table, "s_w3").device_word(),
            Some(DeviceWord::Tablet)
        );
        assert_eq!(
            resolve_stream_label(&table, "s_w4").device_word(),
            Some(DeviceWord::Computer)
        );
        assert_eq!(
            resolve_stream_label(&table, "s_w5").device_word(),
            Some(DeviceWord::Computer)
        );
    }

    #[test]
    fn unlisted_platform_blocks_later_facts() {
        let root = make_journal();
        let desc = json!({
            "sha256:1111": {
                "protocol_version": 1, "revision": 1, "owner_label": null, "updated_at": null,
                "reported": {
                    "name": "dev", "platform": "visionos", "device_type": null,
                    "app_id": null, "app_version": null
                }
            }
        });
        write_file(
            root.path(),
            "link/client-descriptions.json",
            &desc.to_string(),
        );
        let ledger = json!([{
            "fingerprint": "sha256:1111",
            "device_label": "Dev",
            "paired_at": "2026-01-01T00:00:00Z",
            "instance_id": "inst",
            "role": "observer",
            "network": null,
            "client_label": "dev",
            "label_ordinal": 1,
            "kind": "cert",
            "platform": "ios"
        }]);
        write_file(
            root.path(),
            "link/authorized_clients.json",
            &ledger.to_string(),
        );
        write_stream(
            root.path(),
            "s1",
            "observer",
            Some("sha256:1111"),
            Some("screen"),
            None,
        );

        let table = load_stream_labels(root.path());
        assert_eq!(table.rows[0].part.device_word(), None);
    }

    #[test]
    fn allocation_platform_used_only_when_reported_and_ledger_missing() {
        let root = make_journal();
        let stream = json!({
            "name": "s1", "kind": "observer", "host": null, "platform": null,
            "created_at": 1, "last_day": null, "last_segment": null, "seq": 1,
            "cid": "sha256:1111", "source": "screen",
            "allocation": {
                "base": "platform",
                "base_input": "macos",
                "source": "screen"
            }
        });
        write_file(root.path(), "streams/s1.json", &stream.to_string());
        let table = load_stream_labels(root.path());
        assert_eq!(table.rows[0].part.device_word(), Some(DeviceWord::Mac));
    }

    #[test]
    fn two_computer_rows_parenthetical_resolution() {
        let root = make_journal();
        let desc = json!({
            "sha256:1": {
                "protocol_version": 1, "revision": 1, "owner_label": null, "updated_at": null,
                "reported": { "name": "fedora", "platform": "linux", "device_type": "desktop", "app_id": null, "app_version": null }
            },
            "sha256:2": {
                "protocol_version": 1, "revision": 1, "owner_label": null, "updated_at": null,
                "reported": { "name": "studio", "platform": "linux", "device_type": "desktop", "app_id": null, "app_version": null }
            }
        });
        write_file(
            root.path(),
            "link/client-descriptions.json",
            &desc.to_string(),
        );
        write_stream(
            root.path(),
            "c1",
            "observer",
            Some("sha256:1"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "c2",
            "observer",
            Some("sha256:2"),
            Some("screen"),
            None,
        );

        let table = load_stream_labels(root.path());
        assert_eq!(
            compute_parenthetical(&table.rows[0], &table.rows),
            Some("fedora")
        );
        assert_eq!(
            compute_parenthetical(&table.rows[1], &table.rows),
            Some("studio")
        );

        // If same identity: no parentheses
        let desc_same = json!({
            "sha256:1": {
                "protocol_version": 1, "revision": 1, "owner_label": null, "updated_at": null,
                "reported": { "name": "same", "platform": "linux", "device_type": "desktop", "app_id": null, "app_version": null }
            },
            "sha256:2": {
                "protocol_version": 1, "revision": 1, "owner_label": null, "updated_at": null,
                "reported": { "name": "same", "platform": "linux", "device_type": "desktop", "app_id": null, "app_version": null }
            }
        });
        write_file(
            root.path(),
            "link/client-descriptions.json",
            &desc_same.to_string(),
        );
        let table_same = load_stream_labels(root.path());
        assert_eq!(
            compute_parenthetical(&table_same.rows[0], &table_same.rows),
            None
        );
        assert_eq!(
            compute_parenthetical(&table_same.rows[1], &table_same.rows),
            None
        );
    }

    #[test]
    fn owner_label_shows_an_agents_short_key_and_an_agents_own_label_does_not() {
        let row = |name: &str| StreamLabelRow {
            name: name.to_owned(),
            part: ProvenancePart::Agent,
            created_at: 1,
            owner_named: false,
        };
        let table = StreamLabelTable {
            status: LabelTableStatus::Complete,
            rows: vec![row("agent-memory-a3f29c41"), row("agent-memory-0d7b1111")],
        };
        let owner = rendered_stream_labels(&table).unwrap();
        assert_ne!(
            owner["agent-memory-a3f29c41"],
            owner["agent-memory-0d7b1111"]
        );
        assert!(owner["agent-memory-a3f29c41"].contains("a3f2"));
        let agent = agent_stream_labels(&table);
        assert!(!agent["agent-memory-a3f29c41"].contains("a3f2"));
    }

    #[test]
    fn agent_memory_with_cid_and_source_is_agent() {
        let root = make_journal();
        write_stream(
            root.path(),
            "agent-memory-1234",
            "agent_memory",
            Some("sha256:1234"),
            Some("1234"),
            None,
        );
        let table = load_stream_labels(root.path());
        assert_eq!(table.rows[0].part, ProvenancePart::Agent);
    }

    #[test]
    fn a_previously_imported_note_keeps_its_notes_label() {
        let part = ProvenancePart::Import {
            source: "obsidian".to_owned(),
        };
        assert_eq!(provenance_label(&part, None), "notes · imported");
        let evernote = ProvenancePart::Import {
            source: "evernote".to_owned(),
        };
        assert_eq!(provenance_label(&evernote, None), "Evernote · imported");
    }

    #[test]
    fn a_note_from_the_app_names_obsidian_as_its_source() {
        let part = bound(None, Some(WhatWord::Notes), None);
        assert_eq!(
            provenance_label(&part, None),
            "your device · Obsidian notes"
        );
    }

    #[test]
    fn import_stream_is_import_part() {
        let root = make_journal();
        write_stream(root.path(), "import.oura", "import", None, None, None);
        let table = load_stream_labels(root.path());
        assert_eq!(
            table.rows[0].part,
            ProvenancePart::Import {
                source: "oura".to_owned()
            }
        );
    }

    #[test]
    fn bound_cid_with_no_description_ledger_or_allocation_is_device_none() {
        let root = make_journal();
        write_stream(
            root.path(),
            "bound_orphaned",
            "observer",
            Some("sha256:deadbeef"),
            Some("audio"),
            None,
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            table.rows[0].part,
            ProvenancePart::Bound {
                device: None,
                what: Some(WhatWord::Audio),
                identity: None,
            }
        );
    }

    #[test]
    fn resolve_unregistered_name_on_complete_table_is_unknown() {
        let root = make_journal();
        write_stream(root.path(), "registered", "observer", None, None, None);
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "not_registered"),
            ProvenancePart::Unknown
        );
    }

    #[test]
    fn multi_dot_stream_name_is_unknown() {
        let root = make_journal();
        write_stream(
            root.path(),
            "device.watch.audio",
            "observer",
            None,
            None,
            None,
        );
        let table = load_stream_labels(root.path());
        assert_eq!(table.rows[0].part, ProvenancePart::Unknown);
    }

    type DescribeEntry<'a> = (&'a str, Option<&'a str>, Option<&'a str>, Option<&'a str>);

    fn describe(root: &Path, entries: &[DescribeEntry<'_>]) {
        let mut map = serde_json::Map::new();
        for (cid, name, platform, device_type) in entries {
            map.insert(
                (*cid).to_owned(),
                json!({
                    "protocol_version": 1, "revision": 1, "owner_label": null, "updated_at": null,
                    "reported": {
                        "name": name, "platform": platform, "device_type": device_type,
                        "app_id": null, "app_version": null
                    }
                }),
            );
        }
        write_file(
            root,
            "link/client-descriptions.json",
            &serde_json::Value::Object(map).to_string(),
        );
    }

    fn pair(root: &Path, entries: &[(&str, &str, u32, Option<&str>)]) {
        let rows: Vec<_> = entries
            .iter()
            .map(|(cid, label, ordinal, platform)| {
                json!({
                    "fingerprint": cid, "device_label": label, "paired_at": "2026-01-01T00:00:00Z",
                    "instance_id": format!("inst-{cid}"), "role": "observer", "network": null,
                    "client_label": label, "label_ordinal": ordinal, "kind": "cert",
                    "platform": platform
                })
            })
            .collect();
        write_file(
            root,
            "link/authorized_clients.json",
            &serde_json::Value::Array(rows).to_string(),
        );
    }

    fn write_allocated(root: &Path, name: &str, cid: &str, source: &str, base: &str, input: &str) {
        let stream = json!({
            "name": name, "kind": "observer", "host": null, "platform": null,
            "created_at": 1, "last_day": null, "last_segment": null, "seq": 1,
            "cid": cid, "source": source,
            "allocation": { "base": base, "base_input": input, "source": source }
        });
        write_file(root, &format!("streams/{name}.json"), &stream.to_string());
    }

    fn bound(
        device: Option<DeviceWord>,
        what: Option<WhatWord>,
        identity: Option<&str>,
    ) -> ProvenancePart {
        ProvenancePart::Bound {
            device,
            what,
            identity: identity.map(str::to_owned),
        }
    }

    #[test]
    fn empty_source_is_the_devices_own_recording_with_no_what_word() {
        let root = make_journal();
        write_stream(
            root.path(),
            "device_2",
            "observer",
            Some("sha256:1"),
            Some(""),
            None,
        );
        describe(
            root.path(),
            &[("sha256:1", Some("fedora"), Some("linux"), Some("desktop"))],
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "device_2"),
            bound(Some(DeviceWord::Computer), None, Some("fedora"))
        );
    }

    #[test]
    fn a_calendar_source_reads_as_the_device_s_calendar() {
        let root = make_journal();
        write_stream(
            root.path(),
            "phone_calendar",
            "observer",
            Some("sha256:1"),
            Some("calendar"),
            None,
        );
        write_stream(
            root.path(),
            "phone_location",
            "observer",
            Some("sha256:1"),
            Some("location"),
            None,
        );
        describe(
            root.path(),
            &[("sha256:1", Some("pocket"), Some("ios"), Some("phone"))],
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "phone_calendar"),
            bound(
                Some(DeviceWord::Iphone),
                Some(WhatWord::Calendar),
                Some("pocket")
            )
        );
        assert_eq!(
            resolve_stream_label(&table, "phone_location"),
            bound(
                Some(DeviceWord::Iphone),
                Some(WhatWord::Location),
                Some("pocket")
            )
        );
        let says = rendered_stream_says(&table).unwrap();
        assert_eq!(says["phone_calendar"].what, Some("calendar"));
        assert_eq!(says["phone_location"].what, Some("location"));
    }

    #[test]
    fn ledger_platform_hint_answers_when_nothing_is_reported() {
        let root = make_journal();
        write_stream(
            root.path(),
            "s1",
            "observer",
            Some("sha256:1"),
            Some("screen"),
            None,
        );
        pair(root.path(), &[("sha256:1", "studio", 1, Some("macos"))]);
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "s1"),
            bound(
                Some(DeviceWord::Mac),
                Some(WhatWord::Screen),
                Some("studio")
            )
        );
    }

    #[test]
    fn unpaired_allocation_platform_gives_mac_but_never_a_phone_word() {
        let root = make_journal();
        write_allocated(
            root.path(),
            "macos_screen",
            "sha256:m",
            "screen",
            "platform",
            "macos",
        );
        write_allocated(
            root.path(),
            "ios_audio",
            "sha256:i",
            "audio",
            "platform",
            "ios",
        );
        write_allocated(
            root.path(),
            "android_audio",
            "sha256:a",
            "audio",
            "platform",
            "android",
        );
        write_allocated(
            root.path(),
            "sams_macbook_screen",
            "sha256:s",
            "screen",
            "client_label",
            "Sam's MacBook",
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "macos_screen").device_word(),
            Some(DeviceWord::Mac)
        );
        assert_eq!(
            resolve_stream_label(&table, "ios_audio"),
            bound(None, Some(WhatWord::Audio), None)
        );
        assert_eq!(
            resolve_stream_label(&table, "android_audio"),
            bound(None, Some(WhatWord::Audio), None)
        );
        assert_eq!(
            resolve_stream_label(&table, "sams_macbook_screen"),
            bound(None, Some(WhatWord::Screen), None)
        );
    }

    #[test]
    fn bound_record_platform_alone_is_not_a_bound_fact() {
        let root = make_journal();
        write_stream(
            root.path(),
            "s1",
            "observer",
            Some("sha256:1"),
            Some("screen"),
            Some("macos"),
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "s1"),
            bound(None, Some(WhatWord::Screen), None)
        );
    }

    #[test]
    fn platform_and_device_type_are_read_case_insensitively() {
        let root = make_journal();
        write_stream(
            root.path(),
            "s1",
            "observer",
            Some("sha256:1"),
            Some("audio"),
            None,
        );
        describe(
            root.path(),
            &[("sha256:1", None, Some(" iOS "), Some("Phone"))],
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "s1").device_word(),
            Some(DeviceWord::Iphone)
        );
    }

    #[test]
    fn unlisted_platform_with_a_listed_device_type_uses_the_device_type() {
        let root = make_journal();
        write_stream(
            root.path(),
            "s1",
            "observer",
            Some("sha256:1"),
            Some("screen"),
            None,
        );
        describe(
            root.path(),
            &[("sha256:1", None, Some("visionos"), Some("phone"))],
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "s1").device_word(),
            Some(DeviceWord::Phone)
        );
    }

    #[test]
    fn linux_reporting_terminal_is_terminal() {
        let root = make_journal();
        write_stream(
            root.path(),
            "s1",
            "observer",
            Some("sha256:1"),
            Some("screen"),
            None,
        );
        describe(
            root.path(),
            &[("sha256:1", None, Some("linux"), Some("terminal"))],
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "s1").device_word(),
            Some(DeviceWord::Terminal)
        );
    }

    #[test]
    fn accessories_relayed_by_a_phone_are_not_the_phone() {
        let root = make_journal();
        describe(
            root.path(),
            &[
                ("sha256:i", None, Some("ios"), Some("phone")),
                ("sha256:a", None, Some("android"), Some("phone")),
            ],
        );
        write_stream(
            root.path(),
            "omi",
            "observer",
            Some("sha256:i"),
            Some("omi-audio"),
            None,
        );
        write_stream(
            root.path(),
            "watch",
            "observer",
            Some("sha256:a"),
            Some("watch"),
            None,
        );
        write_stream(
            root.path(),
            "glasses",
            "observer",
            Some("sha256:a"),
            Some("glasses"),
            None,
        );
        let table = load_stream_labels(root.path());
        for name in ["omi", "watch", "glasses"] {
            assert_eq!(
                resolve_stream_label(&table, name),
                bound(None, None, None),
                "{name}"
            );
        }
    }

    #[test]
    fn mobile_segment_never_names_the_phone() {
        let root = make_journal();
        describe(root.path(), &[("sha256:i", None, Some("ios"), None)]);
        write_stream(
            root.path(),
            "read",
            "observer",
            Some("sha256:i"),
            Some("mobile-segment"),
            None,
        );
        write_stream(
            root.path(),
            "unread",
            "observer",
            Some("sha256:x"),
            Some("mobile-segment"),
            None,
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "read"),
            resolve_stream_label(&table, "unread")
        );
        assert_eq!(resolve_stream_label(&table, "read").device_word(), None);
    }

    #[test]
    fn two_macs_named_only_by_the_ledger_carry_their_ordinals() {
        let root = make_journal();
        pair(
            root.path(),
            &[
                ("sha256:1", "macbook", 1, Some("macos")),
                ("sha256:2", "macbook", 2, Some("macos")),
            ],
        );
        write_stream(
            root.path(),
            "a",
            "observer",
            Some("sha256:1"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "b",
            "observer",
            Some("sha256:2"),
            Some("screen"),
            None,
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "a").identity(),
            Some("macbook")
        );
        assert_eq!(
            resolve_stream_label(&table, "b").identity(),
            Some("macbook 2")
        );
        let rows = &table.rows;
        assert_eq!(compute_parenthetical(&rows[0], rows), Some("macbook"));
        assert_eq!(compute_parenthetical(&rows[1], rows), Some("macbook 2"));
    }

    #[test]
    fn two_macs_with_no_names_get_no_parentheses() {
        let root = make_journal();
        pair(
            root.path(),
            &[
                ("sha256:1", "", 1, Some("macos")),
                ("sha256:2", "", 1, Some("macos")),
            ],
        );
        write_stream(
            root.path(),
            "a",
            "observer",
            Some("sha256:1"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "b",
            "observer",
            Some("sha256:2"),
            Some("screen"),
            None,
        );
        let table = load_stream_labels(root.path());
        assert!(
            table
                .rows
                .iter()
                .all(|row| row.part.device_word() == Some(DeviceWord::Mac))
        );
        assert!(
            table
                .rows
                .iter()
                .all(|row| compute_parenthetical(row, &table.rows).is_none())
        );
    }

    #[test]
    fn legacy_unbound_name_beside_a_bound_stream_with_nothing_read() {
        let root = make_journal();
        write_stream(root.path(), "fedora", "observer", None, None, None);
        write_stream(
            root.path(),
            "macos_screen",
            "observer",
            Some("sha256:1"),
            Some("screen"),
            None,
        );
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "fedora"),
            ProvenancePart::Unknown
        );
        assert_eq!(
            resolve_stream_label(&table, "macos_screen"),
            bound(None, Some(WhatWord::Screen), None)
        );
    }

    #[test]
    fn unreadable_streams_directory_fails_the_table() {
        let root = make_journal();
        write_file(root.path(), "streams", "not a directory");
        let table = load_stream_labels(root.path());
        assert_eq!(table.status, LabelTableStatus::Failed);
        assert_eq!(rendered_stream_labels(&table), None);
    }

    #[test]
    fn readcompat_shapes() {
        let root = make_journal();
        // cid with no source, platform ios: an older phone record.
        write_stream(
            root.path(),
            "iphone",
            "observer",
            Some("sha256:1"),
            None,
            Some("ios"),
        );
        // no cid, platform linux: an older desktop record.
        write_stream(
            root.path(),
            "workstation",
            "observer",
            None,
            None,
            Some("linux"),
        );
        write_stream(root.path(), "pixel-7", "observer", None, None, None);
        write_stream(root.path(), "recovered", "unknown", None, None, None);
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "iphone"),
            ProvenancePart::Unknown
        );
        assert_eq!(
            resolve_stream_label(&table, "workstation"),
            ProvenancePart::Unbound {
                device: DeviceWord::Computer,
                identity: Some("workstation".to_owned()),
            }
        );
        assert_eq!(
            resolve_stream_label(&table, "pixel-7"),
            ProvenancePart::Unknown
        );
        assert_eq!(
            resolve_stream_label(&table, "recovered"),
            ProvenancePart::Unknown
        );
    }

    #[test]
    fn legacy_qualifier_tokens_win_over_a_read_platform() {
        let root = make_journal();
        write_stream(
            root.path(),
            "fedora.tmux",
            "observer",
            None,
            None,
            Some("linux"),
        );
        write_stream(root.path(), "suze.tmux", "observer", None, None, None);
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "fedora.tmux").device_word(),
            Some(DeviceWord::Terminal)
        );
        assert_eq!(
            resolve_stream_label(&table, "fedora.tmux").identity(),
            Some("fedora")
        );
        assert_eq!(
            resolve_stream_label(&table, "suze.tmux").identity(),
            Some("suze")
        );
    }

    #[test]
    fn device_dotted_names_have_no_writer_and_read_unknown() {
        let root = make_journal();
        for name in [
            "device.omi.audio",
            "device.mac.audio",
            "device.pixel_watch.audio",
        ] {
            write_stream(root.path(), name, "observer", None, None, None);
        }
        let table = load_stream_labels(root.path());
        assert!(
            table
                .rows
                .iter()
                .all(|row| row.part == ProvenancePart::Unknown)
        );
    }

    #[test]
    fn unlisted_import_keeps_its_parsed_source() {
        let root = make_journal();
        write_stream(root.path(), "import.apple", "import", None, None, None);
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "import.apple"),
            ProvenancePart::Import {
                source: "apple".to_owned()
            }
        );
    }

    #[test]
    fn two_legacy_linux_computers_are_told_apart_by_name() {
        let root = make_journal();
        write_stream(root.path(), "fedora", "observer", None, None, Some("linux"));
        write_stream(root.path(), "suze", "observer", None, None, Some("linux"));
        let table = load_stream_labels(root.path());
        for row in &table.rows {
            assert_eq!(row.part.device_word(), Some(DeviceWord::Computer));
            assert_eq!(
                compute_parenthetical(row, &table.rows),
                Some(row.name.as_str())
            );
        }
    }

    #[test]
    fn empty_source_with_nothing_read_or_a_read_mac() {
        let root = make_journal();
        write_stream(
            root.path(),
            "unread",
            "observer",
            Some("sha256:1"),
            Some(""),
            None,
        );
        write_allocated(root.path(), "macos", "sha256:2", "", "platform", "macos");
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "unread"),
            bound(None, None, None)
        );
        assert_eq!(
            resolve_stream_label(&table, "macos"),
            bound(Some(DeviceWord::Mac), None, None)
        );
    }

    #[test]
    fn served_parts_say_the_what_word_and_imports() {
        let root = make_journal();
        write_stream(
            root.path(),
            "s1",
            "observer",
            Some("sha256:1"),
            Some("audio"),
            None,
        );
        write_stream(
            root.path(),
            "s2",
            "observer",
            Some("sha256:2"),
            Some("tmux"),
            None,
        );
        write_stream(root.path(), "import.oura", "import", None, None, None);
        let table = load_stream_labels(root.path());
        let says = rendered_stream_says(&table).unwrap();
        assert_eq!(
            says["s1"],
            StreamLabelSays {
                what: Some("audio"),
                imported: false
            }
        );
        assert_eq!(
            says["s2"],
            StreamLabelSays {
                what: None,
                imported: false
            }
        );
        assert_eq!(
            says["import.oura"],
            StreamLabelSays {
                what: None,
                imported: true
            }
        );
        write_file(root.path(), "streams/broken.json", "{");
        assert_eq!(rendered_stream_says(&load_stream_labels(root.path())), None);
    }

    #[test]
    fn a_whitespace_only_platform_is_absent_and_the_next_fact_answers() {
        let root = make_journal();
        write_stream(
            root.path(),
            "s1",
            "observer",
            Some("sha256:1"),
            Some("screen"),
            None,
        );
        describe(root.path(), &[("sha256:1", None, Some("   "), None)]);
        pair(root.path(), &[("sha256:1", "studio", 1, Some("macos"))]);
        let table = load_stream_labels(root.path());
        assert_eq!(
            resolve_stream_label(&table, "s1").device_word(),
            Some(DeviceWord::Mac)
        );
    }

    fn write_created(root: &Path, name: &str, created_at: u64, platform: Option<&str>) {
        let stream = json!({
            "name": name, "kind": "observer", "host": null, "platform": platform,
            "created_at": created_at, "last_day": null, "last_segment": null, "seq": 1
        });
        write_file(root, &format!("streams/{name}.json"), &stream.to_string());
    }

    #[test]
    fn agent_labels_number_alike_streams_by_creation_and_never_carry_a_stream_name() {
        let root = make_journal();
        // Older unbound streams name themselves by host, which the dashboard
        // shows in parentheses; an agent must not read it.
        write_created(root.path(), "zqboronstudio", 20, Some("macos"));
        write_created(root.path(), "zqmvecair", 10, Some("macos"));
        let table = load_stream_labels(root.path());

        let labels = agent_stream_labels(&table);
        assert_eq!(labels["zqmvecair"], "your mac (1)");
        assert_eq!(labels["zqboronstudio"], "your mac (2)");
        let dashboard = rendered_stream_labels(&table).unwrap();
        assert_eq!(dashboard["zqboronstudio"], "your mac (zqboronstudio)");
    }

    #[test]
    fn agent_labels_keep_a_client_name_that_is_not_a_stream_name() {
        let root = make_journal();
        describe(
            root.path(),
            &[
                ("sha256:1", Some("boron"), Some("macos"), None),
                ("sha256:2", Some("mvec"), Some("macos"), None),
            ],
        );
        write_stream(
            root.path(),
            "device",
            "observer",
            Some("sha256:1"),
            Some(""),
            None,
        );
        write_stream(
            root.path(),
            "macos",
            "observer",
            Some("sha256:2"),
            Some(""),
            None,
        );
        let labels = agent_stream_labels(&load_stream_labels(root.path()));
        assert_eq!(labels["device"], "your mac (boron)");
        assert_eq!(labels["macos"], "your mac (mvec)");
    }

    #[test]
    fn agent_labels_are_empty_when_the_table_failed() {
        let root = make_journal();
        write_file(root.path(), "streams/broken.json", "{not json");
        assert!(agent_stream_labels(&load_stream_labels(root.path())).is_empty());
    }

    #[test]
    fn agent_import_labels_match_sources_without_changing_owner_dashboard_labels() {
        let root = make_journal();
        for (idx, stream) in [
            "import.text",
            "import.image",
            "import.obsidian",
            "import.evernote",
            "import.document",
            "import.apple_health",
            "import.oura",
            "import.strava",
            "import.chatgpt",
            "import.claude",
            "import.gemini",
            "import.kindle",
            "import.granola",
            "import.ics",
            "import.audio",
            "import.journal_archive",
        ]
        .into_iter()
        .enumerate()
        {
            let obj = json!({
                "name": stream,
                "kind": "import",
                "host": null,
                "platform": null,
                "created_at": (idx as u64) + 1,
                "last_day": null,
                "last_segment": null,
                "seq": 1
            });
            write_file(
                root.path(),
                &format!("streams/{stream}.json"),
                &obj.to_string(),
            );
        }
        write_stream(
            root.path(),
            "mac_obsidian",
            "observer",
            Some("sha256:obs1"),
            Some("obsidian"),
            Some("macos"),
        );
        write_stream(
            root.path(),
            "phone_obsidian",
            "observer",
            Some("sha256:obs2"),
            Some("obsidian"),
            None,
        );
        let table = load_stream_labels(root.path());
        let agent = agent_stream_labels(&table);
        let dashboard = rendered_stream_labels(&table).unwrap();

        // Unified agent labels share their source word without synthetic numbering.
        assert_eq!(agent["import.text"], "imported file");
        assert_eq!(agent["import.image"], "imported file");
        assert_eq!(agent["import.obsidian"], "Obsidian");
        assert_eq!(agent["mac_obsidian"], "Obsidian");
        assert_eq!(agent["phone_obsidian"], "Obsidian");
        assert_eq!(agent["import.evernote"], "Evernote");
        assert_eq!(agent["import.document"], "PDF");

        // All other imports keep their existing labels untouched.
        assert_eq!(agent["import.apple_health"], "Apple Health · imported");
        assert_eq!(agent["import.oura"], "Oura · imported");
        assert_eq!(agent["import.strava"], "Strava · imported");
        assert_eq!(agent["import.chatgpt"], "ChatGPT · imported");
        assert_eq!(agent["import.claude"], "Claude · imported");
        assert_eq!(agent["import.gemini"], "Gemini · imported");
        assert_eq!(agent["import.kindle"], "Kindle · imported");
        assert_eq!(agent["import.granola"], "Granola · imported");
        assert_eq!(agent["import.ics"], "calendar · imported");
        assert_eq!(agent["import.audio"], "audio · imported");
        assert_eq!(
            agent["import.journal_archive"],
            "another journal · imported"
        );

        // Owner dashboard labels remain untouched.
        assert_eq!(dashboard["import.text"], "text · imported");
        assert_eq!(dashboard["import.image"], "images · imported");
        assert_eq!(dashboard["import.obsidian"], "notes · imported");
        assert_eq!(dashboard["mac_obsidian"], "your device · Obsidian notes");
        assert_eq!(dashboard["import.evernote"], "Evernote · imported");
        assert_eq!(dashboard["import.document"], "documents · imported");
    }

    #[test]
    fn photos_stream_resolves_to_photos_label_and_what_word() {
        let root = tempfile::tempdir().unwrap();
        let obj = json!({
            "name": "photos",
            "kind": "import",
            "host": null,
            "platform": null,
            "created_at": 1,
            "last_day": null,
            "last_segment": null,
            "seq": 1
        });
        write_file(root.path(), "streams/photos.json", &obj.to_string());
        write_stream(
            root.path(),
            "iphone_photos",
            "observer",
            Some("sha256:ph1"),
            Some("photos"),
            Some("ios"),
        );
        let table = load_stream_labels(root.path());
        let dashboard = rendered_stream_labels(&table).unwrap();
        let says = rendered_stream_says(&table).unwrap();
        let agent = agent_stream_labels(&table);
        assert_eq!(dashboard.get("photos").map(String::as_str), Some("photos"));
        assert_eq!(agent.get("photos").map(String::as_str), Some("photos"));
        assert_eq!(says["photos"].what, Some("photos"));
        assert!(!says["photos"].imported);
        assert_eq!(
            dashboard.get("iphone_photos").map(String::as_str),
            Some("your device · photos")
        );
        assert_eq!(says["iphone_photos"].what, Some("photos"));
    }

    type OwnerDescribeEntry<'a> = (
        &'a str,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
        Option<&'a str>,
    );

    fn describe_with_owner(root: &Path, entries: &[OwnerDescribeEntry<'_>]) {
        let mut map = serde_json::Map::new();
        for (cid, owner_label, reported_name, platform, device_type) in entries {
            let reported = if reported_name.is_some() || platform.is_some() || device_type.is_some()
            {
                json!({
                    "name": reported_name,
                    "platform": platform,
                    "device_type": device_type,
                    "app_id": null,
                    "app_version": null,
                })
            } else {
                serde_json::Value::Null
            };
            map.insert(
                (*cid).to_owned(),
                json!({
                    "protocol_version": 1,
                    "revision": 1,
                    "owner_label": owner_label,
                    "updated_at": null,
                    "reported": reported,
                }),
            );
        }
        write_file(
            root,
            "link/client-descriptions.json",
            &serde_json::Value::Object(map).to_string(),
        );
    }

    #[test]
    fn generic_bound_streams_with_owner_labels_keep_owner_words_without_numbering() {
        let root = make_journal();
        describe_with_owner(
            root.path(),
            &[
                ("sha256:t", Some("studio talk"), None, None, None),
                ("sha256:m", Some("field memos"), None, None, None),
                ("sha256:c", Some("desk calls"), None, None, None),
                ("sha256:d", Some("work desk"), None, None, None),
                ("sha256:o", Some("field notebook"), None, None, None),
            ],
        );
        write_stream(
            root.path(),
            "talk",
            "observer",
            Some("sha256:t"),
            Some("microphone"),
            None,
        );
        write_stream(
            root.path(),
            "memos",
            "observer",
            Some("sha256:m"),
            Some("voice_memo"),
            None,
        );
        write_stream(
            root.path(),
            "calls",
            "observer",
            Some("sha256:c"),
            Some("call"),
            None,
        );
        write_stream(
            root.path(),
            "desk",
            "observer",
            Some("sha256:d"),
            Some("screen"),
            None,
        );
        write_stream(
            root.path(),
            "obsidian",
            "observer",
            Some("sha256:o"),
            Some("obsidian"),
            None,
        );

        let table = load_stream_labels(root.path());
        let agent = agent_stream_labels(&table);
        let dashboard = rendered_stream_labels(&table).unwrap();

        // Distinct owner_labels on device:None streams surface cleanly instead
        // of collapsing to "your device (1)/(2)/(3)", even when an English word
        // inside an explicit owner_label overlaps a stream stem ("talk").
        assert_eq!(agent["talk"], "studio talk");
        assert_eq!(agent["memos"], "field memos");
        assert_eq!(agent["calls"], "desk calls");
        assert_eq!(agent["desk"], "work desk · screen");
        // Unified Obsidian agent label remains shared across bound/imported notes.
        assert_eq!(agent["obsidian"], "Obsidian");

        assert_eq!(dashboard["talk"], "studio talk");
        assert_eq!(dashboard["memos"], "field memos");
        assert_eq!(dashboard["calls"], "desk calls");
        assert_eq!(dashboard["desk"], "work desk · screen");
        assert_eq!(dashboard["obsidian"], "field notebook · Obsidian notes");
    }

    #[test]
    fn single_generic_bound_stream_with_owner_label_uses_owner_label() {
        let root = make_journal();
        describe_with_owner(
            root.path(),
            &[("sha256:1", Some("lapel recorder"), None, None, None)],
        );
        write_stream(
            root.path(),
            "accessory",
            "observer",
            Some("sha256:1"),
            Some("omi-audio"),
            None,
        );
        let table = load_stream_labels(root.path());
        assert_eq!(agent_stream_labels(&table)["accessory"], "lapel recorder");
        assert_eq!(
            rendered_stream_labels(&table).unwrap()["accessory"],
            "lapel recorder"
        );
    }

    #[test]
    fn generic_bound_streams_with_reported_names_disambiguate_and_guard_hostnames() {
        let root = make_journal();
        describe_with_owner(
            root.path(),
            &[
                ("sha256:1", None, Some("pendant"), None, None),
                ("sha256:2", None, Some("lapel"), None, None),
                ("sha256:3", None, Some("host.local"), None, None),
                ("sha256:4", Some("bad/path"), None, None, None),
                ("sha256:5", Some("bad.host"), None, None, None),
                ("sha256:6", None, Some("stream_six_box"), None, None),
            ],
        );
        write_stream(
            root.path(),
            "s1",
            "observer",
            Some("sha256:1"),
            Some("omi-audio"),
            None,
        );
        write_stream(
            root.path(),
            "s2",
            "observer",
            Some("sha256:2"),
            Some("pin-audio"),
            None,
        );
        write_stream(
            root.path(),
            "s3",
            "observer",
            Some("sha256:3"),
            Some("other"),
            None,
        );
        write_stream(
            root.path(),
            "s4",
            "observer",
            Some("sha256:4"),
            Some("other"),
            None,
        );
        write_stream(
            root.path(),
            "s5",
            "observer",
            Some("sha256:5"),
            Some("other"),
            None,
        );
        write_stream(
            root.path(),
            "stream_six",
            "observer",
            Some("sha256:6"),
            Some("other"),
            None,
        );

        let table = load_stream_labels(root.path());
        let agent = agent_stream_labels(&table);
        assert_eq!(agent["s1"], "your device (pendant)");
        assert_eq!(agent["s2"], "your device (lapel)");
        // Dotted hostnames, paths, and auto-reported names containing a stream
        // stem never leak to agents.
        assert!(!agent["s3"].contains("host.local"));
        assert!(!agent["s4"].contains("bad/path"));
        assert!(!agent["s5"].contains("bad.host"));
        assert!(!agent["stream_six"].contains("stream_six"));
    }

    #[test]
    fn duplicate_owner_labels_on_generic_streams_still_disambiguate() {
        let root = make_journal();
        describe_with_owner(
            root.path(),
            &[
                ("sha256:1", Some("studio mic"), None, None, None),
                ("sha256:2", Some("studio mic"), None, None, None),
            ],
        );
        write_stream(
            root.path(),
            "a",
            "observer",
            Some("sha256:1"),
            Some("mic"),
            None,
        );
        write_stream(
            root.path(),
            "b",
            "observer",
            Some("sha256:2"),
            Some("mic"),
            None,
        );
        let agent = agent_stream_labels(&load_stream_labels(root.path()));
        assert_eq!(agent["a"], "studio mic (1)");
        assert_eq!(agent["b"], "studio mic (2)");
    }
}
