// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host-native import adapters, including FFmpeg-backed audio operations.

pub mod audio;
pub mod audio_archive;
pub mod audio_custody;
pub mod audio_publication;
mod body_activity;
pub mod body_segments;
pub mod call;
pub mod cli_argv;
pub mod import_publication;
pub mod oura_coverage;
mod oura_fetch;
#[cfg(feature = "in-process-pdf")]
pub mod pdf_in_process;
pub mod registry;
pub mod strava_convert;
mod strava_decision;
mod strava_filing;
mod strava_names;
mod strava_summary;

#[cfg(all(test, feature = "full-tests"))]
mod embedded_admission;
#[cfg(test)]
mod folder_sources;
