// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Native `solstone journal experiences` CLI surface.
//!
//! Supports: `list`, `show`, `create`, `update`, `add-activity`,
//! `remove-activity`, `mute`, `unmute`, `detect`, and `recompose`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use solstone_core_facets::{
    ActivityRef, ExperienceActivitiesMutation, ExperienceDirectPatch, ExperienceMoment,
    ExperienceRecord, ShareDetailPolicy, create_owner_experience, get_experience,
    hydrate_experience, list_declared_facet_names, list_experiences_in_range,
    load_experiences_overlapping_day, mutate_experience_activities, project_experience_for_share,
    set_experience_hidden, update_experience_direct,
};
use solstone_core_generate::GenerateExecutor;
use solstone_core_journal_io::is_day_key;
use solstone_core_system::owner_ask::ask_experience_compose;
use solstone_core_talent_runtime::experiences::{run_compose_experience, run_detect_day};

pub const EXPERIENCES_USAGE: &str = "usage: solstone journal experiences [-h] {list,show,create,update,add-activity,remove-activity,mute,unmute,detect,recompose} ...\n";

const EXPERIENCES_HELP: &str = "\
usage: solstone journal experiences [-h] {list,show,create,update,add-activity,remove-activity,mute,unmute,detect,recompose} ...

Manage and inspect journal Experiences.

subcommands:
  list              List Experiences for a day or date range
  show              Show one hydrated Experience (or a share-tier projection)
  create            Create a manual Experience from one or more activities
  update            Edit an Experience's title, summary, locations, or holds
  add-activity      Add one activity (same-facet or cross-facet) to an Experience
  remove-activity   Remove one activity from an Experience
  mute              Hide an Experience (and prevent automatic re-detection)
  unmute            Restore a muted Experience
  detect            Run two-stage Experience detection for a day
  recompose         Re-run Experience composition for one Experience

options:
  -h, --help        Show this help message and exit
";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliRun {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u8,
}

impl CliRun {
    fn ok(stdout: String) -> Self {
        Self {
            stdout,
            stderr: String::new(),
            exit_code: 0,
        }
    }

    fn usage(message: impl Into<String>) -> Self {
        Self {
            stdout: String::new(),
            stderr: format!("{EXPERIENCES_USAGE}{}\n", message.into()),
            exit_code: 2,
        }
    }

