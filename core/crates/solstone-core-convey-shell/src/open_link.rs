// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! `GET /app/open/<t>`: a journal link opens its place.
//!
//! `<t>` is `<sealed>` or `<sealed>~HHMMSS`. The journal opens the sealed
//! part with its own key, looks the place up, and answers a relative
//! redirect to that place's page, or a page that says why not.
//!
//! ⛔ A link is a pointer and never a credential. This route sits behind the
//! same gates as every other dashboard route and is exempt from none of
//! them; it writes nothing; and it resolves the opened place by lookup
//! against what the journal lists, never by joining its parts into a path.
//! The redirect is built only from the place that was found and the
//! validated moment.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Extension, Path as RoutePath};
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::get;
use chrono::{DateTime, NaiveDate, Utc};
use solstone_core_convey_http::cant_open::{LinkPageAction, link_cant_open_response};
use solstone_core_facets::{
    FacetIdResolveError, RetiredFacets, activity_value_truthy, get_activity_record,
    list_activity_day_stems, list_declared_facet_names, load_activity_records, read_retired_facets,
    resolve_facet_id, retired_facets_path,
};
use solstone_core_format::segment::segment_start_and_end_seconds;
use solstone_core_journal_io::paths::{
    DEFAULT_STREAM, PathOrDay, iter_segments_skipping_unreadable_streams,
};
use solstone_core_journal_link::{OpenError, Place};
use solstone_core_segment::{OwnerDeletionState, owner_deletion_state};
use solstone_core_transcripts_web::day_segment_list;

use crate::source_link::{encode_path_component, is_safe_location, redirect_response};

/// How far a chain of merges and renames is followed before giving up.
const SUCCESSOR_HOPS: usize = 16;

#[derive(Clone)]
struct LinkJournal(Arc<PathBuf>);

pub fn open_link_router(journal: PathBuf) -> Router {
    Router::new()
        .route("/app/open/{link}", get(handle_open_link))
        .layer(Extension(LinkJournal(Arc::new(journal))))
}

async fn handle_open_link(
    RoutePath(link): RoutePath<String>,
    Extension(LinkJournal(journal)): Extension<LinkJournal>,
) -> Response {
    solstone_core_convey_http::owner_read::spawn_blocking_response(
        solstone_core_convey_http::owner_read::OwnerReadRole::SourceLink,
        move || open_link(&journal, &link, Utc::now()),
    )
    .await
}

fn open_link(journal: &Path, link: &str, now: DateTime<Utc>) -> Response {
    // Base64url has no `~`, so the last one, if any, starts the moment.
    let (sealed, moment) = match link.rsplit_once('~') {
        Some((sealed, moment)) => (sealed, parse_moment(moment)),
        None => (link, None),
    };
    let place = match solstone_core_journal_link::open(journal, sealed) {
        Ok(place) => place,
        // Another journal's link, an altered one and text that was never a
        // link can't be told apart, and the page doesn't pretend to.
        Err(OpenError::NotOpenedHere) => {
            return link_cant_open_response(
                StatusCode::NOT_FOUND,
                "this link can't be opened here.",
                Some("it may be for another journal, or the link may have changed."),
                LinkPageAction::OpenJournal,
            );
        }
        Err(OpenError::UnknownPlace) => return cant_show(),
        Err(OpenError::KeyUnusable) => {
            log::error!("open_link: the journal's link key couldn't be read");
            return couldnt_check(sealed, moment);
        }
    };
    match resolve_place(journal, &place, moment, now) {
        Ok(Landing::Redirect(location)) => redirect_response(&location),
        // Absent, deleted and hidden read the same, and say nothing of the
        // place.
        Ok(Landing::Absent) => link_cant_open_response(
            StatusCode::NOT_FOUND,
            "it isn't in your journal.",
            None,
            LinkPageAction::OpenJournal,
        ),
        Ok(Landing::WontFollow) => link_cant_open_response(
            StatusCode::BAD_REQUEST,
            "your journal won't follow this link.",
            None,
            LinkPageAction::OpenJournal,
        ),
        Err(error) => {
            log::error!("open_link: couldn't check: {error}");
            couldnt_check(sealed, moment)
        }
    }
}

fn cant_show() -> Response {
    link_cant_open_response(
        StatusCode::NOT_FOUND,
        "your journal can't show this kind of source.",
        None,
        LinkPageAction::OpenJournal,
    )
}

/// "try again" repeats the link, rebuilt from what was checked: the sealed
/// text reached the key only as base64url, and the moment is six digits.
fn couldnt_check(sealed: &str, moment: Option<Moment>) -> Response {
    let mut again = format!("/app/open/{sealed}");
    if let Some(moment) = moment {
        again.push('~');
        again.push_str(&moment.digits());
    }
    link_cant_open_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "your journal couldn't check this source.",
        None,
        LinkPageAction::TryAgain(&again),
    )
}

