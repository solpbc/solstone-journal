// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

use serde_json::{Map, Value, json};
use solstone_core_cogitate_tools::{
    GlobOptions, GrepSearchOptions, ListDirectoryOptions, ReadBudget, ReadFileOptions, ReadPayload,
    ReadResult, SlotLease, SlotReacquireError, SolCallBudget, bound_tools, glob, grep_search,
    list_directory, read_file, run_sol_command,
};
use solstone_core_generate_wire::{ConverseToolCall, ConverseToolSpec};

use crate::config::RunConfig;

const INVALID_ARGUMENTS: &str =
    "tool_call_arguments_invalid: tool arguments did not match the bound schema";

/// A concrete tool observation returned to the model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolExecution {
    pub output: String,
    pub is_error: bool,
    pub sol_budget_exhausted: Option<(i64, i64)>,
    pub slot_reacquire_error: Option<String>,
}

impl ToolExecution {
    fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
            sol_budget_exhausted: None,
            slot_reacquire_error: None,
        }
    }
}

/// The runtime's provider-independent boundary for executing an offered tool.
pub trait ToolExecutor {
    fn offered_tools(&self, config: &RunConfig) -> Result<Vec<ConverseToolSpec>, String>;
    fn execute(&mut self, config: &RunConfig, call: &ConverseToolCall) -> ToolExecution;
}

/// Production adapter over the landed, bounded cogitate-tools crate.
pub struct CogitateToolExecutor<'a> {
    journal_root: &'a Path,
    read_budget: ReadBudget,
    sol_budget: SolCallBudget,
    slot: ObservedSlotLease<'a>,
}

impl<'a> CogitateToolExecutor<'a> {
    /// Python seeds its raw-read and sol counters from the same single config
    /// field (`openhands.py:521-540,1335-1336`), while keeping the counters
    /// independently scoped to their respective tool families.
    pub fn new(journal_root: &'a Path, read_call_budget: i64, slot: &'a mut dyn SlotLease) -> Self {
        Self {
            journal_root,
            read_budget: ReadBudget::new(read_call_budget),
            sol_budget: SolCallBudget::new(read_call_budget),
            slot: ObservedSlotLease::new(slot),
        }
    }
}

impl ToolExecutor for CogitateToolExecutor<'_> {
    fn offered_tools(&self, config: &RunConfig) -> Result<Vec<ConverseToolSpec>, String> {
        bound_tools(&config.access_tier, config.expects_emit_final)
            .map_err(|error| error.to_string())
            .map(|tools| tools.into_iter().map(tool_spec).collect())
    }

    fn execute(&mut self, config: &RunConfig, call: &ConverseToolCall) -> ToolExecution {
        if call.not_offered {
            return ToolExecution::error(solstone_core_cogitate_tools::REFUSAL_TOOL_NOT_BOUND);
        }
        match call.name.as_str() {
            "read_file" => self.read_file(&call.arguments),
            "list_directory" => self.list_directory(&call.arguments),
            "glob" => self.glob(&call.arguments),
            "grep_search" => self.grep_search(&call.arguments),
            "sol" => self.sol(config, &call.arguments),
            _ => ToolExecution::error(solstone_core_cogitate_tools::REFUSAL_TOOL_NOT_BOUND),
        }
    }
}

impl CogitateToolExecutor<'_> {
    fn read_file(&mut self, arguments: &Value) -> ToolExecution {
        let Some(path) = string(arguments, "path") else {
            return ToolExecution::error(INVALID_ARGUMENTS);
        };
        let mut options = ReadFileOptions::default();
        options.start_line = integer(arguments, "start_line").unwrap_or(options.start_line);
        options.max_lines = integer(arguments, "max_lines").unwrap_or(options.max_lines);
        ToolExecution::from_read(read_file(
            self.journal_root,
            path,
            &options,
            Some(&mut self.read_budget),
        ))
    }

    fn list_directory(&mut self, arguments: &Value) -> ToolExecution {
        let path = string(arguments, "path").unwrap_or("");
        let mut options = ListDirectoryOptions::default();
        options.recursive = boolean(arguments, "recursive").unwrap_or(options.recursive);
        options.include_hidden =
            boolean(arguments, "include_hidden").unwrap_or(options.include_hidden);
        options.pattern = string(arguments, "pattern").map(str::to_owned);
        ToolExecution::from_read(list_directory(
            self.journal_root,
            path,
            &options,
            Some(&mut self.read_budget),
        ))
    }

    fn glob(&mut self, arguments: &Value) -> ToolExecution {
        let Some(pattern) = string(arguments, "pattern") else {
            return ToolExecution::error(INVALID_ARGUMENTS);
        };
        let mut options = GlobOptions::default();
        options.include_hidden =
            boolean(arguments, "include_hidden").unwrap_or(options.include_hidden);
        let root = string(arguments, "root").unwrap_or("");
        ToolExecution::from_read(glob(
            self.journal_root,
            pattern,
            root,
            &options,
            Some(&mut self.read_budget),
        ))
    }

    fn grep_search(&mut self, arguments: &Value) -> ToolExecution {
        let Some(pattern) = string(arguments, "pattern") else {
            return ToolExecution::error(INVALID_ARGUMENTS);
        };
        let mut options = GrepSearchOptions::default();
        options.regex = boolean(arguments, "regex").unwrap_or(options.regex);
        options.case_sensitive =
            boolean(arguments, "case_sensitive").unwrap_or(options.case_sensitive);
        options.include_hidden =
            boolean(arguments, "include_hidden").unwrap_or(options.include_hidden);
        options.file_glob = string(arguments, "file_glob").map(str::to_owned);
        options.context_lines =
            integer(arguments, "context_lines").unwrap_or(options.context_lines);
        let root = string(arguments, "root").unwrap_or("");
        ToolExecution::from_read(grep_search(
            self.journal_root,
            pattern,
            root,
            &options,
            Some(&mut self.read_budget),
        ))
    }

    fn sol(&mut self, config: &RunConfig, arguments: &Value) -> ToolExecution {
        let Some(command) = string(arguments, "command") else {
            return ToolExecution::error(INVALID_ARGUMENTS);
        };
        match run_sol_command(
            command,
            &config.access_tier,
            config.outbound_approval.as_deref(),
            self.journal_root,
            &mut self.sol_budget,
            &mut self.slot,
        ) {
            Ok(result) => ToolExecution {
                output: result.observation.text,
                is_error: result.observation.is_error,
                sol_budget_exhausted: result
                    .budget_exhausted_event
                    .map(|event| (event.budget, event.count)),
                slot_reacquire_error: self.slot.take_terminal_error(),
            },
            Err(error) => ToolExecution::error(error.to_string()),
        }
    }
}

