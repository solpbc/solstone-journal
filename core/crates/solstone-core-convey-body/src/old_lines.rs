// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the Body app says about old imports: the lines on a day page and the
//! overview's listing of old bundles.

use std::collections::BTreeSet;
use std::path::Path;

use serde_json::{Value, json};
use solstone_core_location::old_imports::{
    CoverScope, OldBundle, OldCover, OldSource, OldSpan, covers_on_day, month_words,
};

use crate::fmt::short_day_year;

fn import_again(source: OldSource) -> &'static str {
    match source {
        OldSource::AppleHealth => "import again from a new Apple Health export to bring it in.",
        OldSource::Oura => "import again from your Oura account to bring it in.",
        OldSource::DexcomClarity => "there's no importer for Dexcom Clarity now, so it stays as it is.",
    }
}

fn day_line(cover: &OldCover) -> String {
    let source = cover.source.words();
    match &cover.scope {
        CoverScope::Day => format!(
            "an earlier {source} import holds this day's body data, kept but not read here. {}",
            import_again(cover.source)
        ),
        CoverScope::Month(month) => format!(
            "an earlier {source} import holds body data for {}, kept but not read here, so it doesn't show on this page. {}",
            month_words(month).unwrap_or_else(|| month.clone()),
            import_again(cover.source)
        ),
    }
}

/// The day page's lines: one per source an old import covers and the new path
/// hasn't touched that day.
pub(crate) fn day_lines(root: &Path, day: &str) -> Vec<Value> {
    covers_on_day(root, day, None)
        .iter()
        .map(|cover| {
            json!({
                "source": cover.source.words(),
                "scope": match cover.scope {
                    CoverScope::Day => "day",
                    CoverScope::Month(_) => "month",
                },
                "line": day_line(cover),
            })
        })
        .collect()
}

fn span_label(span: &OldSpan) -> String {
    match span {
        OldSpan::Days(days) => {
            let first = days.iter().next().cloned().unwrap_or_default();
            let last = days.iter().next_back().cloned().unwrap_or_default();
            let first_label = short_day_year(&first).unwrap_or(first);
            if days.len() == 1 {
                first_label
            } else {
                let last_label = short_day_year(&last).unwrap_or(last);
                format!("{first_label} to {last_label}, {} days", days.len())
            }
        }
        OldSpan::Months(months) => {
            let words = |month: &str| month_words(month).unwrap_or_else(|| month.to_owned());
            match (months.iter().next(), months.iter().next_back()) {
                (Some(only), Some(last)) if only == last => format!("{} (by month)", words(only)),
                (Some(first), Some(last)) => format!(
                    "{} to {} ({} months)",
                    words(first),
                    words(last),
                    months.len()
                ),
                _ => "an unknown span".to_owned(),
            }
        }
    }
}

fn scope_word(span: &OldSpan) -> &'static str {
    match span {
        OldSpan::Days(_) => "days",
        OldSpan::Months(_) => "months",
    }
}

/// One old bundle on the overview: its source, what it spans and what to do.
pub(crate) fn listing_item(bundle: &OldBundle) -> Value {
    let span = span_label(&bundle.span);
    let line = match bundle.source {
        OldSource::DexcomClarity => format!(
            "{} from {span}, kept but not read here. there's no importer for Dexcom Clarity now, so it stays as it is.",
            bundle.source.words()
        ),
        OldSource::AppleHealth => format!(
            "{} from {span}, kept but not read here. import again from a new Apple Health export to bring it in.",
            bundle.source.words()
        ),
        OldSource::Oura => format!(
            "{} from {span}, kept but not read here. import again from your Oura account to bring it in.",
            bundle.source.words()
        ),
    };
    json!({
        "id": bundle.id,
        "source": bundle.source.words(),
        "span_by": scope_word(&bundle.span),
        "span": span,
        "line": line,
        "importer": bundle.source != OldSource::DexcomClarity,
    })
}

/// Every source name that has at least one old bundle, in a stable order.
pub(crate) fn sources_with_old(bundles: &[OldBundle]) -> BTreeSet<&'static str> {
    bundles.iter().map(|bundle| bundle.source.words()).collect()
}
