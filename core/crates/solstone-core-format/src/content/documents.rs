// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::Value;

use super::{JsonObject, ProducedChunks, clean_value, recorded_chunk};

const ABSENT_TEXT: &str = "Not specified in this document";

pub(super) fn render(records: &[JsonObject]) -> ProducedChunks {
    let chunks = records
        .first()
        .and_then(|document| {
            let content = render_document(document);
            (!content.is_empty()).then(|| recorded_chunk(content, 0, document))
        })
        .into_iter()
        .collect();

    ProducedChunks {
        chunks,
        agent_override: Some("documents".to_string()),
        header: None,
        error: None,
        warnings: Vec::new(),
    }
}

fn render_document(document: &JsonObject) -> String {
    let mut lines = Vec::new();
    append_section(
        &mut lines,
        "Overview",
        clean_value(document.get("overview")),
    );
    append_section(&mut lines, "Parties and Roles", parties(document));
    append_section(&mut lines, "Key Provisions", key_provisions(document));
    append_section(&mut lines, "Assets and Property", assets(document));
    append_section(&mut lines, "Conditions and Triggers", conditions(document));
    append_section(&mut lines, "Important Dates", important_dates(document));
    append_section(&mut lines, "Summary", clean_value(document.get("summary")));
    lines.join("\n").trim().to_string()
}

fn append_section(lines: &mut Vec<String>, heading: &str, body: String) {
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(format!("## {heading}"));
    lines.push(String::new());
    if body.is_empty() {
        lines.push(ABSENT_TEXT.to_string());
    } else {
        lines.push(body);
    }
}

fn append_item_detail(lines: &mut Vec<String>, label: &str, value: &str) {
    if !value.is_empty() {
        lines.push(format!("  {label}: {value}"));
    }
}

fn parties(document: &JsonObject) -> String {
    let mut lines = Vec::new();
    let Some(Value::Array(parties)) = document.get("parties") else {
        return String::new();
    };

    for party in parties {
        let Value::Object(party) = party else {
            continue;
        };
        let name = clean_value(party.get("name"));
        let role = clean_value(party.get("role"));
        let formal_term = clean_value(party.get("formal_term"));
        let tier = clean_value(party.get("appointment_tier"));
        let context_text = clean_value(party.get("context"));
        if [
            name.as_str(),
            role.as_str(),
            formal_term.as_str(),
            tier.as_str(),
            context_text.as_str(),
        ]
        .iter()
        .all(|part| part.is_empty())
        {
            continue;
        }

        let mut label = if name.is_empty() {
            "Unnamed party".to_string()
        } else {
            name
        };
        if !role.is_empty() {
            label.push_str(&format!(" - {role}"));
        }
        if !formal_term.is_empty() {
            label.push_str(&format!(" ({formal_term})"));
        }
        if !tier.is_empty() && tier != "not_applicable" {
            label.push_str(&format!(" [{tier}]"));
        }
        lines.push(format!("- {label}"));
        if !context_text.is_empty() {
            lines.push(format!("  {context_text}"));
        }
    }
    lines.join("\n")
}

fn key_provisions(document: &JsonObject) -> String {
    let mut lines = Vec::new();
    let Some(Value::Array(provisions)) = document.get("key_provisions") else {
        return String::new();
    };

    for provision in provisions {
        let Value::Object(provision) = provision else {
            continue;
        };
        let provision_type = clean_value(provision.get("type"));
        let text = clean_value(provision.get("text"));
        let applies_to = clean_value(provision.get("applies_to"));
        if [provision_type.as_str(), text.as_str(), applies_to.as_str()]
            .iter()
            .all(|part| part.is_empty())
        {
            continue;
        }

        let prefix = if provision_type.is_empty() {
            String::new()
        } else {
            format!("**{provision_type}:** ")
        };
        lines.push(format!("- {prefix}{text}").trim_end().to_string());
        append_item_detail(&mut lines, "Applies to", &applies_to);
    }
    lines.join("\n")
}

fn assets(document: &JsonObject) -> String {
    let mut lines = Vec::new();
    let Some(Value::Array(assets)) = document.get("assets") else {
        return String::new();
    };

    for asset in assets {
        let Value::Object(asset) = asset else {
            continue;
        };
        let name = clean_value(asset.get("name"));
        let asset_type = clean_value(asset.get("asset_type"));
        let disposition = clean_value(asset.get("disposition"));
        if [name.as_str(), asset_type.as_str(), disposition.as_str()]
            .iter()
            .all(|part| part.is_empty())
        {
            continue;
        }

        let mut label = if name.is_empty() {
            "Unnamed asset".to_string()
        } else {
            format!("**{name}**")
        };
        if !asset_type.is_empty() && asset_type != "unspecified" {
            label.push_str(&format!(" ({asset_type})"));
        }
        if !disposition.is_empty() {
            label.push_str(&format!(" - {disposition}"));
        }
        lines.push(format!("- {label}"));
    }
    lines.join("\n")
}

fn conditions(document: &JsonObject) -> String {
    let mut lines = Vec::new();
    let Some(Value::Array(conditions)) = document.get("conditions") else {
        return String::new();
    };

    for condition in conditions {
        let Value::Object(condition) = condition else {
            continue;
        };
        let trigger = clean_value(condition.get("trigger"));
        let effect = clean_value(condition.get("effect"));
        let timing = clean_value(condition.get("date_or_timing"));
        if [trigger.as_str(), effect.as_str(), timing.as_str()]
            .iter()
            .all(|part| part.is_empty())
        {
            continue;
        }

        let line = if trigger.is_empty() {
            format!("- {}", if effect.is_empty() { &timing } else { &effect })
        } else {
            format!("- **{trigger}:** {effect}").trim_end().to_string()
        };
        lines.push(line);
        append_item_detail(&mut lines, "Timing", &timing);
    }
    lines.join("\n")
}