    fn fail(exit_code: u8, message: impl Into<String>) -> Self {
        Self {
            stdout: String::new(),
            stderr: format!("{}\n", message.into()),
            exit_code,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExperiencesCmd {
    Help(String),
    Usage(String),
    List {
        journal: Option<PathBuf>,
        day: Option<String>,
        from: Option<String>,
        to: Option<String>,
        facet: Option<String>,
        include_hidden: bool,
        json: bool,
    },
    Show {
        journal: Option<PathBuf>,
        day: String,
        facet: String,
        id: String,
        share_tier: Option<ShareDetailPolicy>,
        include_cross_facet_candidates: bool,
        json: bool,
    },
    Create {
        journal: Option<PathBuf>,
        facet: String,
        activities: Vec<ActivityRef>,
        title: Option<String>,
        summary: Option<String>,
        json: bool,
    },
    Update {
        journal: Option<PathBuf>,
        day: String,
        facet: String,
        id: String,
        title: Option<String>,
        summary: Option<String>,
        locations: Option<Vec<String>>,
        moments_json: Option<String>,
        release_holds: Vec<String>,
        note: Option<String>,
        json: bool,
    },
    AddActivity {
        journal: Option<PathBuf>,
        day: String,
        facet: String,
        id: String,
        activity: ActivityRef,
        note: Option<String>,
        json: bool,
    },
    RemoveActivity {
        journal: Option<PathBuf>,
        day: String,
        facet: String,
        id: String,
        activity: ActivityRef,
        note: Option<String>,
        json: bool,
    },
    Mute {
        journal: Option<PathBuf>,
        day: String,
        facet: String,
        id: String,
        hidden: bool,
        note: Option<String>,
        json: bool,
    },
    Detect {
        journal: Option<PathBuf>,
        day: String,
        facets: Vec<String>,
        compose: bool,
        talent_root: Option<PathBuf>,
        json: bool,
    },
    Recompose {
        journal: Option<PathBuf>,
        day: String,
        facet: String,
        id: String,
        talent_root: Option<PathBuf>,
        json: bool,
    },
}

impl ExperiencesCmd {
    pub fn journal_override(&self) -> Option<&PathBuf> {
        match self {
            Self::Help(_) | Self::Usage(_) => None,
            Self::List { journal, .. }
            | Self::Show { journal, .. }
            | Self::Create { journal, .. }
            | Self::Update { journal, .. }
            | Self::AddActivity { journal, .. }
            | Self::RemoveActivity { journal, .. }
            | Self::Mute { journal, .. }
            | Self::Detect { journal, .. }
            | Self::Recompose { journal, .. } => journal.as_ref(),
        }
    }

    pub fn needs_journal(&self) -> bool {
        !matches!(self, Self::Help(_) | Self::Usage(_))
    }

    pub fn needs_generate(&self) -> bool {
        matches!(self, Self::Detect { .. } | Self::Recompose { .. })
    }

    pub fn talent_root_override(&self) -> Option<&PathBuf> {
        match self {
            Self::Detect { talent_root, .. } | Self::Recompose { talent_root, .. } => {
                talent_root.as_ref()
            }
            _ => None,
        }
    }
}

/// Parse an activity reference spec `YYYYMMDD:stream:id` or `YYYYMMDD:stream:id:facet`.
pub fn parse_activity_spec(raw: &str, explicit_facet: Option<&str>) -> Result<ActivityRef, String> {
    let parts: Vec<&str> = raw.split(':').collect();
    if parts.len() != 3 && parts.len() != 4 {
        return Err(format!(
            "invalid --activity `{raw}`: expected DAY:STREAM:ID or DAY:STREAM:ID:FACET"
        ));
    }
    let day = parts[0].trim();
    let stream = parts[1].trim();
    let id = parts[2].trim();
    if !is_day_key(day) || stream.is_empty() || id.is_empty() {
        return Err(format!(
            "invalid --activity `{raw}`: day must be YYYYMMDD and stream/id non-empty"
        ));
    }
    let spec_facet = parts
        .get(3)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let facet = explicit_facet
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or(spec_facet);
    Ok(ActivityRef {
        day: day.to_owned(),
        stream: stream.to_owned(),
        id: id.to_owned(),
        facet,
    })
}

fn parse_share_tier(raw: &str) -> Result<ShareDetailPolicy, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "overview" | "overview_only" => Ok(ShareDetailPolicy::OverviewOnly),
        "moments" | "overview_moments_media" => Ok(ShareDetailPolicy::OverviewMomentsMedia),
        "full" | "full_hydrated_detail" => Ok(ShareDetailPolicy::FullHydratedDetail),
        other => Err(format!(
            "invalid --share-tier `{other}`: expected overview, moments, or full"
        )),
    }
}

fn next_value(args: &[String], idx: &mut usize, flag: &str) -> Result<String, String> {
    let Some(val) = args.get(*idx + 1) else {
        return Err(format!("option `{flag}` requires a value"));
    };
    *idx += 1;
    Ok(val.clone())
}

fn subcommand_help(sub: &str) -> String {
    match sub {
        "list" => "usage: solstone journal experiences list [-h] [--day YYYYMMDD | --from YYYYMMDD --to YYYYMMDD] [--facet SLUG] [--include-hidden] [--journal PATH] [--json]\n".to_owned(),
        "show" => "usage: solstone journal experiences show [-h] --day YYYYMMDD --facet SLUG --id EXP_ID [--share-tier {overview,moments,full}] [--include-cross-facet-candidates] [--journal PATH] [--json]\n".to_owned(),
        "create" => "usage: solstone journal experiences create [-h] --facet SLUG --activity DAY:STREAM:ID[:FACET] [--activity ...] [--title TEXT] [--summary TEXT] [--journal PATH] [--json]\n".to_owned(),
        "update" => "usage: solstone journal experiences update [-h] --day YYYYMMDD --facet SLUG --id EXP_ID [--title TEXT] [--summary TEXT] [--location TEXT]... [--clear-locations] [--moments-json JSON] [--release-hold NAME]... [--note TEXT] [--journal PATH] [--json]\n".to_owned(),
        "add-activity" => "usage: solstone journal experiences add-activity [-h] --day YYYYMMDD --facet SLUG --id EXP_ID --activity DAY:STREAM:ID[:FACET] [--activity-facet SLUG] [--note TEXT] [--journal PATH] [--json]\n".to_owned(),
        "remove-activity" => "usage: solstone journal experiences remove-activity [-h] --day YYYYMMDD --facet SLUG --id EXP_ID --activity DAY:STREAM:ID[:FACET] [--activity-facet SLUG] [--note TEXT] [--journal PATH] [--json]\n".to_owned(),
        "mute" => "usage: solstone journal experiences mute [-h] --day YYYYMMDD --facet SLUG --id EXP_ID [--note TEXT] [--journal PATH] [--json]\n".to_owned(),
        "unmute" => "usage: solstone journal experiences unmute [-h] --day YYYYMMDD --facet SLUG --id EXP_ID [--note TEXT] [--journal PATH] [--json]\n".to_owned(),
        "detect" => "usage: solstone journal experiences detect [-h] --day YYYYMMDD [--facet SLUG]... [--compose] [--talent-root PATH] [--journal PATH] [--json]\n".to_owned(),
        "recompose" => "usage: solstone journal experiences recompose [-h] --day YYYYMMDD --facet SLUG --id EXP_ID [--talent-root PATH] [--journal PATH] [--json]\n".to_owned(),
        _ => EXPERIENCES_HELP.to_owned(),
    }
}

pub fn parse(raw_args: &[OsString]) -> ExperiencesCmd {
    let Some(args) = raw_args
        .iter()
        .map(|a| a.to_str().map(str::to_owned))
        .collect::<Option<Vec<_>>>()
    else {
        return ExperiencesCmd::Usage("arguments must be valid UTF-8".to_owned());
    };
    let Some((sub, rest)) = args.split_first() else {
        return ExperiencesCmd::Usage("missing subcommand".to_owned());
    };
    if sub == "-h" || sub == "--help" {
        return ExperiencesCmd::Help(EXPERIENCES_HELP.to_owned());
    }
    if rest.iter().any(|a| a == "-h" || a == "--help") {
        return match sub.as_str() {
            "list" | "show" | "create" | "update" | "add-activity" | "remove-activity" | "mute"
            | "unmute" | "detect" | "recompose" => ExperiencesCmd::Help(subcommand_help(sub)),
            other => ExperiencesCmd::Usage(format!("unrecognized subcommand `{other}`")),
        };
    }

    match sub.as_str() {
        "list" => parse_list(rest),
        "show" => parse_show(rest),
        "create" => parse_create(rest),
        "update" => parse_update(rest),
        "add-activity" => parse_activity_mutation(rest, true),
        "remove-activity" => parse_activity_mutation(rest, false),
        "mute" => parse_mute(rest, true),
        "unmute" => parse_mute(rest, false),
        "detect" => parse_detect(rest),
        "recompose" => parse_recompose(rest),
        other => ExperiencesCmd::Usage(format!("unrecognized subcommand `{other}`")),
    }
}

fn parse_list(args: &[String]) -> ExperiencesCmd {
    let mut journal = None;
    let mut day = None;
    let mut from = None;
    let mut to = None;
    let mut facet = None;
    let mut include_hidden = false;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--journal" => match next_value(args, &mut i, "--journal") {
                Ok(v) => journal = Some(PathBuf::from(v)),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--day" => match next_value(args, &mut i, "--day") {
                Ok(v) if is_day_key(&v) => day = Some(v),
                Ok(v) => return ExperiencesCmd::Usage(format!("invalid --day `{v}`")),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--from" => match next_value(args, &mut i, "--from") {
                Ok(v) if is_day_key(&v) => from = Some(v),
                Ok(v) => return ExperiencesCmd::Usage(format!("invalid --from `{v}`")),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--to" => match next_value(args, &mut i, "--to") {
                Ok(v) if is_day_key(&v) => to = Some(v),
                Ok(v) => return ExperiencesCmd::Usage(format!("invalid --to `{v}`")),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--facet" => match next_value(args, &mut i, "--facet") {
                Ok(v) => facet = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--include-hidden" => include_hidden = true,
            "--json" => json = true,
            other => return ExperiencesCmd::Usage(format!("unrecognized argument `{other}`")),
        }
        i += 1;
    }
    if day.is_some() && (from.is_some() || to.is_some()) {
        return ExperiencesCmd::Usage("cannot combine --day with --from/--to".to_owned());
    }
    ExperiencesCmd::List {
        journal,
        day,
        from,
        to,
        facet,
        include_hidden,
        json,
    }
}

