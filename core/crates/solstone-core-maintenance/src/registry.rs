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
}

struct Definition {
    id: &'static str,
    description_id: &'static str,
    cadence: Cadence,
    max_runtime: Option<&'static str>,
}

const DEFINITIONS: [Definition; 17] = [
    Definition {
        id: "faces:backfill",
        description_id: "journal.cli.maintenance.description.faces.backfill",
        cadence: Cadence::Hourly,
        max_runtime: Some("40m"),
    },
    Definition {
        id: "faces:reconcile",
        description_id: "journal.cli.maintenance.description.faces.backfill",
        cadence: Cadence::Daily,
        max_runtime: Some("10m"),
    },
    Definition {
        id: "speakers:discover-voices",
        description_id: "journal.cli.maintenance.description.speakers.discover.voices",
        cadence: Cadence::Daily,
        max_runtime: Some("10m"),
    },
    Definition {
        id: "speakers:candidate-pair-suggestions",
        description_id: "journal.cli.maintenance.description.speakers.candidate.pair.suggestions",
        cadence: Cadence::Daily,
        max_runtime: Some("10m"),
    },
    Definition {
        id: "speakers:name-variants",
        description_id: "journal.cli.maintenance.description.speakers.name.variants",
        cadence: Cadence::Daily,
        max_runtime: Some("10m"),
    },
    Definition {
        id: "speakers:consolidate-pool",
        description_id: "journal.cli.maintenance.description.speakers.consolidate.pool",
        cadence: Cadence::Daily,
        max_runtime: Some("10m"),
    },
    Definition {
        id: "speakers:backfill-pool",
        description_id: "journal.cli.maintenance.description.speakers.backfill.pool",
        cadence: Cadence::Daily,
        max_runtime: Some("10m"),
    },
    Definition {
        id: "speakers:backfill-labels",
        description_id: "journal.cli.maintenance.description.speakers.backfill.labels",
        cadence: Cadence::Daily,
        max_runtime: Some("30m"),
    },
    Definition {
        id: "backup:run",
        description_id: "journal.cli.maintenance.description.backup.run",
        cadence: Cadence::Hourly,
        max_runtime: Some("49h"),
    },
    Definition {
        id: "backup:prune",
        description_id: "journal.cli.maintenance.description.backup.prune",
        cadence: Cadence::Daily,
        max_runtime: Some("3h"),
    },
    Definition {
        id: "backup:erase",
        description_id: "journal.cli.maintenance.description.backup.erase",
        cadence: Cadence::Hourly,
        max_runtime: Some("7h"),
    },
    Definition {
        id: "backup:verify",
        description_id: "journal.cli.maintenance.description.backup.verify",
        cadence: Cadence::Weekly,
        max_runtime: Some("90m"),
    },
    Definition {
        id: "backup:offload",
        description_id: "journal.cli.maintenance.description.backup.offload",
        cadence: Cadence::Daily,
        max_runtime: Some("7h"),
    },
    Definition {
        id: "health:mark-raw",
        description_id: "journal.cli.maintenance.description.health.mark.raw",
        cadence: Cadence::Daily,
        max_runtime: Some("60m"),
    },
    Definition {
        id: "health:prune-logs",
        description_id: "journal.cli.maintenance.description.health.prune.logs",
        cadence: Cadence::Daily,
        max_runtime: Some("30m"),
    },
    Definition {
        id: "trash:erase",
        description_id: "journal.cli.maintenance.description.trash.erase",
        cadence: Cadence::Hourly,
        max_runtime: Some("30m"),
    },
    Definition {
        id: "retention:owner-voice-originals",
        description_id: "journal.cli.maintenance.description.retention.owner.voice.originals",
        cadence: Cadence::Daily,
        max_runtime: Some("30m"),
    },
];

// Keep the reference descriptor's public English field available to existing callers.
static ENGLISH_DESCRIPTIONS: std::sync::LazyLock<[String; DEFINITIONS.len()]> =
    std::sync::LazyLock::new(|| {
        std::array::from_fn(|i| {
            crate::copy::Output::canonical().message(DEFINITIONS[i].description_id, [])
        })
    });
static ROUTINES: std::sync::LazyLock<[RoutineDescriptor; DEFINITIONS.len()]> =
    std::sync::LazyLock::new(|| {
        std::array::from_fn(|i| {
            let definition = &DEFINITIONS[i];
            RoutineDescriptor {
                id: definition.id,
                description: &ENGLISH_DESCRIPTIONS[i],
                cadence: definition.cadence,
                max_runtime: definition.max_runtime,
            }
        })
    });

/// All routines in the fixed native maintenance census.
pub fn routines() -> &'static [RoutineDescriptor] {
    ROUTINES.as_slice()
}

impl RoutineDescriptor {
    pub(crate) fn display_description(self, copy: crate::copy::Output<'_>) -> String {
        // An injected descriptor can use the same ID with its own prose. Only
        // the registry's own string (including a copied descriptor) has a catalog identity.
        routines()
            .iter()
            .zip(&DEFINITIONS)
            .find(|(native, _)| {
                native.id == self.id && std::ptr::eq(native.description, self.description)
            })
            .map_or_else(
                || self.description.to_owned(),
                |(_, definition)| copy.message(definition.description_id, []),
            )
    }
}

/// Look up a routine by its stable `app:routine` identifier.
pub fn routine(id: &str) -> Option<&'static RoutineDescriptor> {
    routines().iter().find(|descriptor| descriptor.id == id)
}

#[cfg(test)]
fn validate_census(routines: &[RoutineDescriptor]) -> Result<(), String> {
    const EXPECTED: [(&str, Cadence, Option<&str>); 17] = [
        ("faces:reconcile", Cadence::Daily, Some("10m")),
        ("faces:backfill", Cadence::Hourly, Some("40m")),
        ("speakers:discover-voices", Cadence::Daily, Some("10m")),
        (
            "speakers:candidate-pair-suggestions",
            Cadence::Daily,
            Some("10m"),
        ),
        ("speakers:name-variants", Cadence::Daily, Some("10m")),
        ("speakers:consolidate-pool", Cadence::Daily, Some("10m")),
        ("speakers:backfill-pool", Cadence::Daily, Some("10m")),
        ("speakers:backfill-labels", Cadence::Daily, Some("30m")),
        ("backup:run", Cadence::Hourly, Some("49h")),
        ("backup:prune", Cadence::Daily, Some("3h")),
        ("backup:erase", Cadence::Hourly, Some("7h")),
        ("backup:verify", Cadence::Weekly, Some("90m")),
        ("backup:offload", Cadence::Daily, Some("7h")),
        ("health:mark-raw", Cadence::Daily, Some("60m")),
        ("health:prune-logs", Cadence::Daily, Some("30m")),
        ("trash:erase", Cadence::Hourly, Some("30m")),
        (
            "retention:owner-voice-originals",
            Cadence::Daily,
            Some("30m"),
        ),
    ];
    if routines.len() != EXPECTED.len() {
        return Err(format!(
            "routine count expected {}, got {}",
            EXPECTED.len(),
            routines.len()
        ));
    }
    for (id, cadence, max_runtime) in EXPECTED {
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Cadence, RoutineDescriptor, routine, routines, validate_census};
    use std::collections::BTreeSet;

    #[test]
    fn census_has_unique_well_formed_ids() {
        let all = routines();
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
