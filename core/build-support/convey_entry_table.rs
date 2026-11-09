// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// What each dashboard view needs at the start of a page, generated from its
// fragment. Included by the shell's build script and by its tests, after
// `convey_shell_bundle.rs`, whose tag reader it shares.
//
// A fragment names what it loads with ordinary tags. It names its first reads
// with a comment the build reads, beside the code that makes them:
//
//     <!-- first-read: /app/home/ -> /app/home/api/pulse /app/home/api/briefing -->
//     <!-- first-read: /app/transcripts/{day} -> /app/transcripts/api/day/{day} -->
//     <!-- open-day: /app/transcripts/ -> /app/transcripts/api/open-day -->
//
// A route is a whole path, whose only variable is `{day}`, eight digits. A read
// names only what the view would ask for at that route with no other state, so
// the journal's own clock decides any date a path doesn't carry. `open-day` is
// a read that names the day of an undated route; the page takes that address
// before the view mounts.

/// One script a fragment loads.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragmentScript {
    pub src: String,
    pub route_prefix: Option<String>,
}

/// One route's first reads.
#[allow(dead_code)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FragmentReads {
    pub route: String,
    pub urls: Vec<String>,
}

/// A fragment's tags and declared reads, in document order.
#[allow(dead_code)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FragmentFacts {
    pub scripts: Vec<FragmentScript>,
    pub styles: Vec<String>,
    pub reads: Vec<FragmentReads>,
    pub open_day: Option<FragmentReads>,
}

/// The marker in the shell page the generated table replaces. Standing alone
/// it is an empty object, so a page served unedited starts nothing early.
#[allow(dead_code)]
pub const ENTRY_TABLE_MARKER: &str = "{/*entry-table*/}";

fn check_route(fragment: &str, route: &str) -> Result<(), String> {
    if !route.starts_with("/app/") || route.contains(['?', '#']) {
        return Err(format!("{fragment}: first-read route {route} isn't a whole /app/ path"));
    }
    for part in route.split('/') {
        if part.contains(['{', '}']) && part != "{day}" {
            return Err(format!("{fragment}: first-read route {route}: only {{day}} may vary"));
        }
    }
    Ok(())
}

fn check_url(fragment: &str, route: &str, url: &str) -> Result<(), String> {
    if !url.starts_with('/') {
        return Err(format!("{fragment}: first read {url} isn't a path from the root"));
    }
    if url.contains("{day}") && !route.contains("{day}") {
        return Err(format!(
            "{fragment}: first read {url} names a day its route {route} doesn't carry; a page \
             never works out a date for itself"
        ));
    }
    let rest = url.replace("{day}", "");
    if rest.contains(['{', '}']) {
        return Err(format!("{fragment}: first read {url}: only {{day}} may vary"));
    }
    Ok(())
}

fn parse_declaration(fragment: &str, text: &str) -> Result<Option<(bool, FragmentReads)>, String> {
    let (open_day, rest) = if let Some(rest) = text.strip_prefix("first-read:") {
        (false, rest)
    } else if let Some(rest) = text.strip_prefix("open-day:") {
        (true, rest)
    } else {
        return Ok(None);
    };
    let (route, urls) = rest
        .split_once("->")
        .ok_or_else(|| format!("{fragment}: a first-read comment needs `route -> url`: {text}"))?;
    let route = route.trim().to_owned();
    check_route(fragment, &route)?;
    let urls: Vec<String> = urls.split_whitespace().map(str::to_owned).collect();
    if urls.is_empty() || (open_day && urls.len() != 1) {
        return Err(format!("{fragment}: wrong number of reads in: {text}"));
    }
    for url in &urls {
        check_url(fragment, &route, url)?;
    }
    if open_day && (route.contains("{day}") || !route.ends_with('/')) {
        return Err(format!(
            "{fragment}: open-day is for a route with no day, ending in /: {route}"
        ));
    }
    Ok(Some((open_day, FragmentReads { route, urls })))
}