enum Landing {
    Redirect(String),
    Absent,
    WontFollow,
}

/// A time of day, to the second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Moment(u32);

impl Moment {
    fn digits(self) -> String {
        format!(
            "{:02}{:02}{:02}",
            self.0 / 3_600,
            self.0 / 60 % 60,
            self.0 % 60
        )
    }
}

/// Exactly six digits that are a time of day, or no moment at all.
fn parse_moment(text: &str) -> Option<Moment> {
    if text.len() != 6 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let part = |index: usize| text[index..index + 2].parse::<u32>().ok();
    let (hour, minute, second) = (part(0)?, part(2)?, part(4)?);
    (hour < 24 && minute < 60 && second < 60).then_some(Moment(hour * 3_600 + minute * 60 + second))
}

fn is_calendar_day(day: &str) -> bool {
    day.len() == 8
        && day.bytes().all(|byte| byte.is_ascii_digit())
        && NaiveDate::parse_from_str(day, "%Y%m%d").is_ok()
}

fn redirect(location: String) -> Landing {
    if is_safe_location(&location) {
        Landing::Redirect(location)
    } else {
        Landing::WontFollow
    }
}

fn resolve_place(
    journal: &Path,
    place: &Place,
    moment: Option<Moment>,
    now: DateTime<Utc>,
) -> Result<Landing, String> {
    // A moment belongs to a recording. On any other place it is dropped.
    match place {
        Place::Day { day } => resolve_day(journal, day),
        Place::Recording {
            day,
            stream,
            segment,
        } => resolve_recording(journal, day, stream, segment, moment, now),
        Place::Activity { facet, day, id } => resolve_activity(journal, facet, day, id),
        Place::Person { id } => resolve_person(journal, id),
    }
}

/// A day opens on what happened in it: its activities when any facet has
/// one to show, otherwise its recordings. A day whose activities are all
/// hidden is no more there than a day with none.
fn resolve_day(journal: &Path, day: &str) -> Result<Landing, String> {
    if !is_calendar_day(day) {
        return Ok(Landing::Absent);
    }
    for facet in list_declared_facet_names(journal).map_err(|error| error.to_string())? {
        if list_activity_day_stems(journal, &facet)
            .map_err(|error| error.to_string())?
            .iter()
            .any(|stem| stem == day)
            && !load_activity_records(journal, &facet, day, false)
                .map_err(|error| error.to_string())?
                .is_empty()
        {
            return Ok(redirect(format!("/app/activities/{day}")));
        }
    }
    if journal.join("chronicle").join(day).is_dir() {
        return Ok(redirect(format!("/app/transcripts/{day}")));
    }
    Ok(Landing::Absent)
}

fn resolve_recording(
    journal: &Path,
    day: &str,
    stream: &str,
    segment: &str,
    moment: Option<Moment>,
    now: DateTime<Utc>,
) -> Result<Landing, String> {
    if !is_calendar_day(day) || !journal.join("chronicle").join(day).is_dir() {
        return Ok(Landing::Absent);
    }
    // The recording is found among the day's own listing of its segments.
    let listed = iter_segments_skipping_unreadable_streams(journal, PathOrDay::Day(day))
        .map_err(|error| error.to_string())?;
    let Some(found) = listed.iter().find(|candidate| {
        candidate
            .record_identity()
            .is_ok_and(|identity| identity.stream == stream && identity.name == segment)
    }) else {
        return Ok(Landing::Absent);
    };
    // One the owner deleted is not in the journal, whatever is still on disk.
    if owner_deletion_state(found.path()).map_err(|error| error.to_string())?
        != OwnerDeletionState::Live
    {
        return Ok(Landing::Absent);
    }
    // And it opens only where the transcripts page itself lists it.
    let direct = stream == DEFAULT_STREAM;
    if !day_segment_list(journal, day, now)?
        .iter()
        .any(|shown| shown.key == segment && shown.direct == direct && shown.stream == stream)
    {
        return Ok(Landing::Absent);
    }
    let mut location = format!(
        "/app/transcripts/{}?stream={}#{}",
        encode_path_component(day),
        encode_path_component(stream),
        encode_path_component(segment)
    );
    // A moment outside the recording's own span is dropped, and the
    // recording opens without a cue.
    if let Some(moment) = moment
        && let Some((start, end)) = segment_start_and_end_seconds(segment)
    {
        let start =
            u32::from(start.hour) * 3_600 + u32::from(start.minute) * 60 + u32::from(start.second);
        if start <= moment.0 && u64::from(moment.0) <= end {
            location.push_str("/transcript/");
            location.push_str(&moment.digits());
        }
    }
    Ok(redirect(location))
}