fn parse_show(args: &[String]) -> ExperiencesCmd {
    let mut journal = None;
    let mut day = None;
    let mut facet = None;
    let mut id = None;
    let mut share_tier = None;
    let mut include_cross_facet_candidates = false;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--journal" => match next_value(args, &mut i, "--journal") {
                Ok(v) => journal = Some(PathBuf::from(v)),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--day" => match next_value(args, &mut i, "--day") {
                Ok(v) if is_day_key(&v) => day = Some(v),
                Ok(v) => return ExperiencesCmd::Usage(format!("invalid --day `{v}`")),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--facet" => match next_value(args, &mut i, "--facet") {
                Ok(v) => facet = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--id" => match next_value(args, &mut i, "--id") {
                Ok(v) => id = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--share-tier" => {
                match next_value(args, &mut i, "--share-tier").and_then(|v| parse_share_tier(&v)) {
                    Ok(p) => share_tier = Some(p),
                    Err(e) => return ExperiencesCmd::Usage(e),
                }
            }
            "--include-cross-facet-candidates" => include_cross_facet_candidates = true,
            "--json" => json = true,
            other => return ExperiencesCmd::Usage(format!("unrecognized argument `{other}`")),
        }
        i += 1;
    }
    let (Some(day), Some(facet), Some(id)) = (day, facet, id) else {
        return ExperiencesCmd::Usage("show requires --day, --facet, and --id".to_owned());
    };
    ExperiencesCmd::Show {
        journal,
        day,
        facet,
        id,
        share_tier,
        include_cross_facet_candidates,
        json,
    }
}

fn parse_create(args: &[String]) -> ExperiencesCmd {
    let mut journal = None;
    let mut facet = None;
    let mut activities = Vec::new();
    let mut title = None;
    let mut summary = None;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--journal" => match next_value(args, &mut i, "--journal") {
                Ok(v) => journal = Some(PathBuf::from(v)),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--facet" => match next_value(args, &mut i, "--facet") {
                Ok(v) => facet = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--activity" => match next_value(args, &mut i, "--activity")
                .and_then(|v| parse_activity_spec(&v, None))
            {
                Ok(a) => activities.push(a),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--title" => match next_value(args, &mut i, "--title") {
                Ok(v) => title = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--summary" => match next_value(args, &mut i, "--summary") {
                Ok(v) => summary = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--json" => json = true,
            other => return ExperiencesCmd::Usage(format!("unrecognized argument `{other}`")),
        }
        i += 1;
    }
    let Some(facet) = facet else {
        return ExperiencesCmd::Usage("create requires --facet".to_owned());
    };
    if activities.is_empty() {
        return ExperiencesCmd::Usage("create requires at least one --activity".to_owned());
    }
    ExperiencesCmd::Create {
        journal,
        facet,
        activities,
        title,
        summary,
        json,
    }
}

