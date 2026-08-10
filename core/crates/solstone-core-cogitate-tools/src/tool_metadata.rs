// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::sync::OnceLock;

use solstone_core_cogitate::COGITATE_JOURNAL_COMMANDS;

/// Model-visible metadata for one cogitate tool.
#[derive(Debug, Eq, PartialEq)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub arguments: &'static [ToolArgumentSpec],
    pub read_only_hint: bool,
    pub destructive_hint: bool,
}

/// Model-visible metadata for one tool argument.
#[derive(Debug, Eq, PartialEq)]
pub struct ToolArgumentSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub required: bool,
}

static SOL_TOOL: OnceLock<ToolSpec> = OnceLock::new();

/// The `sol` descriptions derive their approved host vocabulary at construction.
pub fn sol_tool() -> &'static ToolSpec {
    SOL_TOOL.get_or_init(|| {
        let families = COGITATE_JOURNAL_COMMANDS.join(", ");
        let description = Box::leak(
            format!(
                "Run one policy-approved command directly, without a shell: use `sol`/`sol call ...` for normal journal access; run approved `journal` families ({families}) directly as `journal <family> ...`, never prefixed with `sol` or `sol call`."
            )
            .into_boxed_str(),
        );
        let argument_description = Box::leak(
            format!(
                "Single command-line invocation to run directly, without a shell: use `sol`/`sol call ...` for normal journal access; run approved `journal` families ({families}) directly as `journal <family> ...`, never prefixed with `sol` or `sol call`."
            )
            .into_boxed_str(),
        );
        let arguments = Box::leak(
            vec![ToolArgumentSpec {
                name: "command",
                description: argument_description,
                required: true,
            }]
            .into_boxed_slice(),
        );
        ToolSpec {
            name: "sol",
            description,
            arguments,
            read_only_hint: true,
            destructive_hint: false,
        }
    })
}

const EMIT_FINAL_ARGUMENTS: [ToolArgumentSpec; 1] = [ToolArgumentSpec {
    name: "content",
    description: "Final result to carry forward: artifact body or concise record of what changed.",
    required: true,
}];
pub const EMIT_FINAL_TOOL: ToolSpec = ToolSpec {
    name: "emit_final",
    description: "Terminal tool for ending the run with its final result.\n\nCall this tool exactly once when the run is complete. The content argument is the final result the system should carry forward.\n\nArtifact talents: when the talent produces an artifact, content is the complete artifact body itself, such as the markdown or text to save. Do not wrap it in commentary or describe the artifact instead of providing it.\n\nAction talents: when the talent's work was done through side-effecting commands during the run, content is a concise, signal-carrying record of what changed, what was found, and why. Do not emit a bare \"done\".\n\nNo-op: call this tool even when no changes were needed. Emit a brief result explaining why nothing changed rather than ending silently.\n",
    arguments: &EMIT_FINAL_ARGUMENTS,
    read_only_hint: true,
    destructive_hint: false,
};

const FINISH_ARGUMENTS: [ToolArgumentSpec; 1] = [ToolArgumentSpec {
    name: "message",
    description: "Concise record of what changed, what was found, or that already-persisted work is complete.",
    required: true,
}];
/// This native text is hand-maintained: Python owns no solstone `finish` text,
/// so the generated Python fixture cannot produce its companion contract entry.
/// `finish` is read-only and non-destructive because it only announces completion.
pub const FINISH_TOOL: ToolSpec = ToolSpec {
    name: "finish",
    description: "Terminal tool for ending the run when no `emit_final` tool is bound.\n\nCall this tool exactly once when the run is complete. The message argument is a concise, signal-carrying account of what was done: what changed, what was found, or why nothing changed.\n\nA side-effect-only run that already persisted its results through sol commands still calls this tool, with a short completion note rather than ending silently.\n",
    arguments: &FINISH_ARGUMENTS,
    read_only_hint: true,
    destructive_hint: false,
};

const READ_FILE_ARGUMENTS: [ToolArgumentSpec; 2] = [
    ToolArgumentSpec {
        name: "path",
        description: "Journal-root-relative text file path to read.",
        required: true,
    },
    ToolArgumentSpec {
        name: "start_line",
        description: "1-based line number to start reading from.",
        required: false,
    },
];
pub const READ_FILE_TOOL: ToolSpec = ToolSpec {
    name: "read_file",
    description: "Bounded UTF-8 journal text read. Paths are journal-root-relative; use start_line to paginate a truncation.",
    arguments: &READ_FILE_ARGUMENTS,
    read_only_hint: true,
    destructive_hint: false,
};

const LIST_DIRECTORY_ARGUMENTS: [ToolArgumentSpec; 4] = [
    ToolArgumentSpec {
        name: "path",
        description: "Journal-root-relative directory path.",
        required: false,
    },
    ToolArgumentSpec {
        name: "recursive",
        description: "Walk recursively below the directory.",
        required: false,
    },
    ToolArgumentSpec {
        name: "pattern",
        description: "Optional fnmatch pattern applied to each entry name.",
        required: false,
    },
    ToolArgumentSpec {
        name: "include_hidden",
        description: "Include hidden entries that are not otherwise denied.",
        required: false,
    },
];
pub const LIST_DIRECTORY_TOOL: ToolSpec = ToolSpec {
    name: "list_directory",
    description: "Journal-root-relative directory listing. Supports recursive walks and fnmatch patterns on entry names.",
    arguments: &LIST_DIRECTORY_ARGUMENTS,
    read_only_hint: true,
    destructive_hint: false,
};

const GLOB_ARGUMENTS: [ToolArgumentSpec; 3] = [
    ToolArgumentSpec {
        name: "pattern",
        description: "Recursive fnmatch pattern over journal-relative paths.",
        required: true,
    },
    ToolArgumentSpec {
        name: "root",
        description: "Journal-root-relative directory to narrow.",
        required: false,
    },
    ToolArgumentSpec {
        name: "include_hidden",
        description: "Include hidden entries that are not otherwise denied.",
        required: false,
    },
];
pub const GLOB_TOOL: ToolSpec = ToolSpec {
    name: "glob",
    description: "Recursive fnmatch over journal paths where '*' spans '/'; root narrows the search.",
    arguments: &GLOB_ARGUMENTS,
    read_only_hint: true,
    destructive_hint: false,
};

const GREP_SEARCH_ARGUMENTS: [ToolArgumentSpec; 7] = [
    ToolArgumentSpec {
        name: "pattern",
        description: "Literal text or regex pattern to search for.",
        required: true,
    },
    ToolArgumentSpec {
        name: "path",
        description: "Journal-root-relative file or directory.",
        required: false,
    },
    ToolArgumentSpec {
        name: "regex",
        description: "Treat pattern as a regular expression.",
        required: false,
    },
    ToolArgumentSpec {
        name: "case_sensitive",
        description: "Use case-sensitive matching.",
        required: false,
    },
    ToolArgumentSpec {
        name: "file_glob",
        description: "Optional fnmatch pattern over journal-relative file paths.",
        required: false,
    },
    ToolArgumentSpec {
        name: "context_lines",
        description: "Number of surrounding lines to include for each match.",
        required: false,
    },
    ToolArgumentSpec {
        name: "include_hidden",
        description: "Include hidden entries that are not otherwise denied.",
        required: false,
    },
];
pub const GREP_SEARCH_TOOL: ToolSpec = ToolSpec {
    name: "grep_search",
    description: "Literal-or-regex search of journal text files. Narrow with path and file_glob; context_lines adds surrounding lines.",
    arguments: &GREP_SEARCH_ARGUMENTS,
    read_only_hint: true,
    destructive_hint: false,
};
