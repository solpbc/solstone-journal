// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use serde_json::{Map, Value};

use crate::contract::PrePostState;
use crate::{
    ExecutionContext, PreparedTalent, RuntimeOutcome, StageError, apply_template_vars, stage_error,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ChatContextState {
    pub messages: Vec<Value>,
    pub template_vars: Map<String, Value>,
}

pub fn build(
    prepared: &mut PreparedTalent,
    _context: &ExecutionContext,
) -> Result<PrePostState, RuntimeOutcome> {
    let messages = prepared
        .config
        .get("messages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(PrePostState::ChatContext(ChatContextState {
        messages,
        template_vars: Map::new(),
    }))
}

pub fn apply_prompt_override(
    prepared: &mut PreparedTalent,
    state: &PrePostState,
) -> Result<(), StageError> {
    let PrePostState::ChatContext(state) = state else {
        return Err(stage_error(
            "prompt-override",
            "chat_context",
            prepared,
            "chat context state is missing",
        ));
    };
    apply_template_vars(&mut prepared.config, &state.template_vars);
    if !state.messages.is_empty() {
        prepared
            .config
            .insert("messages".to_owned(), Value::Array(state.messages.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::ExecutionContext;

    #[test]
    fn criterion_14_messages_replace_default_prompt_assembly() {
        let mut prepared = PreparedTalent {
            name: "chat".to_owned(),
            config: Map::from_iter([
                (
                    "messages".to_owned(),
                    json!([{"role":"user","content":"from stage"}]),
                ),
                ("prompt".to_owned(), json!("default")),
            ]),
        };
        let state = build(
            &mut prepared,
            &ExecutionContext {
                journal: Default::default(),
            },
        )
        .unwrap();
        apply_prompt_override(&mut prepared, &state).unwrap();
        assert_eq!(prepared.config["messages"][0]["content"], "from stage");
    }
}