fn parse_update(args: &[String]) -> ExperiencesCmd {
    let mut journal = None;
    let mut day = None;
    let mut facet = None;
    let mut id = None;
    let mut title = None;
    let mut summary = None;
    let mut locations: Option<Vec<String>> = None;
    let mut moments_json = None;
    let mut release_holds = Vec::new();
    let mut note = None;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--journal" => match next_value(args, &mut i, "--journal") {
                Ok(v) => journal = Some(PathBuf::from(v)),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--day" => match next_value(args, &mut i, "--day") {
                Ok(v) if is_day_key(&v) => day = Some(v),
                Ok(v) => return ExperiencesCmd::Usage(format!("invalid --day `{v}`")),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--facet" => match next_value(args, &mut i, "--facet") {
                Ok(v) => facet = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--id" => match next_value(args, &mut i, "--id") {
                Ok(v) => id = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--title" => match next_value(args, &mut i, "--title") {
                Ok(v) => title = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--summary" => match next_value(args, &mut i, "--summary") {
                Ok(v) => summary = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--location" => match next_value(args, &mut i, "--location") {
                Ok(v) => locations.get_or_insert_with(Vec::new).push(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--clear-locations" => locations = Some(Vec::new()),
            "--moments-json" => match next_value(args, &mut i, "--moments-json") {
                Ok(v) => moments_json = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--release-hold" => match next_value(args, &mut i, "--release-hold") {
                Ok(v) => release_holds.push(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--note" => match next_value(args, &mut i, "--note") {
                Ok(v) => note = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--json" => json = true,
            other => return ExperiencesCmd::Usage(format!("unrecognized argument `{other}`")),
        }
        i += 1;
    }
    let (Some(day), Some(facet), Some(id)) = (day, facet, id) else {
        return ExperiencesCmd::Usage("update requires --day, --facet, and --id".to_owned());
    };
    ExperiencesCmd::Update {
        journal,
        day,
        facet,
        id,
        title,
        summary,
        locations,
        moments_json,
        release_holds,
        note,
        json,
    }
}

fn parse_activity_mutation(args: &[String], is_add: bool) -> ExperiencesCmd {
    let mut journal = None;
    let mut day = None;
    let mut facet = None;
    let mut id = None;
    let mut activity_raw = None;
    let mut activity_facet = None;
    let mut note = None;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--journal" => match next_value(args, &mut i, "--journal") {
                Ok(v) => journal = Some(PathBuf::from(v)),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--day" => match next_value(args, &mut i, "--day") {
                Ok(v) if is_day_key(&v) => day = Some(v),
                Ok(v) => return ExperiencesCmd::Usage(format!("invalid --day `{v}`")),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--facet" => match next_value(args, &mut i, "--facet") {
                Ok(v) => facet = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--id" => match next_value(args, &mut i, "--id") {
                Ok(v) => id = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--activity" => match next_value(args, &mut i, "--activity") {
                Ok(v) => activity_raw = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--activity-facet" => match next_value(args, &mut i, "--activity-facet") {
                Ok(v) => activity_facet = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--note" => match next_value(args, &mut i, "--note") {
                Ok(v) => note = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--json" => json = true,
            other => return ExperiencesCmd::Usage(format!("unrecognized argument `{other}`")),
        }
        i += 1;
    }
    let verb = if is_add {
        "add-activity"
    } else {
        "remove-activity"
    };
    let (Some(day), Some(facet), Some(id), Some(activity_raw)) = (day, facet, id, activity_raw)
    else {
        return ExperiencesCmd::Usage(format!(
            "{verb} requires --day, --facet, --id, and --activity"
        ));
    };
    let activity = match parse_activity_spec(&activity_raw, activity_facet.as_deref()) {
        Ok(a) => a,
        Err(e) => return ExperiencesCmd::Usage(e),
    };
    if is_add {
        ExperiencesCmd::AddActivity {
            journal,
            day,
            facet,
            id,
            activity,
            note,
            json,
        }
    } else {
        ExperiencesCmd::RemoveActivity {
            journal,
            day,
            facet,
            id,
            activity,
            note,
            json,
        }
    }
}

fn parse_mute(args: &[String], hidden: bool) -> ExperiencesCmd {
    let mut journal = None;
    let mut day = None;
    let mut facet = None;
    let mut id = None;
    let mut note = None;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--journal" => match next_value(args, &mut i, "--journal") {
                Ok(v) => journal = Some(PathBuf::from(v)),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--day" => match next_value(args, &mut i, "--day") {
                Ok(v) if is_day_key(&v) => day = Some(v),
                Ok(v) => return ExperiencesCmd::Usage(format!("invalid --day `{v}`")),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--facet" => match next_value(args, &mut i, "--facet") {
                Ok(v) => facet = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--id" => match next_value(args, &mut i, "--id") {
                Ok(v) => id = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--note" => match next_value(args, &mut i, "--note") {
                Ok(v) => note = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--json" => json = true,
            other => return ExperiencesCmd::Usage(format!("unrecognized argument `{other}`")),
        }
        i += 1;
    }
    let verb = if hidden { "mute" } else { "unmute" };
    let (Some(day), Some(facet), Some(id)) = (day, facet, id) else {
        return ExperiencesCmd::Usage(format!("{verb} requires --day, --facet, and --id"));
    };
    ExperiencesCmd::Mute {
        journal,
        day,
        facet,
        id,
        hidden,
        note,
        json,
    }
}