/// The folder a facet name leads to today: its own, or the facet it was
/// renamed or merged into, through every recorded step.
fn live_facet(journal: &Path, name: &str) -> Result<Option<String>, String> {
    let declared = list_declared_facet_names(journal).map_err(|error| error.to_string())?;
    if declared.iter().any(|facet| facet == name) {
        return Ok(Some(name.to_owned()));
    }
    retired_facets_path(journal).map_err(|error| error.to_string())?;
    let retired = match read_retired_facets(journal) {
        RetiredFacets::Loaded(retired) => retired,
        RetiredFacets::Absent => return Ok(None),
        RetiredFacets::Malformed(message) | RetiredFacets::Unreadable(message) => {
            return Err(format!("retired facets unusable: {message}"));
        }
    };
    let Some(mut entry) = retired.get(name) else {
        return Ok(None);
    };
    for _ in 0..SUCCESSOR_HOPS {
        let Some(successor) = entry.successor.as_deref().filter(|_| entry.redirects()) else {
            return Ok(None);
        };
        match resolve_facet_id(journal, successor) {
            // The folder comes from the journal's own declarations.
            Ok(folder) => return Ok(Some(folder)),
            // The successor was itself retired later: follow its record.
            Err(FacetIdResolveError::Missing) => {
                // A record that names itself leads nowhere new.
                let Some(next) = retired.values().find(|candidate| {
                    candidate.id.as_deref() == Some(successor)
                        && candidate.successor.as_deref() != Some(successor)
                }) else {
                    return Ok(None);
                };
                entry = next;
            }
            Err(_) => return Err("a retired facet's successor couldn't be resolved".to_owned()),
        }
    }
    Ok(None)
}

fn resolve_activity(journal: &Path, facet: &str, day: &str, id: &str) -> Result<Landing, String> {
    if !is_calendar_day(day) {
        return Ok(Landing::Absent);
    }
    let Some(facet) = live_facet(journal, facet)? else {
        return Ok(Landing::Absent);
    };
    if !list_activity_day_stems(journal, &facet)
        .map_err(|error| error.to_string())?
        .iter()
        .any(|stem| stem == day)
    {
        return Ok(Landing::Absent);
    }
    // The store's own reader: no retired plan, raw body record or
    // superseded record is an activity.
    let Some(record) =
        get_activity_record(journal, &facet, day, id).map_err(|error| error.to_string())?
    else {
        return Ok(Landing::Absent);
    };
    if record.get("hidden").is_some_and(activity_value_truthy) {
        return Ok(Landing::Absent);
    }
    let facet = encode_path_component(&facet);
    Ok(redirect(format!(
        "/app/activities/{}?facet={facet}#{facet}/{}",
        encode_path_component(day),
        encode_path_component(id)
    )))
}

fn resolve_person(journal: &Path, id: &str) -> Result<Landing, String> {
    let map =
        solstone_core_entity::read_identity_map(journal).map_err(|error| error.to_string())?;
    let mut folder = map.resolved.get(id);
    if folder.is_none()
        && let Some(successor) = solstone_core_entity::merged_away(journal, id)?
        && let Some(live) = solstone_core_entity::live_merge_successor(journal, &successor)
    {
        folder = map.resolved.get(&live);
    }
    let Some(folder) = folder else {
        return Ok(Landing::Absent);
    };
    let Some(identity) = solstone_core_entity::read_entity_identity(journal, folder)
        .map_err(|error| error.to_string())?
    else {
        return Ok(Landing::Absent);
    };
    // One the owner hid opens no more than one that is gone.
    if solstone_core_entity::entity_is_hidden(identity.value()) {
        return Ok(Landing::Absent);
    }
    Ok(redirect(format!(
        "/app/entities/#{}",
        encode_path_component(folder)
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_is_exactly_six_digits_that_are_a_time() {
        assert_eq!(
            parse_moment("173012").map(Moment::digits).as_deref(),
            Some("173012")
        );
        assert_eq!(
            parse_moment("000000").map(Moment::digits).as_deref(),
            Some("000000")
        );
        assert_eq!(
            parse_moment("235959").map(Moment::digits).as_deref(),
            Some("235959")
        );
        for text in [
            "",
            "17301",
            "1730123",
            "240000",
            "176000",
            "173060",
            "17:30:",
            "+73012",
            "１７３０１２",
        ] {
            assert_eq!(parse_moment(text), None, "{text}");
        }
    }
}