impl ToolExecution {
    fn from_read(result: ReadResult) -> Self {
        let output = result
            .refusal
            .map(str::to_owned)
            .unwrap_or_else(|| render_payload(result.payload));
        Self {
            output,
            is_error: !result.ok,
            sol_budget_exhausted: None,
            slot_reacquire_error: None,
        }
    }
}

struct ObservedSlotLease<'a> {
    inner: &'a mut dyn SlotLease,
    terminal_error: Option<String>,
}

impl<'a> ObservedSlotLease<'a> {
    fn new(inner: &'a mut dyn SlotLease) -> Self {
        Self {
            inner,
            terminal_error: None,
        }
    }
    fn take_terminal_error(&mut self) -> Option<String> {
        self.terminal_error.take()
    }
}

impl SlotLease for ObservedSlotLease<'_> {
    fn yield_slot(&mut self) {
        self.inner.yield_slot();
    }

    fn reacquire(&mut self) -> Result<(), SlotReacquireError> {
        match self.inner.reacquire() {
            Err(SlotReacquireError::Other(error)) => {
                self.terminal_error = Some(error.clone());
                Err(SlotReacquireError::Other(error))
            }
            result => result,
        }
    }

    fn cancel_pending_reacquire(&mut self) {
        self.inner.cancel_pending_reacquire();
    }
}

fn tool_spec(tool: &solstone_core_cogitate_tools::ToolSpec) -> ConverseToolSpec {
    ConverseToolSpec {
        name: tool.name.to_owned(),
        description: tool.description.to_owned(),
        parameters: schema(tool.name),
    }
}

fn schema(name: &str) -> Value {
    let fields: &[(&str, &str, bool)] = match name {
        "sol" => &[("command", "string", true)],
        "read_file" => &[
            ("path", "string", true),
            ("start_line", "integer", false),
            ("max_lines", "integer", false),
        ],
        "list_directory" => &[
            ("path", "string", true),
            ("recursive", "boolean", false),
            ("include_hidden", "boolean", false),
            ("pattern", "string", false),
        ],
        "glob" => &[
            ("pattern", "string", true),
            ("include_hidden", "boolean", false),
        ],
        "grep_search" => &[
            ("pattern", "string", true),
            ("regex", "boolean", false),
            ("case_sensitive", "boolean", false),
            ("file_glob", "string", false),
            ("context_lines", "integer", false),
            ("include_hidden", "boolean", false),
        ],
        "emit_final" => &[("content", "string", true)],
        "finish" => &[("message", "string", false)],
        _ => &[],
    };
    let properties = fields
        .iter()
        .map(|(key, ty, _)| ((*key).to_owned(), json!({"type": ty})))
        .collect::<Map<_, _>>();
    let required = fields
        .iter()
        .filter_map(|(key, _, required)| required.then_some(*key))
        .collect::<Vec<_>>();
    json!({"type":"object", "properties": properties, "required": required, "additionalProperties": false})
}

fn render_payload(payload: ReadPayload) -> String {
    match payload {
        ReadPayload::Text(text) => text,
        ReadPayload::Paths(paths) => paths.join("\n"),
        ReadPayload::Entries(entries) => entries
            .into_iter()
            .map(|entry| format!("{}{}", entry.path, if entry.is_dir { "/" } else { "" }))
            .collect::<Vec<_>>()
            .join("\n"),
        ReadPayload::Matches(matches) => matches
            .into_iter()
            .map(|item| format!("{}:{}:{}", item.path, item.lineno, item.line))
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}
fn integer(value: &Value, key: &str) -> Option<i64> {
    value.get(key).and_then(Value::as_i64)
}
fn boolean(value: &Value, key: &str) -> Option<bool> {
    value.get(key).and_then(Value::as_bool)
}