fn parse_detect(args: &[String]) -> ExperiencesCmd {
    let mut journal = None;
    let mut day = None;
    let mut facets = Vec::new();
    let mut compose = false;
    let mut talent_root = None;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--journal" => match next_value(args, &mut i, "--journal") {
                Ok(v) => journal = Some(PathBuf::from(v)),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--day" => match next_value(args, &mut i, "--day") {
                Ok(v) if is_day_key(&v) => day = Some(v),
                Ok(v) => return ExperiencesCmd::Usage(format!("invalid --day `{v}`")),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--facet" => match next_value(args, &mut i, "--facet") {
                Ok(v) => facets.push(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--compose" => compose = true,
            "--talent-root" => match next_value(args, &mut i, "--talent-root") {
                Ok(v) => talent_root = Some(PathBuf::from(v)),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--json" => json = true,
            other => return ExperiencesCmd::Usage(format!("unrecognized argument `{other}`")),
        }
        i += 1;
    }
    let Some(day) = day else {
        return ExperiencesCmd::Usage("detect requires --day YYYYMMDD".to_owned());
    };
    ExperiencesCmd::Detect {
        journal,
        day,
        facets,
        compose,
        talent_root,
        json,
    }
}

fn parse_recompose(args: &[String]) -> ExperiencesCmd {
    let mut journal = None;
    let mut day = None;
    let mut facet = None;
    let mut id = None;
    let mut talent_root = None;
    let mut json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--journal" => match next_value(args, &mut i, "--journal") {
                Ok(v) => journal = Some(PathBuf::from(v)),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--day" => match next_value(args, &mut i, "--day") {
                Ok(v) if is_day_key(&v) => day = Some(v),
                Ok(v) => return ExperiencesCmd::Usage(format!("invalid --day `{v}`")),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--facet" => match next_value(args, &mut i, "--facet") {
                Ok(v) => facet = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--id" => match next_value(args, &mut i, "--id") {
                Ok(v) => id = Some(v),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--talent-root" => match next_value(args, &mut i, "--talent-root") {
                Ok(v) => talent_root = Some(PathBuf::from(v)),
                Err(e) => return ExperiencesCmd::Usage(e),
            },
            "--json" => json = true,
            other => return ExperiencesCmd::Usage(format!("unrecognized argument `{other}`")),
        }
        i += 1;
    }
    let (Some(day), Some(facet), Some(id)) = (day, facet, id) else {
        return ExperiencesCmd::Usage("recompose requires --day, --facet, and --id".to_owned());
    };
    ExperiencesCmd::Recompose {
        journal,
        day,
        facet,
        id,
        talent_root,
        json,
    }
}

fn now_utc() -> String {
    solstone_core_time::utc_rfc3339(jiff::Timestamp::now())
}

fn today_owner_day(journal: &Path) -> String {
    let zone = solstone_core_journal_config::owner_zone(journal);
    let now = jiff::Timestamp::now().to_zoned_datetime(zone);
    solstone_core_time::day_key(now.date())
}

fn format_json(value: &Value) -> String {
    serde_json::to_string_pretty(value)
        .map(|mut s| {
            s.push('\n');
            s
        })
        .unwrap_or_else(|_| "{}\n".to_owned())
}

fn format_experience_summary_line(rec: &ExperienceRecord) -> String {
    let muted = if rec.hidden { " [muted]" } else { "" };
    let title = if rec.title.trim().is_empty() {
        "(untitled)"
    } else {
        rec.title.as_str()
    };
    format!(
        "{}  {}  {}  {}..{}  ({} acts, {} moments){}  {}",
        rec.anchor_day,
        rec.primary_facet,
        rec.id,
        rec.started_at,
        rec.ended_at,
        rec.activities.len(),
        rec.moments.len(),
        muted,
        title
    )
}

