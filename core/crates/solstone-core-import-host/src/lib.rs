// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Host-native import adapters, including FFmpeg-backed audio operations.

pub mod audio;
pub mod audio_archive;
pub mod audio_publication;
pub mod call;
pub mod cli_argv;
pub mod import_publication;
#[cfg(feature = "in-process-pdf")]
pub mod pdf_in_process;
pub mod registry;

#[cfg(all(test, feature = "full-tests"))]
mod embedded_admission;
#[cfg(test)]
mod folder_sources;