fn important_dates(document: &JsonObject) -> String {
    let mut lines = Vec::new();
    let Some(Value::Array(dates)) = document.get("important_dates") else {
        return String::new();
    };

    for date_entry in dates {
        let Value::Object(date_entry) = date_entry else {
            continue;
        };
        let date_text = clean_value(date_entry.get("date"));
        let meaning = clean_value(date_entry.get("meaning"));
        if [date_text.as_str(), meaning.as_str()]
            .iter()
            .all(|part| part.is_empty())
        {
            continue;
        }

        if !date_text.is_empty() && !meaning.is_empty() {
            lines.push(format!("- **{date_text}:** {meaning}"));
        } else {
            lines.push(format!(
                "- {}",
                if date_text.is_empty() {
                    &meaning
                } else {
                    &date_text
                }
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::parse_json_object;

    fn render_one(text: &str) -> ProducedChunks {
        render(&parse_json_object(text))
    }

    #[test]
    fn renders_full_document_analysis_in_section_order() {
        let produced = render_one(
            r#"{"overview":"Miller Family Trust Amendment updates fiduciaries.","parties":[{"name":"Priya Shah","role":"primary trustee","formal_term":"Trustee","appointment_tier":"primary","context":"Responsible for initial trust administration."}],"key_provisions":[{"type":"distribution power","text":"Trustee may distribute education expenses.","applies_to":"Miller grandchildren"}],"assets":[{"name":"Brokerage Account","asset_type":"financial_account","disposition":"Transferred to the continuing trust."}],"conditions":[{"trigger":"Settlor's death","effect":"Successor trustee takes office.","date_or_timing":"Upon written acceptance."}],"important_dates":[{"date":"the third anniversary of the Settlor's death","meaning":"Mandatory accounting deadline."}],"summary":"Quick reference summary for fiduciary succession."}"#,
        );
        assert_eq!(produced.agent_override.as_deref(), Some("documents"));
        assert_eq!(produced.chunks.len(), 1);
        let rendered = &produced.chunks[0].content;
        let headings = [
            "Overview",
            "Parties and Roles",
            "Key Provisions",
            "Assets and Property",
            "Conditions and Triggers",
            "Important Dates",
            "Summary",
        ];
        let positions: Vec<_> = headings
            .iter()
            .map(|heading| rendered.find(&format!("## {heading}")).expect("heading"))
            .collect();
        assert!(positions.windows(2).all(|window| window[0] < window[1]));
        assert!(rendered.contains("Priya Shah - primary trustee (Trustee) [primary]"));
        assert!(rendered.contains("  Responsible for initial trust administration."));
        assert!(
            rendered
                .contains("- **distribution power:** Trustee may distribute education expenses.")
        );
        assert!(rendered.contains("  Applies to: Miller grandchildren"));
        assert!(rendered.contains(
            "- **Brokerage Account** (financial_account) - Transferred to the continuing trust."
        ));
        assert!(rendered.contains("- **Settlor's death:** Successor trustee takes office."));
        assert!(rendered.contains("  Timing: Upon written acceptance."));
        assert!(rendered.contains(
            "- **the third anniversary of the Settlor's death:** Mandatory accounting deadline."
        ));
    }

    #[test]
    fn empty_object_renders_all_sections_with_absent_text() {
        let produced = render_one("{}");
        assert_eq!(produced.agent_override.as_deref(), Some("documents"));
        assert_eq!(produced.chunks.len(), 1);
        let rendered = &produced.chunks[0].content;
        assert_eq!(rendered.matches("## ").count(), 7);
        assert_eq!(rendered.matches(ABSENT_TEXT).count(), 7);
    }

    #[test]
    fn skips_empty_items_and_uses_section_fallback_labels() {
        let produced = render_one(
            r#"{"parties":[{},[],{"role":"reviewer","appointment_tier":"not_applicable"}],"key_provisions":[{},{"applies_to":"Beneficiary"}],"assets":[{},{"asset_type":"unspecified"}],"conditions":[{},{"date_or_timing":"At closing"}],"important_dates":[{},{"meaning":"Vesting deadline"}]}"#,
        );
        let rendered = &produced.chunks[0].content;
        assert!(rendered.contains("- Unnamed party - reviewer"));
        assert!(!rendered.contains("not_applicable"));
        assert!(rendered.contains("## Key Provisions\n\n-\n  Applies to: Beneficiary"));
        assert!(rendered.contains("- Unnamed asset"));
        assert!(!rendered.contains("unspecified"));
        assert!(rendered.contains("- At closing\n  Timing: At closing"));
        assert!(rendered.contains("- Vesting deadline"));
    }

    #[test]
    fn missing_or_non_object_record_keeps_agent_and_zero_chunks() {
        let produced = render(&[]);
        assert_eq!(produced.agent_override.as_deref(), Some("documents"));
        assert!(produced.chunks.is_empty());

        let produced = render_one("[]");
        assert_eq!(produced.agent_override.as_deref(), Some("documents"));
        assert!(produced.chunks.is_empty());
    }
}
