// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The order shared by task selection and model admission.

use std::cmp::Reverse;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

pub const WORK_PRIORITY_ENV: &str = "SOLSTONE_WORK_PRIORITY";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum WorkBand {
    Now = 1,
    Waiting = 2,
    Recent = 3,
    Import = 4,
    Completing = 5,
    Remake = 6,
}

impl WorkBand {
    /// Imports and history (bands 4-6) run behind newer work, never ahead of it.
    pub fn is_background(self) -> bool {
        self >= Self::Import
    }
}

/// Source time, not enqueue time, decides newest-first order within a band.
/// The key is YYYYMMDDHHMMSS; zero means no dated source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkPriority {
    pub band: WorkBand,
    pub source_time: u64,
}

impl Default for WorkPriority {
    fn default() -> Self {
        Self {
            band: WorkBand::Now,
            source_time: 0,
        }
    }
}

impl WorkPriority {
    pub fn sort_key(self) -> (WorkBand, Reverse<u64>) {
        (self.band, Reverse(self.source_time))
    }

    pub fn from_environment() -> Result<Self, String> {
        Self::from_environment_in(solstone_core_cli_copy::ENGLISH)
    }
    pub fn from_environment_in(locale: &str) -> Result<Self, String> {
        match std::env::var(WORK_PRIORITY_ENV) {
            Ok(value) => Self::parse_in(locale, &value),
            Err(std::env::VarError::NotPresent) => Ok(Self::default()),
            Err(error) => Err(solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.think.priority.environment",
                [("error", &(error))],
            )),
        }
    }
}

impl fmt::Display for WorkPriority {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{}:{}", self.band as u8, self.source_time)
    }
}

impl FromStr for WorkPriority {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse_in(solstone_core_cli_copy::ENGLISH, value)
    }
}
impl WorkPriority {
    fn parse_in(locale: &str, value: &str) -> Result<Self, String> {
        let invalid =
            || solstone_core_cli_copy::render_for(locale, "journal.cli.think.priority.syntax", &[]);
        let (band, source) = value.split_once(':').ok_or_else(invalid)?;
        let band = match band {
            "1" => WorkBand::Now,
            "2" => WorkBand::Waiting,
            "3" => WorkBand::Recent,
            "4" => WorkBand::Import,
            "5" => WorkBand::Completing,
            "6" => WorkBand::Remake,
            _ => return Err(invalid()),
        };
        if source.is_empty() || !source.bytes().all(|c| c.is_ascii_digit()) {
            return Err(invalid());
        }
        let source_time = source.parse().map_err(|_| invalid())?;
        Ok(Self { band, source_time })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_orders_bands_then_newest_source() {
        let mut priorities = [
            "6:20261011120000",
            "1:20261011010000",
            "2:20261010000000",
            "1:20261011120000",
        ]
        .map(|value| value.parse::<WorkPriority>().unwrap());
        priorities.sort_by_key(|priority| priority.sort_key());
        assert_eq!(
            priorities.map(|priority| priority.to_string()),
            [
                "1:20261011120000",
                "1:20261011010000",
                "2:20261010000000",
                "6:20261011120000"
            ]
        );
    }

    #[test]
    fn priority_round_trips_and_rejects_invalid_input() {
        for band in 1..=6 {
            let value = format!("{band}:20261011120000");
            assert_eq!(value.parse::<WorkPriority>().unwrap().to_string(), value);
        }
        for value in [
            "",
            "0:0",
            "7:0",
            "1:-1",
            "1:",
            "1:1:2",
            "1:18446744073709551616",
            "1:+1",
        ] {
            assert!(value.parse::<WorkPriority>().is_err(), "{value}");
        }
    }
}
