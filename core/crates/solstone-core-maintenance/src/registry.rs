// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/// How often a maintenance routine is scheduled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    Hourly,
    Daily,
    Weekly,
}

impl Cadence {
    /// The schedule-config interval spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Hourly => "hourly",
            Self::Daily => "daily",
            Self::Weekly => "weekly",
        }
    }
}

/// One maintenance routine owned by the native registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoutineDescriptor {
    pub id: &'static str,
    pub description: &'static str,
    pub cadence: Cadence,
    pub max_runtime: Option<&'static str>,
    pub args: &'static [&'static str],
}

const ROUTINES: [RoutineDescriptor; 10] = [
    RoutineDescriptor {
        id: "speakers:consolidate-pool",
        description: "Consolidate dense speaker candidates.",
        cadence: Cadence::Daily,
        max_runtime: Some("10m"),
        args: &[],
    },
    RoutineDescriptor {
        id: "backup:run",
        description: "run encrypted backup.",
        cadence: Cadence::Hourly,
        max_runtime: Some("49h"),
        args: &[],
    },
    RoutineDescriptor {
        id: "backup:prune",
        description: "apply encrypted backup retention policy.",
        cadence: Cadence::Daily,
        max_runtime: Some("3h"),
        args: &[],
    },
    RoutineDescriptor {
        id: "backup:verify",
        description: "verify encrypted backup read-back.",
        cadence: Cadence::Weekly,
        max_runtime: Some("90m"),
        args: &[],
    },
    RoutineDescriptor {
        id: "backup:offload",
        description: "offload verified raw media after backup.",
        cadence: Cadence::Daily,
        max_runtime: Some("7h"),
        args: &[],
    },
    RoutineDescriptor {
        id: "health:mark-raw",
        description: "list original media ready for removal.",
        cadence: Cadence::Daily,
        max_runtime: Some("60m"),
        args: &[],
    },
    RoutineDescriptor {
        id: "health:prune-logs",
        description: "prune old operational logs.",
        cadence: Cadence::Daily,
        max_runtime: Some("30m"),
        args: &[],
    },
    RoutineDescriptor {
        id: "timeline:rollup",
        description: "Roll segment timelines through the journal master timeline.",
        cadence: Cadence::Daily,
        max_runtime: Some("60m"),
        args: &["--commit"],
    },
    RoutineDescriptor {
        id: "timeline:rollup-day",
        description: "Roll segment timelines up into one day timeline.",
        cadence: Cadence::Daily,
        max_runtime: Some("30m"),
        args: &[],
    },
    RoutineDescriptor {
        id: "timeline:rollup-master",
        description: "Roll day timelines up into the journal master timeline.",
        cadence: Cadence::Daily,
        max_runtime: Some("30m"),
        args: &[],
    },
];

/// All routines in the fixed native maintenance census.
pub const fn routines() -> &'static [RoutineDescriptor] {
    &ROUTINES
}

/// Look up a routine by its stable `app:routine` identifier.
pub fn routine(id: &str) -> Option<&'static RoutineDescriptor> {
    routines().iter().find(|descriptor| descriptor.id == id)
}

#[cfg(test)]
fn validate_census(routines: &[RoutineDescriptor]) -> Result<(), String> {
    const EXPECTED: [(&str, Cadence, Option<&str>, &[&str]); 10] = [
        (
            "speakers:consolidate-pool",
            Cadence::Daily,
            Some("10m"),
            &[],
        ),
        ("backup:run", Cadence::Hourly, Some("49h"), &[]),
        ("backup:prune", Cadence::Daily, Some("3h"), &[]),
        ("backup:verify", Cadence::Weekly, Some("90m"), &[]),
        ("backup:offload", Cadence::Daily, Some("7h"), &[]),
        ("health:mark-raw", Cadence::Daily, Some("60m"), &[]),
        ("health:prune-logs", Cadence::Daily, Some("30m"), &[]),
        (
            "timeline:rollup",
            Cadence::Daily,
            Some("60m"),
            &["--commit"],
        ),
        ("timeline:rollup-day", Cadence::Daily, Some("30m"), &[]),
        ("timeline:rollup-master", Cadence::Daily, Some("30m"), &[]),
    ];
    if routines.len() != EXPECTED.len() {
        return Err(format!(
            "routine count expected {}, got {}",
            EXPECTED.len(),
            routines.len()
        ));
    }
    for (id, cadence, max_runtime, args) in EXPECTED {
        let matches = routines
            .iter()
            .filter(|descriptor| descriptor.id == id)
            .collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(format!("routine {id} expected once, got {}", matches.len()));
        }
        let descriptor = matches[0];
        if descriptor.cadence != cadence {
            return Err(format!("routine {id} cadence does not match"));
        }
        if descriptor.max_runtime != max_runtime {
            return Err(format!("routine {id} max_runtime does not match"));
        }
        if descriptor.args != args {
            return Err(format!("routine {id} args do not match"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Cadence, RoutineDescriptor, routine, routines, validate_census};
    use std::collections::BTreeSet;

    #[test]
    fn census_has_ten_unique_well_formed_ids() {
        let all = routines();
        assert_eq!(all.len(), 10);
        let ids = all
            .iter()
            .map(|descriptor| descriptor.id)
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), all.len());
        for id in ids {
            let Some((app, action)) = id.split_once(':') else {
                panic!("missing app:routine separator: {id}");
            };
            assert!(is_slug(app), "invalid app component: {app}");
            assert!(is_slug(action), "invalid routine component: {action}");
            assert!(routine(id).is_some());
        }
    }

    #[test]
    fn descriptions_are_nonblank_without_freezing_their_prose() {
        assert!(
            routines()
                .iter()
                .all(|descriptor| !descriptor.description.trim().is_empty())
        );
    }

    #[test]
    fn census_matches_the_independently_pinned_schedule_contract() {
        validate_census(routines()).expect("fixed census");
    }

    #[test]
    fn census_validation_rejects_a_mutated_registry() {
        let mut missing = routines().to_vec();
        missing.retain(|descriptor| descriptor.id != "health:mark-raw");
        let error = validate_census(&missing).expect_err("missing routine must fail");
        assert!(error.contains("count"));

        let mut wrong_cap = routines().to_vec();
        *wrong_cap
            .iter_mut()
            .find(|routine| routine.id == "backup:run")
            .unwrap() = RoutineDescriptor {
            id: "backup:run",
            description: "different wording remains allowed",
            cadence: Cadence::Hourly,
            max_runtime: Some("1m"),
            args: &[],
        };
        let error = validate_census(&wrong_cap).expect_err("wrong cap must fail");
        assert!(error.contains("backup:run") && error.contains("max_runtime"));
    }

    fn is_slug(value: &str) -> bool {
        let mut chars = value.chars();
        matches!(chars.next(), Some('a'..='z'))
            && chars.all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '_' | '-')
            })
    }
}