/// Read a fragment's tags the way the mount does: script and style bodies and
/// `<template>` contents are skipped, and an unclosed tag or body is refused.
#[allow(dead_code)]
pub fn fragment_facts(name: &str, fragment: &str) -> Result<FragmentFacts, String> {
    let lower = fragment.to_ascii_lowercase();
    let mut facts = FragmentFacts::default();
    let mut cursor = 0;
    let unclosed = |what: &str| format!("{name}: an unclosed {what}");
    let skip_to = |from: usize, end: &str, what: &str| -> Result<usize, String> {
        lower[from..]
            .find(end)
            .map(|at| from + at + end.len())
            .ok_or_else(|| unclosed(what))
    };
    while let Some(found) = lower[cursor..].find('<') {
        let start = cursor + found;
        cursor = start + 1;
        let rest = &lower[start..];
        if rest.starts_with("<!--") {
            let end = skip_to(start, "-->", "comment")?;
            let text = fragment[start + 4..end - 3].trim();
            if let Some((open_day, reads)) = parse_declaration(name, text)? {
                if open_day {
                    if facts.open_day.replace(reads).is_some() {
                        return Err(format!("{name}: more than one open-day read"));
                    }
                } else {
                    facts.reads.push(reads);
                }
            }
            cursor = end;
            continue;
        }
        if rest.starts_with("<style") {
            cursor = skip_to(start, "</style>", "style")?;
            continue;
        }
        if rest.starts_with("<template") {
            cursor = skip_to(start, "</template>", "template")?;
            continue;
        }
        let is_script = rest.starts_with("<script");
        let is_link = rest.starts_with("<link");
        if !is_script && !is_link {
            continue;
        }
        let close = skip_to(start, ">", "tag")?;
        let tag = &fragment[start..close];
        cursor = close;
        if is_link {
            let rel = attribute(tag, "rel").unwrap_or_default().to_ascii_lowercase();
            if rel.split_whitespace().any(|word| word == "stylesheet") {
                let href = attribute(tag, "href")
                    .ok_or_else(|| format!("{name}: a stylesheet tag with no href: {tag}"))?;
                facts.styles.push(href.to_owned());
            }
            continue;
        }
        if let Some(src) = attribute(tag, "src") {
            facts.scripts.push(FragmentScript {
                src: src.to_owned(),
                route_prefix: attribute(tag, "data-route-prefix").map(str::to_owned),
            });
        }
        cursor = skip_to(close, "</script>", "script")?;
    }
    Ok(facts)
}

/// One view's row of the table, with every asset at its hashed name. An asset
/// `hashed` can't name is left out: the fragment's own tag still loads it.
#[allow(dead_code)]
pub fn entry_row(
    fragment_url: &str,
    facts: &FragmentFacts,
    hashed: &dyn Fn(&str) -> Option<String>,
) -> serde_json::Value {
    let scripts: Vec<serde_json::Value> = facts
        .scripts
        .iter()
        .filter_map(|script| {
            let src = hashed(&script.src)?;
            let mut row = serde_json::Map::new();
            row.insert("src".to_owned(), src.into());
            if let Some(prefix) = &script.route_prefix {
                row.insert("routePrefix".to_owned(), prefix.clone().into());
            }
            Some(serde_json::Value::Object(row))
        })
        .collect();
    let styles: Vec<String> = facts.styles.iter().filter_map(|href| hashed(href)).collect();
    let reads = |rows: &[&FragmentReads]| -> Vec<serde_json::Value> {
        rows.iter()
            .map(|row| serde_json::json!({ "route": row.route, "urls": row.urls }))
            .collect()
    };
    let mut row = serde_json::json!({
        "fragment": fragment_url,
        "scripts": scripts,
        "styles": styles,
        "reads": reads(&facts.reads.iter().collect::<Vec<_>>()),
    });
    if let Some(open_day) = &facts.open_day {
        row["openDay"] = serde_json::json!({ "route": open_day.route, "url": open_day.urls[0] });
    }
    row
}

/// The table as text to set inside an inline script: JSON with no `<`.
#[allow(dead_code)]
pub fn entry_table_text(table: &serde_json::Value) -> String {
    serde_json::to_string(table)
        .expect("entry table serializes")
        .replace('<', "\\u003c")
}
