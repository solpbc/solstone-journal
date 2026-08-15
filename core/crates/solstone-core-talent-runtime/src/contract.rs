// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Closed, static hook-stage contract.  The runtime is not a plugin host.

use crate::{ExecutionContext, PreparedTalent, RuntimeOutcome, StageError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StageId {
    Documents,
    Steward,
    Story,
    ChatContext,
    Pulse,
    MorningBriefing,
    EntityDescribe,
    Participation,
    Schedule,
    DailySchedule,
    FacetNewsletter,
    EntityDetection,
    EntitiesReview,
    EntityObserver,
}

#[derive(Clone, Copy)]
pub struct HookBinding {
    pub hook: &'static str,
    pub stage: StageId,
}

pub type GateFn = fn(&PreparedTalent, &ExecutionContext) -> Result<GateDecision, StageError>;
pub type BuildFn =
    fn(&mut PreparedTalent, &ExecutionContext) -> Result<PrePostState, RuntimeOutcome>;
pub type PromptOverrideFn = fn(&mut PreparedTalent, &PrePostState) -> Result<(), StageError>;
pub type ParseOutputFn =
    fn(&str, &PreparedTalent, &PrePostState) -> Result<ParsedOutput, StageError>;
pub type CommitFn =
    fn(ParsedOutput, &PreparedTalent, &PrePostState) -> Result<CommitPlan, StageError>;
pub type WriteIntentFn = fn(CommitPlan, &ExecutionContext) -> Result<CommitDisposition, StageError>;
pub type OutputOverrideFn = fn(&str, &PreparedTalent, &PrePostState) -> Result<String, StageError>;

#[derive(Clone, Debug, PartialEq)]
pub enum GateDecision {
    Proceed,
    Skip(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PrePostState {
    None,
    Steward(crate::steward::StewardPreState),
    ChatContext(crate::chat_context::ChatContextState),
    Pulse(Box<crate::pulse::PulsePreState>),
    MorningBriefing(crate::morning_briefing::MorningBriefingPreState),
    EntityDescribe(crate::entities::describe::EntityDescribePreState),
    DailySchedule(crate::daily_schedule::DailySchedulePreState),
    FacetNewsletter(crate::facet_newsletter::FacetNewsletterState),
    EntityDetection(crate::entities::detection::DetectionState),
    EntitiesReview(crate::entities::review::ReviewState),
    EntityObserver(crate::entities::observer::ObserverState),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ParsedOutput {
    Text(String),
    Json(serde_json::Value),
}

#[derive(Clone, Debug, PartialEq)]
pub enum CommitPlan {
    NoOutput,
    Write(crate::writers::WriteIntent),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitDisposition {
    Written,
    CommittedNoOutput,
    RejectedNoMutation,
}

#[derive(Clone, Copy)]
pub struct CommitSpec {
    pub parse: ParseOutputFn,
    pub commit: CommitFn,
}

#[derive(Clone, Copy)]
pub struct StageSpec {
    pub stage: StageId,
    pub gate: Option<GateFn>,
    pub build: Option<BuildFn>,
    pub prompt_override: Option<PromptOverrideFn>,
    pub commit: Option<CommitSpec>,
    pub writes_as_intent: Option<WriteIntentFn>,
    pub output_override: Option<OutputOverrideFn>,
}

pub const HOOK_TABLE: [HookBinding; 14] = [
    HookBinding {
        hook: "documents",
        stage: StageId::Documents,
    },
    HookBinding {
        hook: "steward",
        stage: StageId::Steward,
    },
    HookBinding {
        hook: "story",
        stage: StageId::Story,
    },
    HookBinding {
        hook: "chat_context",
        stage: StageId::ChatContext,
    },
    HookBinding {
        hook: "pulse",
        stage: StageId::Pulse,
    },
    HookBinding {
        hook: "morning_briefing",
        stage: StageId::MorningBriefing,
    },
    HookBinding {
        hook: "entities:entity_describe",
        stage: StageId::EntityDescribe,
    },
    HookBinding {
        hook: "participation",
        stage: StageId::Participation,
    },
    HookBinding {
        hook: "schedule",
        stage: StageId::Schedule,
    },
    HookBinding {
        hook: "daily_schedule",
        stage: StageId::DailySchedule,
    },
    HookBinding {
        hook: "facet_newsletter",
        stage: StageId::FacetNewsletter,
    },
    HookBinding {
        hook: "entities:detection",
        stage: StageId::EntityDetection,
    },
    HookBinding {
        hook: "entities:entities_review",
        stage: StageId::EntitiesReview,
    },
    HookBinding {
        hook: "entities:entity_observer",
        stage: StageId::EntityObserver,
    },
];

pub static DOCUMENTS: StageSpec = StageSpec {
    stage: StageId::Documents,
    gate: Some(crate::documents::gate),
    build: None,
    prompt_override: None,
    commit: None,
    writes_as_intent: None,
    output_override: None,
};
pub static STEWARD: StageSpec = StageSpec {
    stage: StageId::Steward,
    gate: None,
    build: Some(crate::steward::build),
    prompt_override: Some(crate::steward::apply_prompt_override),
    commit: Some(CommitSpec {
        parse: crate::steward::parse,
        commit: crate::steward::commit,
    }),
    writes_as_intent: Some(crate::writers::apply),
    output_override: None,
};
pub static STORY: StageSpec = StageSpec {
    stage: StageId::Story,
    gate: None,
    build: None,
    prompt_override: None,
    commit: Some(CommitSpec {
        parse: crate::story::parse,
        commit: crate::story::commit,
    }),
    writes_as_intent: Some(crate::writers::apply),
    output_override: None,
};
pub static CHAT_CONTEXT: StageSpec = StageSpec {
    stage: StageId::ChatContext,
    gate: None,
    build: Some(crate::chat_context::build),
    prompt_override: Some(crate::chat_context::apply_prompt_override),
    commit: None,
    writes_as_intent: None,
    output_override: None,
};
pub static PULSE: StageSpec = StageSpec {
    stage: StageId::Pulse,
    gate: Some(crate::pulse::gate),
    build: Some(crate::pulse::build),
    prompt_override: Some(crate::pulse::apply_prompt_override),
    commit: Some(CommitSpec {
        parse: crate::pulse::parse,
        commit: crate::pulse::commit,
    }),
    writes_as_intent: Some(crate::writers::apply),
    output_override: Some(crate::pulse::output_override),
};
pub static MORNING_BRIEFING: StageSpec = StageSpec {
    stage: StageId::MorningBriefing,
    gate: Some(crate::morning_briefing::gate),
    build: Some(crate::morning_briefing::build),
    prompt_override: Some(crate::morning_briefing::apply_prompt_override),
    commit: None,
    writes_as_intent: None,
    output_override: None,
};
pub static ENTITY_DESCRIBE: StageSpec = StageSpec {
    stage: StageId::EntityDescribe,
    gate: Some(crate::entities::describe::gate),
    build: Some(crate::entities::describe::build),
    prompt_override: Some(crate::entities::describe::apply_prompt_override),
    commit: None,
    writes_as_intent: None,
    output_override: None,
};
pub static PARTICIPATION: StageSpec = StageSpec {
    stage: StageId::Participation,
    gate: None,
    build: None,
    prompt_override: None,
    commit: Some(CommitSpec {
        parse: crate::participation::parse,
        commit: crate::participation::commit,
    }),
    writes_as_intent: Some(crate::writers::apply),
    output_override: None,
};
pub static SCHEDULE: StageSpec = StageSpec {
    stage: StageId::Schedule,
    gate: None,
    build: None,
    prompt_override: None,
    commit: Some(CommitSpec {
        parse: crate::schedule::parse,
        commit: crate::schedule::commit,
    }),
    writes_as_intent: Some(crate::writers::apply),
    output_override: None,
};
pub static DAILY_SCHEDULE: StageSpec = StageSpec {
    stage: StageId::DailySchedule,
    gate: None,
    build: Some(crate::daily_schedule::build),
    prompt_override: Some(crate::daily_schedule::apply_prompt_override),
    commit: Some(CommitSpec {
        parse: crate::daily_schedule::parse,
        commit: crate::daily_schedule::commit,
    }),
    writes_as_intent: Some(crate::writers::apply),
    output_override: None,
};
pub static FACET_NEWSLETTER: StageSpec = StageSpec {
    stage: StageId::FacetNewsletter,
    gate: Some(crate::facet_newsletter::gate),
    build: Some(crate::facet_newsletter::build),
    prompt_override: Some(crate::facet_newsletter::apply_prompt_override),
    commit: Some(CommitSpec {
        parse: crate::facet_newsletter::parse,
        commit: crate::facet_newsletter::commit,
    }),
    writes_as_intent: Some(crate::writers::apply),
    output_override: None,
};
pub static ENTITY_DETECTION: StageSpec = StageSpec {
    stage: StageId::EntityDetection,
    gate: Some(crate::entities::detection::gate),
    build: Some(crate::entities::detection::build),
    prompt_override: Some(crate::entities::detection::apply_prompt_override),
    commit: Some(CommitSpec {
        parse: crate::entities::detection::parse,
        commit: crate::entities::detection::commit,
    }),
    writes_as_intent: Some(crate::writers::apply),
    output_override: None,
};
pub static ENTITIES_REVIEW: StageSpec = StageSpec {
    stage: StageId::EntitiesReview,
    gate: Some(crate::entities::review::gate),
    build: Some(crate::entities::review::build),
    prompt_override: Some(crate::entities::review::apply_prompt_override),
    commit: Some(CommitSpec {
        parse: crate::entities::review::parse,
        commit: crate::entities::review::commit,
    }),
    writes_as_intent: Some(crate::writers::apply),
    output_override: None,
};
pub static ENTITY_OBSERVER: StageSpec = StageSpec {
    stage: StageId::EntityObserver,
    gate: None,
    build: Some(crate::entities::observer::build),
    prompt_override: Some(crate::entities::observer::apply_prompt_override),
    commit: Some(CommitSpec {
        parse: crate::entities::observer::parse,
        commit: crate::entities::observer::commit,
    }),
    writes_as_intent: Some(crate::writers::apply),
    output_override: None,
};

pub fn resolve_hook(hook: &str) -> Option<&'static StageSpec> {
    let binding = HOOK_TABLE.iter().find(|binding| binding.hook == hook)?;
    Some(match binding.stage {
        StageId::Documents => &DOCUMENTS,
        StageId::Steward => &STEWARD,
        StageId::Story => &STORY,
        StageId::ChatContext => &CHAT_CONTEXT,
        StageId::Pulse => &PULSE,
        StageId::MorningBriefing => &MORNING_BRIEFING,
        StageId::EntityDescribe => &ENTITY_DESCRIBE,
        StageId::Participation => &PARTICIPATION,
        StageId::Schedule => &SCHEDULE,
        StageId::DailySchedule => &DAILY_SCHEDULE,
        StageId::FacetNewsletter => &FACET_NEWSLETTER,
        StageId::EntityDetection => &ENTITY_DETECTION,
        StageId::EntitiesReview => &ENTITIES_REVIEW,
        StageId::EntityObserver => &ENTITY_OBSERVER,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::ptr;

    use super::*;

    #[test]
    fn criterion_9_hook_table_is_closed_and_story_is_one_stage() {
        let root = tempfile::tempdir().unwrap();
        let talent_root = root.path().join("talent");
        let apps_root = root.path().join("apps");
        fs::create_dir_all(&talent_root).unwrap();
        fs::create_dir_all(&apps_root).unwrap();
        for name in ["conversation", "event", "work"] {
            fs::write(
                talent_root.join(format!("{name}.md")),
                "{\n\"hook\": {\"post\": \"story\"}\n}\nfixture",
            )
            .unwrap();
        }
        let configs = solstone_core_talent_config::discover(&talent_root, &apps_root).unwrap();
        let stages = ["conversation", "event", "work"].map(|name| {
            let hook = configs
                .iter()
                .find(|config| config.key == name)
                .and_then(|config| config.metadata.get("hook"))
                .and_then(serde_json::Value::as_object)
                .and_then(|hook| hook.get("post"))
                .and_then(serde_json::Value::as_str)
                .unwrap();
            resolve_hook(hook).unwrap()
        });
        assert!(ptr::eq(stages[0], stages[1]));
        assert!(ptr::eq(stages[1], stages[2]));
        assert!(ptr::eq(resolve_hook("documents").unwrap(), &DOCUMENTS));
        assert!(ptr::eq(resolve_hook("steward").unwrap(), &STEWARD));
        assert!(ptr::eq(
            resolve_hook("chat_context").unwrap(),
            &CHAT_CONTEXT
        ));
        assert!(resolve_hook("chat").is_none());
    }
}
