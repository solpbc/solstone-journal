// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::{
    env, fs,
    path::{Component, Path, PathBuf},
};

use solstone_core_setup::wrapper::{WrapperCommand, parse_wrapper};

/// Extract the `SOL_BIN` target from a managed source-install wrapper.
///
pub(crate) fn parse_sol_bin(content: &str) -> Option<PathBuf> {
    parse_wrapper(WrapperCommand::Journal, content).map(|wrapper| wrapper.sol_bin)
}

pub(crate) fn resolve_non_strict(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| lexical_absolute(path))
}

fn lexical_absolute(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v7_journal_wrapper(target: &str) -> String {
        format!(
            "#!/bin/bash\n# journal — managed by 'journal config'. Edits will be overwritten.\n# managed-version: 7\n: \"${{SOLSTONE_JOURNAL:=/journal}}\"\nexport SOLSTONE_JOURNAL\nSOL_BIN='{target}'\n# Warn when pyproject.toml or uv.lock is newer than .installed.\n# Skipped silently if .installed is absent.\nREPO_ROOT=\"${{SOL_BIN%/.venv/bin/journal}}\"\nif [ -f \"$REPO_ROOT/.installed\" ]; then\n  if [ \"$REPO_ROOT/pyproject.toml\" -nt \"$REPO_ROOT/.installed\" ] \\\n     || [ \"$REPO_ROOT/uv.lock\" -nt \"$REPO_ROOT/.installed\" ]; then\n    echo \"journal: WARNING — venv is stale (pyproject.toml or uv.lock changed since last install). Run: cd $REPO_ROOT && make install\" >&2\n  fi\nfi\nif [ ! -x \"$SOL_BIN\" ]; then\n    printf 'journal: venv binary missing or not executable: %s\\n' \"$SOL_BIN\" >&2\n    exit 127\nfi\nexec \"$SOL_BIN\" \"$@\"\n"
        )
    }

    #[test]
    fn parses_only_marked_wrappers_and_unescapes_sol_bin() {
        assert_eq!(
            parse_sol_bin(&v7_journal_wrapper("/tmp/it'\\''s/bin/journal")),
            Some(PathBuf::from("/tmp/it's/bin/journal"))
        );
        assert_eq!(parse_sol_bin("SOL_BIN='/tmp/bin/journal'\n"), None);
        assert_eq!(
            parse_sol_bin(&format!(
                "{}SOL_BIN='/tmp/other/journal'\n",
                v7_journal_wrapper("/tmp/bin/journal")
            )),
            None
        );
    }
}