/// Execute a parsed `ExperiencesCmd` against `journal`.
pub fn execute(
    cmd: ExperiencesCmd,
    journal: &Path,
    generate: Option<&dyn GenerateExecutor>,
    talent_paths: Option<(&Path, &Path)>,
) -> CliRun {
    match cmd {
        ExperiencesCmd::Help(text) => CliRun::ok(text),
        ExperiencesCmd::Usage(err) => CliRun::usage(err),
        ExperiencesCmd::List {
            day,
            from,
            to,
            facet,
            include_hidden,
            json: as_json,
            ..
        } => {
            let facets: Vec<String> = match facet.as_ref() {
                Some(slug) => vec![slug.clone()],
                None => list_declared_facet_names(journal).unwrap_or_default(),
            };
            let mut rows = Vec::new();
            let (label_from, label_to) = if let Some(d) = day {
                for f in &facets {
                    if let Ok(found) =
                        load_experiences_overlapping_day(journal, f, &d, include_hidden)
                    {
                        rows.extend(found);
                    }
                }
                (d.clone(), d)
            } else {
                let start = from.unwrap_or_else(|| today_owner_day(journal));
                let end = to.unwrap_or_else(|| start.clone());
                for f in &facets {
                    if let Ok(found) =
                        list_experiences_in_range(journal, f, &start, &end, include_hidden)
                    {
                        rows.extend(found);
                    }
                }
                (start, end)
            };
            rows.sort_by(|a, b| {
                a.started_at
                    .cmp(&b.started_at)
                    .then_with(|| a.id.cmp(&b.id))
            });
            if as_json {
                let hydrated: Vec<_> = rows
                    .into_iter()
                    .map(|r| hydrate_experience(journal, r, None))
                    .collect();
                let payload = json!({
                    "from": label_from,
                    "to": label_to,
                    "facet": facet,
                    "include_hidden": include_hidden,
                    "experiences": hydrated,
                });
                CliRun::ok(format_json(&payload))
            } else if rows.is_empty() {
                CliRun::ok(format!(
                    "No experiences found for {label_from}..{label_to}.\n"
                ))
            } else {
                let mut out = String::new();
                for rec in &rows {
                    out.push_str(&format_experience_summary_line(rec));
                    out.push('\n');
                }
                CliRun::ok(out)
            }
        }
        ExperiencesCmd::Show {
            day,
            facet,
            id,
            share_tier,
            include_cross_facet_candidates,
            json: as_json,
            ..
        } => {
            let record = match get_experience(journal, &facet, &day, &id) {
                Ok(Some(rec)) => rec,
                Ok(None) => {
                    return CliRun::fail(1, format!("experience not found: {facet}/{day}/{id}"));
                }
                Err(e) => return CliRun::fail(1, format!("failed to read experience: {e}")),
            };
            let hydrated =
                hydrate_experience(journal, record, Some(include_cross_facet_candidates));
            if let Some(policy) = share_tier {
                let projected = project_experience_for_share(&hydrated, policy);
                if as_json {
                    CliRun::ok(format_json(&json!({ "preview": projected })))
                } else {
                    let mut out = format!(
                        "[{}] {} ({}..{})\n{}\n",
                        projected.tier,
                        projected.title,
                        projected.started_at,
                        projected.ended_at,
                        projected.summary
                    );
                    if let Some(moments) = &projected.moments {
                        for m in moments {
                            out.push_str(&format!(
                                "  - {}..{}: {}\n",
                                m.started_at, m.ended_at, m.summary
                            ));
                        }
                    }
                    CliRun::ok(out)
                }
            } else if as_json {
                CliRun::ok(format_json(&json!({ "experience": hydrated })))
            } else {
                let mut out = format!(
                    "{}\n{}\n",
                    format_experience_summary_line(&hydrated.experience),
                    hydrated.experience.summary
                );
                if !hydrated.experience.locations.is_empty() {
                    out.push_str(&format!(
                        "Locations: {}\n",
                        hydrated.experience.locations.join(", ")
                    ));
                }
                if !hydrated.hydrated.people.is_empty() {
                    let names: Vec<&str> = hydrated
                        .hydrated
                        .people
                        .iter()
                        .map(|p| p.name.as_str())
                        .collect();
                    out.push_str(&format!("People: {}\n", names.join(", ")));
                }
                for m in &hydrated.experience.moments {
                    out.push_str(&format!(
                        "  - [{}] {}..{}: {}\n",
                        m.id, m.started_at, m.ended_at, m.summary
                    ));
                }
                CliRun::ok(out)
            }
        }
        ExperiencesCmd::Create {
            facet,
            activities,
            title,
            summary,
            json: as_json,
            ..
        } => {
            let now = now_utc();
            let created = match create_owner_experience(
                journal, &facet, activities, title, summary, "owner", &now,
            ) {
                Ok(rec) => rec,
                Err(e) => return CliRun::fail(1, format!("create experience failed: {e}")),
            };
            let asked = ask_experience_compose(
                journal,
                &facet,
                &created.anchor_day,
                &created.id,
                Some("owner_create"),
            )
            .is_ok();
            let hydrated = hydrate_experience(journal, created, None);
            if as_json {
                CliRun::ok(format_json(&json!({
                    "experience": hydrated,
                    "asked": asked,
                })))
            } else {
                CliRun::ok(format!(
                    "Created {}\n",
                    format_experience_summary_line(&hydrated.experience)
                ))
            }
        }
        ExperiencesCmd::Update {
            day,
            facet,
            id,
            title,
            summary,
            locations,
            moments_json,
            release_holds,
            note,
            json: as_json,
            ..
        } => {
            let moments: Option<Vec<ExperienceMoment>> = match moments_json {
                Some(raw) => match serde_json::from_str(&raw) {
                    Ok(m) => Some(m),
                    Err(e) => return CliRun::usage(format!("invalid --moments-json: {e}")),
                },
                None => None,
            };
            let released_any = !release_holds.is_empty();
            let patch = ExperienceDirectPatch {
                title,
                summary,
                locations,
                moments,
                release_holds,
                note,
            };
            let now = now_utc();
            let updated =
                match update_experience_direct(journal, &facet, &day, &id, patch, "owner", &now) {
                    Ok(rec) => rec,
                    Err(e) => return CliRun::fail(1, format!("update experience failed: {e}")),
                };
            let asked = if released_any {
                ask_experience_compose(
                    journal,
                    &facet,
                    &updated.anchor_day,
                    &updated.id,
                    Some("owner_release_hold"),
                )
                .is_ok()
            } else {
                false
            };
            let hydrated = hydrate_experience(journal, updated, None);
            if as_json {
                CliRun::ok(format_json(&json!({
                    "experience": hydrated,
                    "asked": asked,
                })))
            } else {
                CliRun::ok(format!(
                    "Updated {}\n",
                    format_experience_summary_line(&hydrated.experience)
                ))
            }
        }
        ExperiencesCmd::AddActivity {
            day,
            facet,
            id,
            activity,
            note,
            json: as_json,
            ..
        } => {
            let now = now_utc();
            let mutation = ExperienceActivitiesMutation {
                add: vec![activity],
                remove: Vec::new(),
                note,
            };
            let updated = match mutate_experience_activities(
                journal, &facet, &day, &id, mutation, "owner", &now,
            ) {
                Ok(rec) => rec,
                Err(e) => return CliRun::fail(1, format!("add-activity failed: {e}")),
            };
            let asked = ask_experience_compose(
                journal,
                &facet,
                &updated.anchor_day,
                &updated.id,
                Some("owner_add_activity"),
            )
            .is_ok();
            let hydrated = hydrate_experience(journal, updated, None);
            if as_json {
                CliRun::ok(format_json(&json!({
                    "experience": hydrated,
                    "asked": asked,
                })))
            } else {
                CliRun::ok(format!(
                    "Updated {}\n",
                    format_experience_summary_line(&hydrated.experience)
                ))
            }
        }
        ExperiencesCmd::RemoveActivity {
            day,
            facet,
            id,
            activity,
            note,
            json: as_json,
            ..
        } => {
            let now = now_utc();
            let mutation = ExperienceActivitiesMutation {
                add: Vec::new(),
                remove: vec![activity],
                note,
            };
            let updated = match mutate_experience_activities(
                journal, &facet, &day, &id, mutation, "owner", &now,
            ) {
                Ok(rec) => rec,
                Err(e) => return CliRun::fail(1, format!("remove-activity failed: {e}")),
            };
            let asked = if !updated.activities.is_empty() {
                ask_experience_compose(
                    journal,
                    &facet,
                    &updated.anchor_day,
                    &updated.id,
                    Some("owner_remove_activity"),
                )
                .is_ok()
            } else {
                false
            };
            let hydrated = hydrate_experience(journal, updated, None);
            if as_json {
                CliRun::ok(format_json(&json!({
                    "experience": hydrated,
                    "asked": asked,
                })))
            } else {
                CliRun::ok(format!(
                    "Updated {}\n",
                    format_experience_summary_line(&hydrated.experience)
                ))
            }
        }
        ExperiencesCmd::Mute {
            day,
            facet,
            id,
            hidden,
            note,
            json: as_json,
            ..
        } => {
            let now = now_utc();
            let updated = match set_experience_hidden(
                journal,
                &facet,
                &day,
                &id,
                hidden,
                "owner",
                note.as_deref(),
                &now,
            ) {
                Ok(rec) => rec,
                Err(e) => {
                    let verb = if hidden { "mute" } else { "unmute" };
                    return CliRun::fail(1, format!("{verb} failed: {e}"));
                }
            };
            let hydrated = hydrate_experience(journal, updated, None);
            if as_json {
                CliRun::ok(format_json(&json!({ "experience": hydrated })))
            } else {
                let verb = if hidden { "Muted" } else { "Unmuted" };
                CliRun::ok(format!(
                    "{verb} {}\n",
                    format_experience_summary_line(&hydrated.experience)
                ))
            }
        }
        ExperiencesCmd::Detect {
            day,
            facets,
            compose,
            json: as_json,
            ..
        } => {
            let Some(generate) = generate else {
                return CliRun::fail(75, "generate client unavailable for detect");
            };
            let Some((talent_root, apps_root)) = talent_paths else {
                return CliRun::fail(75, "talent root unavailable for detect");
            };
            let filter = if facets.is_empty() {
                None
            } else {
                Some(facets.as_slice())
            };
            match run_detect_day(
                journal,
                &day,
                filter,
                compose,
                generate,
                talent_root,
                apps_root,
            ) {
                Ok(result) => {
                    if as_json {
                        CliRun::ok(format_json(&json!(result)))
                    } else {
                        CliRun::ok(format!(
                            "Detected {} experiences on {} (stage1a={}, composed={})\n",
                            result.committed.len(),
                            result.day,
                            result.stage1a_candidates,
                            result.composed.len()
                        ))
                    }
                }
                Err(e) => CliRun::fail(1, format!("detect failed: {e}")),
            }
        }
        ExperiencesCmd::Recompose {
            day,
            facet,
            id,
            json: as_json,
            ..
        } => {
            let Some(generate) = generate else {
                return CliRun::fail(75, "generate client unavailable for recompose");
            };
            let Some((talent_root, apps_root)) = talent_paths else {
                return CliRun::fail(75, "talent root unavailable for recompose");
            };
            match run_compose_experience(
                journal,
                &facet,
                &day,
                &id,
                generate,
                talent_root,
                apps_root,
            ) {
                Ok(result) => {
                    if as_json {
                        CliRun::ok(format_json(&json!(result)))
                    } else {
                        CliRun::ok(format!(
                            "Recomposed {} (retried={}, repaired={})\n",
                            format_experience_summary_line(&result.experience),
                            result.retried,
                            result.repaired
                        ))
                    }
                }
                Err(e) => CliRun::fail(1, format!("recompose failed: {e}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solstone_core_facets::{append_activity_record, create_facet};
    use tempfile::TempDir;

    fn os(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn cli_help_and_usage_errors_never_touch_journal() {
        let help = parse(&os(&["--help"]));
        assert!(!help.needs_journal());
        assert!(matches!(help, ExperiencesCmd::Help(_)));

        let bad = parse(&os(&["--nonsense"]));
        assert!(!bad.needs_journal());
        let run = execute(bad, Path::new(""), None, None);
        assert_eq!(run.exit_code, 2);
        assert!(run.stderr.starts_with(EXPERIENCES_USAGE));
    }

    #[test]
    fn cli_full_crud_cross_facet_and_share_tier_lifecycle() {
        let dir = TempDir::new().unwrap();
        create_facet(
            dir.path(),
            "personal",
            "Personal",
            "",
            "#6366f1",
            "✨",
            None,
        )
        .unwrap();
        create_facet(dir.path(), "work", "Work", "", "#0ea5e9", "💼", None).unwrap();

        let mut act1 = serde_json::Map::new();
        act1.insert("id".to_owned(), json!("social_180000_600"));
        act1.insert("stream".to_owned(), json!("watch"));
        act1.insert("activity".to_owned(), json!("social"));
        act1.insert("description".to_owned(), json!("Dinner"));
        act1.insert("segments".to_owned(), json!(["180000_600"]));
        let _ = append_activity_record(dir.path(), "personal", "20260304", act1).unwrap();

        let mut act2 = serde_json::Map::new();
        act2.insert("id".to_owned(), json!("meeting_183000_300"));
        act2.insert("stream".to_owned(), json!("watch"));
        act2.insert("activity".to_owned(), json!("meeting"));
        act2.insert("description".to_owned(), json!("Work call"));
        act2.insert("segments".to_owned(), json!(["183000_300"]));
        let _ = append_activity_record(dir.path(), "work", "20260304", act2).unwrap();

        // Create
        let run = execute(
            parse(&os(&[
                "create",
                "--facet",
                "personal",
                "--activity",
                "20260304:watch:social_180000_600",
                "--title",
                "Dinner out",
                "--summary",
                "Alex had dinner.",
                "--json",
            ])),
            dir.path(),
            None,
            None,
        );
        assert_eq!(run.exit_code, 0, "stderr: {}", run.stderr);
        let created: Value = serde_json::from_str(&run.stdout).unwrap();
        let exp_id = created["experience"]["id"].as_str().unwrap().to_owned();

        // Cross-facet add-activity
        let run = execute(
            parse(&os(&[
                "add-activity",
                "--day",
                "20260304",
                "--facet",
                "personal",
                "--id",
                &exp_id,
                "--activity",
                "20260304:watch:meeting_183000_300",
                "--activity-facet",
                "work",
                "--json",
            ])),
            dir.path(),
            None,
            None,
        );
        assert_eq!(run.exit_code, 0, "stderr: {}", run.stderr);
        let added: Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(
            added["experience"]["activities"].as_array().unwrap().len(),
            2
        );

        // Update + share-tier preview
        let run = execute(
            parse(&os(&[
                "update",
                "--day",
                "20260304",
                "--facet",
                "personal",
                "--id",
                &exp_id,
                "--title",
                "Evening out",
                "--location",
                "Riverside Cafe",
                "--json",
            ])),
            dir.path(),
            None,
            None,
        );
        assert_eq!(run.exit_code, 0, "stderr: {}", run.stderr);

        let run = execute(
            parse(&os(&[
                "show",
                "--day",
                "20260304",
                "--facet",
                "personal",
                "--id",
                &exp_id,
                "--share-tier",
                "overview",
                "--json",
            ])),
            dir.path(),
            None,
            None,
        );
        assert_eq!(run.exit_code, 0, "stderr: {}", run.stderr);
        let preview: Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(preview["preview"]["policy"], "overview_only");
        assert_eq!(preview["preview"]["title"], "Evening out");

        // Mute & verify hidden from default list, visible with --include-hidden, then unmute
        let run = execute(
            parse(&os(&[
                "mute", "--day", "20260304", "--facet", "personal", "--id", &exp_id, "--json",
            ])),
            dir.path(),
            None,
            None,
        );
        assert_eq!(run.exit_code, 0);

        let run = execute(
            parse(&os(&["list", "--day", "20260304", "--json"])),
            dir.path(),
            None,
            None,
        );
        assert_eq!(run.exit_code, 0);
        let listed: Value = serde_json::from_str(&run.stdout).unwrap();
        assert!(listed["experiences"].as_array().unwrap().is_empty());

        let run = execute(
            parse(&os(&[
                "list",
                "--day",
                "20260304",
                "--include-hidden",
                "--json",
            ])),
            dir.path(),
            None,
            None,
        );
        let listed_all: Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(listed_all["experiences"].as_array().unwrap().len(), 1);

        let run = execute(
            parse(&os(&[
                "unmute", "--day", "20260304", "--facet", "personal", "--id", &exp_id, "--json",
            ])),
            dir.path(),
            None,
            None,
        );
        assert_eq!(run.exit_code, 0);

        // Remove activity
        let run = execute(
            parse(&os(&[
                "remove-activity",
                "--day",
                "20260304",
                "--facet",
                "personal",
                "--id",
                &exp_id,
                "--activity",
                "20260304:watch:meeting_183000_300",
                "--activity-facet",
                "work",
                "--json",
            ])),
            dir.path(),
            None,
            None,
        );
        assert_eq!(run.exit_code, 0);
        let removed: Value = serde_json::from_str(&run.stdout).unwrap();
        assert_eq!(
            removed["experience"]["activities"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
}
